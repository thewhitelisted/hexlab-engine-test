//! Rules engine for the base hex-board trading game (2-4 players), built so that expansion
//! rulesets can be added as modules: board topology is data, and rule variants are config.
//!
//! Entry points: [`GameState::new`], [`GameState::legal_actions`], [`GameState::apply`].

pub mod action_bytes;
pub mod actions;
mod apply;
pub mod belief;
pub mod bits;
pub mod board;
pub mod dice;
pub mod edit;
pub mod entered;
pub mod fuzz;
mod invariants;
pub mod legal;
mod longest_road;
pub mod observation;
pub mod record;
pub mod rng;
pub mod rules;
pub mod sim;
pub mod state;
pub mod topology;
pub mod types;
pub mod view;

pub use actions::{Action, Forced, Outcome, RuleError};
pub use belief::BeliefTracker;
pub use board::{Board, BoardGen, BoardLayout};
pub use observation::Observation;
pub use record::GameRecord;
pub use rng::Rng;
pub use rules::{GameConfig, PlayerCount, Rules};
pub use state::{GameState, Phase};
pub use types::*;
pub use view::View;
