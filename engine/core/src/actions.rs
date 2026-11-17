//! Player actions, chance outcomes and rule errors.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::types::{DevCard, EdgeId, Hand, HexId, MAX_PLAYERS, PlayerId, Resource, VertexId};

/// A decision by the acting player ([`crate::GameState::acting_player`]). The actor is implied
/// by the game state, so actions don't name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Initial placement or a paid build.
    BuildSettlement {
        vertex: VertexId,
    },
    /// Initial placement, a paid build or a free Road Building placement.
    BuildRoad {
        edge: EdgeId,
    },
    BuildCity {
        vertex: VertexId,
    },
    BuyDevCard,
    RollDice,
    PlayKnight,
    PlayRoadBuilding,
    PlayYearOfPlenty {
        a: Resource,
        b: Resource,
    },
    PlayMonopoly {
        resource: Resource,
    },
    /// Move the robber and steal from `victim`, which is required whenever someone adjacent has cards.
    MoveRobber {
        hex: HexId,
        victim: Option<PlayerId>,
    },
    /// Discard one card after a 7. Repeated until the player has discarded half their hand.
    Discard {
        resource: Resource,
    },
    /// Trade with the bank at the best ratio the player's ports allow.
    MaritimeTrade {
        give: Resource,
        get: Resource,
    },
    OfferTrade {
        give: Hand,
        get: Hand,
    },
    AcceptTrade,
    /// Declines the offer, or passes in a round of proposals.
    RejectTrade,
    /// The player on turn takes the trade `with` accepted, or `with`'s proposal.
    ConfirmTrade {
        with: PlayerId,
    },
    CancelTrade,
    EndTurn,
    // Last, so the variants before it keep their hashes (decision digests, golden corpus).
    /// A trade proposed to the player on turn (`open_trading`): a counter-offer when answering an
    /// offer, or a proposal in the round after a roll. `give` is what the proposer gives.
    ProposeTrade {
        give: Hand,
        get: Hand,
    },
}

impl Action {
    /// Compact text code, keyed by this engine's ids. Used by the differential test and the
    /// bridge to other engines (`tools/difftest`).
    pub fn code(&self) -> String {
        match *self {
            Action::BuildSettlement { vertex } => format!("S{vertex}"),
            Action::BuildRoad { edge } => format!("R{edge}"),
            Action::BuildCity { vertex } => format!("C{vertex}"),
            Action::BuyDevCard => "BUY".into(),
            Action::RollDice => "ROLL".into(),
            Action::PlayKnight => "KN".into(),
            Action::PlayRoadBuilding => "RB".into(),
            Action::PlayYearOfPlenty { a, b } => format!("Y{}{}", a.index(), b.index()),
            Action::PlayMonopoly { resource } => format!("MO{}", resource.index()),
            Action::MoveRobber { hex, victim } => match victim {
                Some(v) => format!("MV{hex}:{v}"),
                None => format!("MV{hex}:-"),
            },
            Action::Discard { resource } => format!("DI{}", resource.index()),
            Action::MaritimeTrade { give, get } => format!("MT{}{}", give.index(), get.index()),
            Action::EndTurn => "END".into(),
            Action::OfferTrade { give, get } => {
                let counts = |h: Hand| h.0.map(|c| c.to_string()).join(",");
                format!("OF{}/{}", counts(give), counts(get))
            }
            Action::ProposeTrade { give, get } => {
                let counts = |h: Hand| h.0.map(|c| c.to_string()).join(",");
                format!("PR{}/{}", counts(give), counts(get))
            }
            Action::AcceptTrade => "ACC".into(),
            Action::RejectTrade => "REJ".into(),
            Action::ConfirmTrade { with } => format!("CF{with}"),
            Action::CancelTrade => "CAN".into(),
        }
    }
}

/// What chance (or hidden state) decided when an action was applied.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outcome {
    #[default]
    None,
    Dice {
        a: u8,
        b: u8,
    },
    /// `None` when the card is hidden from the observer.
    DevCard {
        card: Option<DevCard>,
    },
    /// `None` when the stolen card is hidden from the observer.
    Stole {
        resource: Option<Resource>,
    },
    /// Cards of the named resource taken from each player (public).
    Monopoly {
        taken: [u8; MAX_PLAYERS],
    },
}

impl Outcome {
    /// The outcome as `observer` sees it: drawn dev cards are private to the buyer, stolen cards
    /// to the thief and the victim.
    pub fn visible_to(&self, observer: PlayerId, actor: PlayerId, action: &Action) -> Outcome {
        match *self {
            Outcome::DevCard { .. } if observer != actor => Outcome::DevCard { card: None },
            Outcome::Stole { .. } => {
                let victim = match action {
                    Action::MoveRobber { victim, .. } => *victim,
                    _ => None,
                };
                if observer == actor || Some(observer) == victim {
                    *self
                } else {
                    Outcome::Stole { resource: None }
                }
            }
            other => other,
        }
    }

    /// Compact JSON form used with [`Action::code`]: dice as `[a, b]`, a drawn card or stolen
    /// resource as its index, otherwise null.
    pub fn code(&self) -> serde_json::Value {
        match *self {
            Outcome::Dice { a, b } => serde_json::json!([a, b]),
            Outcome::DevCard { card: Some(c) } => serde_json::json!(c.index()),
            Outcome::Stole { resource: Some(r) } => serde_json::json!(r.index()),
            _ => serde_json::Value::Null,
        }
    }

    /// The forced outcome that reproduces this outcome on replay, if chance was involved.
    pub fn as_forced(&self) -> Option<Forced> {
        match *self {
            Outcome::Dice { a, b } => Some(Forced::Dice(a, b)),
            Outcome::DevCard { card: Some(c) } => Some(Forced::DevCard(c)),
            Outcome::Stole { resource: Some(r) } => Some(Forced::Steal(r)),
            _ => None,
        }
    }
}

/// A chance outcome imposed on an action instead of drawing it from the game's RNG. Used for
/// replays, search over chance nodes and differential testing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Forced {
    Dice(u8, u8),
    DevCard(DevCard),
    Steal(Resource),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleError {
    GameOver,
    WrongPhase,
    InvalidId,
    IllegalLocation,
    CannotAfford,
    NoPiecesLeft,
    NoCard,
    DevCardAlreadyPlayed,
    DevCardNotAllowedNow,
    DeckEmpty,
    BankEmpty,
    InvalidRobberHex,
    InvalidVictim,
    InvalidTrade,
    TradingDisabled,
    OfferLimitReached,
    InvalidForcedOutcome,
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            RuleError::GameOver => "the game is over",
            RuleError::WrongPhase => "action not allowed in this phase",
            RuleError::InvalidId => "no such vertex, edge, hex or player",
            RuleError::IllegalLocation => "cannot build there",
            RuleError::CannotAfford => "not enough resources",
            RuleError::NoPiecesLeft => "no pieces of that kind left",
            RuleError::NoCard => "player does not hold that card",
            RuleError::DevCardAlreadyPlayed => "a development card was already played this turn",
            RuleError::DevCardNotAllowedNow => "that development card cannot be played now",
            RuleError::DeckEmpty => "development deck is empty",
            RuleError::BankEmpty => "the bank has run out of that resource",
            RuleError::InvalidRobberHex => "robber cannot move there",
            RuleError::InvalidVictim => "invalid robbery victim",
            RuleError::InvalidTrade => "invalid trade",
            RuleError::TradingDisabled => "player trading is disabled",
            RuleError::OfferLimitReached => "no more trade offers this turn",
            RuleError::InvalidForcedOutcome => "forced outcome does not fit the action or state",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for RuleError {}
