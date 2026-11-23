//! Actions as bytes: one tag byte, then the action's fields (docs/BOTS.md, "Stored moves").
//! `hexlab positions --moves` stores games this way; the fuzzer round-trips every action it plays.
//!
//! The bytes come from a file, so `read` checks every field it can without a board: the tag, the
//! resources, the players and the card counts. Vertex, edge and hex numbers are left to the rules
//! (`GameState::check`), which know the board.

use std::fmt;

use crate::actions::Action;
use crate::types::{BANK_PER_RESOURCE, Hand, MAX_PLAYERS, NUM_RESOURCES, PlayerId, Resource};

/// The most bytes one action takes: a tag and two sets of cards.
pub const MAX_LEN: usize = 1 + 2 * NUM_RESOURCES;

/// The victim byte of a robber move that steals from nobody.
pub const NOBODY: u8 = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionBytesError {
    /// The bytes end inside an action.
    Truncated,
    UnknownTag(u8),
    /// A resource number other than 0 to 4.
    Resource(u8),
    /// A player number of 6 or more (or, for a victim, other than 255).
    Player(u8),
    /// More cards of one resource than exist.
    Count(u8),
}

impl fmt::Display for ActionBytesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ActionBytesError::Truncated => write!(f, "the bytes end inside a move"),
            ActionBytesError::UnknownTag(t) => write!(f, "no move has tag {t}"),
            ActionBytesError::Resource(r) => write!(f, "no resource has number {r}"),
            ActionBytesError::Player(p) => write!(f, "no player has number {p}"),
            ActionBytesError::Count(c) => {
                write!(
                    f,
                    "{c} cards of one resource; there are {BANK_PER_RESOURCE}"
                )
            }
        }
    }
}

impl std::error::Error for ActionBytesError {}

fn resource(r: Resource) -> u8 {
    // `Resource` is `repr(u8)` with the values 0 to 4.
    r as u8
}

fn hand(out: &mut Vec<u8>, h: Hand) {
    out.extend(h.0);
}

/// Appends `action` to `out`. `read` gives back any action the rules can accept (players below
/// 6, at most 19 cards of a resource).
pub fn write(action: &Action, out: &mut Vec<u8>) {
    match *action {
        Action::BuildSettlement { vertex } => {
            out.push(0);
            out.extend(vertex.to_le_bytes());
        }
        Action::BuildRoad { edge } => {
            out.push(1);
            out.extend(edge.to_le_bytes());
        }
        Action::BuildCity { vertex } => {
            out.push(2);
            out.extend(vertex.to_le_bytes());
        }
        Action::BuyDevCard => out.push(3),
        Action::RollDice => out.push(4),
        Action::PlayKnight => out.push(5),
        Action::PlayRoadBuilding => out.push(6),
        Action::PlayYearOfPlenty { a, b } => out.extend([7, resource(a), resource(b)]),
        Action::PlayMonopoly { resource: r } => out.extend([8, resource(r)]),
        Action::MoveRobber { hex, victim } => out.extend([9, hex, victim.unwrap_or(NOBODY)]),
        Action::Discard { resource: r } => out.extend([10, resource(r)]),
        Action::MaritimeTrade { give, get } => out.extend([11, resource(give), resource(get)]),
        Action::OfferTrade { give, get } => {
            out.push(12);
            hand(out, give);
            hand(out, get);
        }
        Action::AcceptTrade => out.push(13),
        Action::RejectTrade => out.push(14),
        Action::ConfirmTrade { with } => out.extend([15, with]),
        Action::CancelTrade => out.push(16),
        Action::EndTurn => out.push(17),
        Action::ProposeTrade { give, get } => {
            out.push(18);
            hand(out, give);
            hand(out, get);
        }
    }
}

/// Reads the bytes of an action from `bytes`, as `write` wrote them.
struct Fields<'a> {
    bytes: &'a [u8],
    used: usize,
}

impl Fields<'_> {
    fn byte(&mut self) -> Result<u8, ActionBytesError> {
        let b = *self
            .bytes
            .get(self.used)
            .ok_or(ActionBytesError::Truncated)?;
        self.used += 1;
        Ok(b)
    }

    fn u16(&mut self) -> Result<u16, ActionBytesError> {
        Ok(u16::from_le_bytes([self.byte()?, self.byte()?]))
    }

    fn resource(&mut self) -> Result<Resource, ActionBytesError> {
        let r = self.byte()?;
        Resource::ALL
            .get(usize::from(r))
            .copied()
            .ok_or(ActionBytesError::Resource(r))
    }

    fn player(&mut self) -> Result<PlayerId, ActionBytesError> {
        let p = self.byte()?;
        if usize::from(p) < MAX_PLAYERS {
            Ok(p)
        } else {
            Err(ActionBytesError::Player(p))
        }
    }

    fn hand(&mut self) -> Result<Hand, ActionBytesError> {
        let mut h = Hand::EMPTY;
        for c in h.0.iter_mut() {
            let n = self.byte()?;
            if n > BANK_PER_RESOURCE {
                return Err(ActionBytesError::Count(n));
            }
            *c = n;
        }
        Ok(h)
    }
}

