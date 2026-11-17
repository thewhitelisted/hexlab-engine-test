//! Game records: enough to replay a game exactly and to show it in the review tools.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::actions::{Action, Outcome, RuleError};
use crate::board::{Board, BoardError, BoardLayout};
use crate::rng::Rng;
use crate::rules::GameConfig;
use crate::state::GameState;
use crate::types::PlayerId;

pub const RECORD_FORMAT: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub actor: PlayerId,
    pub action: Action,
    #[serde(default, skip_serializing_if = "is_no_outcome")]
    pub outcome: Outcome,
}

fn is_no_outcome(o: &Outcome) -> bool {
    *o == Outcome::None
}

/// A complete game from the referee's view (all chance outcomes included).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameRecord {
    pub format: u32,
    pub config: GameConfig,
    pub seed: u64,
    pub board: BoardLayout,
    pub steps: Vec<Step>,
    pub winner: Option<PlayerId>,
    pub final_hash: u64,
}

#[derive(Debug)]
pub enum ReplayError {
    Format(u32),
    Board(BoardError),
    Rule { step: usize, error: RuleError },
    WrongActor { step: usize },
    OutcomeMismatch { step: usize },
    FinalPosition,
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplayError::Format(v) => write!(f, "unsupported record format {v}"),
            ReplayError::Board(e) => write!(f, "bad board: {e}"),
            ReplayError::Rule { step, error } => write!(f, "step {step}: {error}"),
            ReplayError::WrongActor { step } => {
                write!(f, "step {step}: not that player's decision")
            }
            ReplayError::OutcomeMismatch { step } => {
                write!(f, "step {step}: outcome differs from the record")
            }
            ReplayError::FinalPosition => write!(f, "final position differs from the record"),
        }
    }
}

impl std::error::Error for ReplayError {}

impl GameRecord {
    /// Starts a record for a game created with `GameState::new(config, seed)`.
    pub fn start(config: &GameConfig, seed: u64, state: &GameState) -> GameRecord {
        GameRecord {
            format: RECORD_FORMAT,
            config: *config,
            seed,
            board: state.board.to_layout(),
            steps: Vec::new(),
            winner: None,
            final_hash: 0,
        }
    }

    /// Applies `action` to `state` and appends it to the record.
    pub fn play(&mut self, state: &mut GameState, action: Action) -> Result<Outcome, RuleError> {
        let actor = state.acting_player();
        let outcome = state.apply(action)?;
        self.steps.push(Step {
            actor,
            action,
            outcome,
        });
        Ok(outcome)
    }

    pub fn finish(&mut self, state: &GameState) {
        self.winner = state.winner;
        self.final_hash = state.position_hash();
    }

    /// Replays the record with its recorded chance outcomes and checks the final position.
    pub fn replay(&self) -> Result<GameState, ReplayError> {
        let state = self.replay_steps(|_, _| {})?;
        if state.position_hash() != self.final_hash || state.winner != self.winner {
            return Err(ReplayError::FinalPosition);
        }
        Ok(state)
    }

    /// Replays the steps with their recorded chance outcomes, showing `seen` the state before each
    /// step, and returns the state after the last. It doesn't check the final position, so it
    /// also reads the record of a game still in play.
    pub fn replay_steps(
        &self,
        mut seen: impl FnMut(&GameState, &Step),
    ) -> Result<GameState, ReplayError> {
        if self.format != RECORD_FORMAT {
            return Err(ReplayError::Format(self.format));
        }
        let board = Board::from_layout(&self.board).map_err(ReplayError::Board)?;
        let mut state =
            GameState::with_board(self.config.rules, Arc::new(board), Rng::seed(self.seed));
        for (i, step) in self.steps.iter().enumerate() {
            if state.is_over() || state.acting_player() != step.actor {
                return Err(ReplayError::WrongActor { step: i });
            }
            seen(&state, step);
            let outcome = state
                .apply_forced(step.action, step.outcome.as_forced())
                .map_err(|error| ReplayError::Rule { step: i, error })?;
            if outcome != step.outcome {
                return Err(ReplayError::OutcomeMismatch { step: i });
            }
        }
        Ok(state)
    }
}
