//! Position editing: build a position directly instead of playing up to it. Used by the
//! analysis board (positions typed in from real games) and by rule tests.
//!
//! Start from [`GameState::empty_position`], place pieces and cards, then call
//! [`GameState::finish_edit`], which recomputes derived state and validates the result.

use std::fmt;
use std::sync::Arc;

use crate::board::{Board, BoardError};
use crate::rng::Rng;
use crate::rules::Rules;
use crate::state::{GameState, Phase, Resume, city_code};
use crate::types::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    InvalidId,
    Occupied,
    DistanceRule,
    NoPiecesLeft,
    NotOwnSettlement,
    BankShort(Resource),
    CardNotInDeck(DevCard),
    Invalid(String),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::InvalidId => write!(f, "no such vertex, edge, hex or player"),
            EditError::Occupied => write!(f, "spot already taken"),
            EditError::DistanceRule => write!(f, "too close to another building"),
            EditError::NoPiecesLeft => write!(f, "player has no pieces of that kind left"),
            EditError::NotOwnSettlement => write!(f, "a city must replace the player's settlement"),
            EditError::BankShort(r) => write!(f, "not enough {r:?} left in the bank"),
            EditError::CardNotInDeck(c) => write!(f, "no {c:?} left in the deck"),
            EditError::Invalid(m) => write!(f, "invalid position: {m}"),
        }
    }
}

impl std::error::Error for EditError {}

/// The last turn a position may be set to. Far past any game (playouts stop at 1,000 turns by
/// default), and far enough from `u32::MAX` that the turns played on from it don't wrap round to
/// turn 0, the initial placement, where nobody can win.
pub const MAX_TURN: u32 = 100_000;

impl GameState {
    /// An empty board after initial placement: player 0 to roll on turn 1. The placement is
    /// over, so of what [`Board::hosts`] checks only the robber's move applies: a board of one
    /// hex is refused.
    pub fn empty_position(
        rules: Rules,
        board: Arc<Board>,
        seed: u64,
    ) -> Result<GameState, BoardError> {
        board.robber_can_move()?;
        let mut s = GameState::on_board(rules, board, Rng::seed(seed));
        s.setup_step = 2 * rules.num_players.get();
        s.turn = 1;
        s.phase = Phase::PreRoll;
        Ok(s)
    }

    fn check_player(&self, p: PlayerId) -> Result<(), EditError> {
        if p < self.rules.num_players.get() {
            Ok(())
        } else {
            Err(EditError::InvalidId)
        }
    }

    pub fn put_settlement(&mut self, p: PlayerId, v: VertexId) -> Result<(), EditError> {
        self.check_player(p)?;
        if v as usize >= self.board.topology.num_vertices() {
            return Err(EditError::InvalidId);
        }
        if self.vertex[v as usize] != 0 {
            return Err(EditError::Occupied);
        }
        if !self.vertex_open(v) {
            return Err(EditError::DistanceRule);
        }
        if self.players[p as usize].settlements_left == 0 {
            return Err(EditError::NoPiecesLeft);
        }
        self.place_settlement(p, v);
        if let Some(port) = self.board.vertex_port[v as usize] {
            self.players[p as usize].ports |= port.bit();
        }
        Ok(())
    }

    /// Places a city: on the player's settlement, which goes back to their stock, or whole on
    /// an empty spot, which leaves the settlement stock alone (so five settlements and a city,
    /// a position that can happen, can be set up in any order).
    pub fn put_city(&mut self, p: PlayerId, v: VertexId) -> Result<(), EditError> {
        self.check_player(p)?;
        if v as usize >= self.board.topology.num_vertices() {
            return Err(EditError::InvalidId);
        }
        if self.vertex[v as usize] == 0 {
            if !self.vertex_open(v) {
                return Err(EditError::DistanceRule);
            }
            let ps = &mut self.players[p as usize];
            if ps.cities_left == 0 {
                return Err(EditError::NoPiecesLeft);
            }
            ps.cities_left -= 1;
            self.vertex[v as usize] = city_code(p);
            self.buildings_of[p as usize].set(v);
            self.occupied.set(v);
            self.blocked |= self.board.topology.vertex_block_bits[v as usize];
            if let Some(port) = self.board.vertex_port[v as usize] {
                self.players[p as usize].ports |= port.bit();
            }
            return Ok(());
        }
        match self.building(v) {
            Some(b) if b.owner == p && !b.city => {}
            _ => return Err(EditError::NotOwnSettlement),
        }
        let ps = &mut self.players[p as usize];
        if ps.cities_left == 0 {
            return Err(EditError::NoPiecesLeft);
        }
        ps.cities_left -= 1;
        ps.settlements_left += 1;
        self.vertex[v as usize] = city_code(p);
        Ok(())
    }

    pub fn put_road(&mut self, p: PlayerId, e: EdgeId) -> Result<(), EditError> {
        self.check_player(p)?;
        if e as usize >= self.board.topology.num_edges() {
            return Err(EditError::InvalidId);
        }
        if self.edge[e as usize] != 0 {
            return Err(EditError::Occupied);
        }
        if self.players[p as usize].roads_left == 0 {
            return Err(EditError::NoPiecesLeft);
        }
        self.place_road(p, e);
        Ok(())
    }