/// The action at the start of `bytes`, and how many bytes it took.
pub fn read(bytes: &[u8]) -> Result<(Action, usize), ActionBytesError> {
    let mut f = Fields { bytes, used: 0 };
    let action = match f.byte()? {
        0 => Action::BuildSettlement { vertex: f.u16()? },
        1 => Action::BuildRoad { edge: f.u16()? },
        2 => Action::BuildCity { vertex: f.u16()? },
        3 => Action::BuyDevCard,
        4 => Action::RollDice,
        5 => Action::PlayKnight,
        6 => Action::PlayRoadBuilding,
        7 => Action::PlayYearOfPlenty {
            a: f.resource()?,
            b: f.resource()?,
        },
        8 => Action::PlayMonopoly {
            resource: f.resource()?,
        },
        9 => {
            let hex = f.byte()?;
            let victim = match f.byte()? {
                NOBODY => None,
                p if usize::from(p) < MAX_PLAYERS => Some(p),
                p => return Err(ActionBytesError::Player(p)),
            };
            Action::MoveRobber { hex, victim }
        }
        10 => Action::Discard {
            resource: f.resource()?,
        },
        11 => Action::MaritimeTrade {
            give: f.resource()?,
            get: f.resource()?,
        },
        12 => Action::OfferTrade {
            give: f.hand()?,
            get: f.hand()?,
        },
        13 => Action::AcceptTrade,
        14 => Action::RejectTrade,
        15 => Action::ConfirmTrade { with: f.player()? },
        16 => Action::CancelTrade,
        17 => Action::EndTurn,
        18 => Action::ProposeTrade {
            give: f.hand()?,
            get: f.hand()?,
        },
        tag => return Err(ActionBytesError::UnknownTag(tag)),
    };
    Ok((action, f.used))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One action of every kind, at the edges of each field.
    fn every_kind() -> Vec<Action> {
        let most = Hand([BANK_PER_RESOURCE; NUM_RESOURCES]);
        vec![
            Action::BuildSettlement { vertex: 0 },
            Action::BuildSettlement { vertex: u16::MAX },
            Action::BuildRoad { edge: 0x1234 },
            Action::BuildCity { vertex: 53 },
            Action::BuyDevCard,
            Action::RollDice,
            Action::PlayKnight,
            Action::PlayRoadBuilding,
            Action::PlayYearOfPlenty {
                a: Resource::Wood,
                b: Resource::Ore,
            },
            Action::PlayMonopoly {
                resource: Resource::Sheep,
            },
            Action::MoveRobber {
                hex: 18,
                victim: None,
            },
            Action::MoveRobber {
                hex: 255,
                victim: Some(5),
            },
            Action::Discard {
                resource: Resource::Wheat,
            },
            Action::MaritimeTrade {
                give: Resource::Brick,
                get: Resource::Wood,
            },
            Action::OfferTrade {
                give: Hand::new(1, 0, 0, 0, 0),
                get: most,
            },
            Action::AcceptTrade,
            Action::RejectTrade,
            Action::ConfirmTrade { with: 0 },
            Action::ConfirmTrade { with: 5 },
            Action::CancelTrade,
            Action::EndTurn,
            Action::ProposeTrade {
                give: most,
                get: Hand::EMPTY,
            },
        ]
    }

    #[test]
    fn every_kind_of_action_round_trips_in_the_documented_bytes() {
        let mut out = Vec::new();
        for a in every_kind() {
            let start = out.len();
            write(&a, &mut out);
            let len = out.len() - start;
            assert!(matches!(len, 1 | 2 | 3 | 11), "{a:?}: {len} bytes");
            assert!(len <= MAX_LEN);
            assert_eq!(read(&out[start..]), Ok((a, len)), "{a:?}");
        }
        // The table in docs/BOTS.md, byte for byte.
        let bytes = |a: Action| {
            let mut v = Vec::new();
            write(&a, &mut v);
            v
        };
        assert_eq!(bytes(Action::BuildRoad { edge: 0x1234 }), [1, 0x34, 0x12]);
        assert_eq!(
            bytes(Action::MoveRobber {
                hex: 7,
                victim: None
            }),
            [9, 7, 255]
        );
        assert_eq!(
            bytes(Action::ProposeTrade {
                give: Hand::new(1, 2, 0, 0, 0),
                get: Hand::new(0, 0, 0, 0, 3)
            }),
            [18, 1, 2, 0, 0, 0, 0, 0, 0, 0, 3]
        );
        assert_eq!(bytes(Action::EndTurn), [17]);
        assert_eq!(
            bytes(Action::MaritimeTrade {
                give: Resource::Ore,
                get: Resource::Wood
            }),
            [11, 4, 0]
        );
    }

    #[test]
    fn bytes_that_are_no_action_are_refused() {
        use ActionBytesError::*;
        let cases: &[(&[u8], ActionBytesError)] = &[
            (&[], Truncated),
            (&[0], Truncated),
            (&[0, 1], Truncated),
            (&[7, 0], Truncated),
            (&[9, 3], Truncated),
            (&[12, 1, 0, 0, 0, 0, 0, 0, 0, 0], Truncated),
            (&[15], Truncated),
            (&[19], UnknownTag(19)),
            (&[255], UnknownTag(255)),
            (&[7, 0, 5], Resource(5)),
            (&[8, 255], Resource(255)),
            (&[10, 5], Resource(5)),
            (&[11, 9, 0], Resource(9)),
            (&[9, 0, 6], Player(6)),
            (&[9, 0, 254], Player(254)),
            (&[15, 6], Player(6)),
            (&[15, 255], Player(255)),
            (&[12, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0], Count(20)),
            (&[18, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255], Count(255)),
        ];
        for &(bytes, want) in cases {
            assert_eq!(read(bytes), Err(want), "{bytes:?}");
        }
        // Just inside each bound.
        assert!(read(&[9, 0, 5]).is_ok());
        assert!(read(&[15, 5]).is_ok());
        assert!(read(&[8, 4]).is_ok());
        assert!(read(&[18, 19, 0, 0, 0, 0, 0, 0, 0, 0, 19]).is_ok());
        // Bytes after the action are left alone.
        assert_eq!(read(&[17, 99, 99]), Ok((Action::EndTurn, 1)));
    }
}