    /// Sets a player's resource cards, returning the old ones to the bank.
    pub fn set_hand(&mut self, p: PlayerId, hand: Hand) -> Result<(), EditError> {
        self.check_player(p)?;
        let old = self.players[p as usize].hand;
        let mut bank = self.bank;
        bank.add(&old);
        for r in Resource::ALL {
            if bank.get(r) < hand.get(r) {
                return Err(EditError::BankShort(r));
            }
        }
        bank.sub(&hand);
        self.bank = bank;
        self.players[p as usize].hand = hand;
        Ok(())
    }

    fn take_from_deck(&mut self, card: DevCard) -> Result<(), EditError> {
        let i = self
            .dev_deck
            .iter()
            .position(|&c| c == card)
            .ok_or(EditError::CardNotInDeck(card))?;
        self.dev_deck.remove(i);
        Ok(())
    }

    /// Gives `p` an unplayed development card from the deck (`fresh`: bought this turn).
    pub fn give_dev_card(
        &mut self,
        p: PlayerId,
        card: DevCard,
        fresh: bool,
    ) -> Result<(), EditError> {
        self.check_player(p)?;
        self.take_from_deck(card)?;
        let ps = &mut self.players[p as usize];
        if fresh {
            ps.dev_new[card.index()] += 1;
        } else {
            ps.dev[card.index()] += 1;
        }
        Ok(())
    }

    /// Records `card` as already played by `p` (e.g. knights in front of them).
    pub fn add_played_card(&mut self, p: PlayerId, card: DevCard) -> Result<(), EditError> {
        self.check_player(p)?;
        if card == DevCard::VictoryPoint {
            return Err(EditError::Invalid(
                "victory point cards are never played".into(),
            ));
        }
        self.take_from_deck(card)?;
        self.players[p as usize].dev_played[card.index()] += 1;
        Ok(())
    }

    pub fn set_robber(&mut self, h: HexId) -> Result<(), EditError> {
        if h as usize >= self.board.topology.num_hexes() {
            return Err(EditError::InvalidId);
        }
        self.robber = h;
        Ok(())
    }

    /// Whose turn it is, and whether they have rolled yet.
    pub fn set_turn(
        &mut self,
        current: PlayerId,
        turn: u32,
        rolled: bool,
    ) -> Result<(), EditError> {
        self.check_player(current)?;
        if turn == 0 {
            return Err(EditError::Invalid("turn 0 is initial placement".into()));
        }
        if turn > MAX_TURN {
            return Err(EditError::Invalid(format!(
                "turn {turn}: a position is at turn {MAX_TURN} at most"
            )));
        }
        self.current = current;
        self.turn = turn;
        self.phase = if rolled { Phase::Main } else { Phase::PreRoll };
        self.dev_played_this_turn = false;
        self.offers_this_turn = 0;
        Ok(())
    }

    /// Overrides who holds Longest Road and Largest Army, for ties the board alone can't settle.
    /// Each holder must be a player of this game; whether they can hold the award is validated
    /// by [`GameState::finish_edit`].
    pub fn set_awards(
        &mut self,
        longest_road: Option<PlayerId>,
        largest_army: Option<PlayerId>,
    ) -> Result<(), EditError> {
        for holder in [longest_road, largest_army].into_iter().flatten() {
            self.check_player(holder)?;
        }
        self.longest_road = longest_road;
        self.largest_army = largest_army;
        Ok(())
    }

    /// The same position with `p` to move the robber at the start of their turn, as with a knight
    /// played before rolling but no card spent: for comparing robber placements in any position.
    /// Another player's turn ends first, so cards they bought this turn become playable, and
    /// discards still owed are let go.
    pub fn robber_to_move(&self, p: PlayerId) -> Result<GameState, EditError> {
        self.check_player(p)?;
        if self.turn == 0 || self.is_over() {
            return Err(EditError::Invalid(
                "the robber moves only between the initial placement and the end".into(),
            ));
        }
        let mut s = self.clone();
        if s.current != p {
            let ps = &mut s.players[s.current as usize];
            for i in 0..NUM_DEV_TYPES {
                ps.dev[i] += ps.dev_new[i];
                ps.dev_new[i] = 0;
            }
        }
        s.trade = None;
        s.discard_pending = [0; MAX_PLAYERS];
        s.set_turn(p, s.turn, false)?;
        s.phase = Phase::MoveRobber {
            resume: Resume::PreRoll,
        };
        s.check_invariants().map_err(EditError::Invalid)?;
        Ok(s)
    }

    /// Recomputes road lengths, assigns any award the position clearly decides, and validates.
    pub fn finish_edit(&mut self) -> Result<(), EditError> {
        for q in 0..self.rules.num_players.get() {
            self.players[q as usize].road_length = self.compute_road_length(q);
        }
        self.update_longest_road();
        let n = self.num_players();
        let knights = |q: usize| self.players[q].knights_played();
        let max_knights = (0..n).map(knights).max().unwrap_or(0);
        let holder_ok = self
            .largest_army
            .is_some_and(|h| knights(h as usize) == max_knights);
        if !holder_ok {
            let mut leaders = (0..n).filter(|&q| knights(q) == max_knights);
            self.largest_army = match (leaders.next(), leaders.next()) {
                (Some(q), None) if max_knights >= crate::state::LARGEST_ARMY_MIN => {
                    Some(q as PlayerId)
                }
                _ => None,
            };
        }
        self.check_win();
        self.check_invariants().map_err(EditError::Invalid)
    }
}
