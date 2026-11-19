//! Rule variants as data. Every setting that differs between the official rules, online
//! platforms and house rules lives here, never as a code branch elsewhere.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::board::BoardGen;
use crate::types::MAX_PLAYERS;

/// How many players a game has: 2 to 4, the counts the base board is made for. A count from
/// outside (a command line, a stored record, a request) becomes one through `TryFrom<u8>`,
/// `FromStr` or deserializing, each of which refuses any other number. It is written as the bare
/// number, as `Rules::num_players` always was.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct PlayerCount(u8);

const _: () = assert!(PlayerCount::MAX as usize <= MAX_PLAYERS);

impl PlayerCount {
    pub const MIN: u8 = 2;
    pub const MAX: u8 = 4;
    pub const TWO: PlayerCount = PlayerCount(2);
    pub const THREE: PlayerCount = PlayerCount(3);
    pub const FOUR: PlayerCount = PlayerCount(4);

    #[inline]
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// A player count outside 2 to 4, as given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerCountError(pub String);

impl fmt::Display for PlayerCountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a game has {} to {} players, not {}",
            PlayerCount::MIN,
            PlayerCount::MAX,
            self.0
        )
    }
}

impl std::error::Error for PlayerCountError {}

impl TryFrom<u8> for PlayerCount {
    type Error = PlayerCountError;

    fn try_from(n: u8) -> Result<PlayerCount, PlayerCountError> {
        if (PlayerCount::MIN..=PlayerCount::MAX).contains(&n) {
            Ok(PlayerCount(n))
        } else {
            Err(PlayerCountError(n.to_string()))
        }
    }
}

impl From<PlayerCount> for u8 {
    fn from(n: PlayerCount) -> u8 {
        n.0
    }
}

impl FromStr for PlayerCount {
    type Err = PlayerCountError;

    fn from_str(s: &str) -> Result<PlayerCount, PlayerCountError> {
        s.parse::<u8>()
            .map_err(|_| PlayerCountError(format!("{s:?}")))
            .and_then(PlayerCount::try_from)
    }
}

impl fmt::Display for PlayerCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiceMode {
    /// Two independent fair dice.
    Random,
    /// A shuffled deck of the 36 two-dice outcomes, refilled when 12 remain. Approximates the
    /// "balanced dice" of online 1v1 play; that platform's extra anti-repeat and per-player
    /// 7 balancing are not modelled.
    Balanced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevTiming {
    /// One development card per turn, at any time in your turn, including before rolling.
    Anytime,
    /// As above, but only a knight may be played before rolling.
    KnightBeforeRoll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rules {
    pub num_players: PlayerCount,
    pub vp_to_win: u8,
    /// A 7 makes every player holding more than this many cards discard half (rounded down).
    pub discard_limit: u8,
    /// The robber may not be placed next to an opponent with this many visible points or fewer.
    /// `None` disables the rule (official).
    pub friendly_robber_max_vp: Option<u8>,
    pub dice: DiceMode,
    pub dev_timing: DevTiming,
    pub player_trading: bool,
    /// Trading as the rulebook has it (5th edition, "Domestic Trade": "The other players can also
    /// make their own proposals and counteroffers"): every other player answers an offer by
    /// accepting, declining or proposing other terms, and after each roll every other player may
    /// propose a trade to the player on turn, who takes one or none. Off, only the player on turn
    /// proposes, and the others accept or decline (every version before 2026-11-12, which old
    /// records and configurations get by default).
    #[serde(default)]
    pub open_trading: bool,
    /// With `open_trading`, the round after each roll in which every other player may propose a
    /// trade to the player on turn. Off, the others propose only in answer to the player on
    /// turn's offer, as counter-offers: the owner's ruling (2026-11-13), as the online game plays.
    /// Records from 2026-11-12, which have no such field, had the round, so they get it.
    #[serde(default = "yes")]
    pub trade_proposals: bool,
    /// Engine cap on the offers the player on turn makes per turn, so bot games terminate. Not an
    /// official rule. Proposals by the others are bounded by the flow: one per player per round.
    pub max_offers_per_turn: u8,
    pub shortage: ShortageRule,
}

fn yes() -> bool {
    true
}

/// What happens when the bank can't pay everyone a resource on a roll.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortageRule {
    /// Nobody gets it, unless only one player is owed it; they take what's left (official).
    Official,
    /// Nobody gets it.
    NobodyGets,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GameConfig {
    pub rules: Rules,
    pub board: BoardGen,
}

impl GameConfig {
    /// Official base game (6th edition rules).
    pub fn official(num_players: PlayerCount) -> GameConfig {
        GameConfig {
            rules: Rules {
                num_players,
                vp_to_win: 10,
                discard_limit: 7,
                friendly_robber_max_vp: None,
                dice: DiceMode::Random,
                dev_timing: DevTiming::Anytime,
                player_trading: true,
                open_trading: true,
                trade_proposals: false,
                max_offers_per_turn: 3,
                shortage: ShortageRule::Official,
            },
            board: BoardGen::OFFICIAL,
        }
    }

    /// Online ranked 1v1 settings as documented in 2023-2024 platform posts: 15 points, friendly
    /// robber up to 2 points, 9-card discard limit, balanced dice, no player trading.
    /// Unverified against the live game as of 2026-10.
    pub fn colonist_1v1() -> GameConfig {
        GameConfig {
            rules: Rules {
                num_players: PlayerCount::TWO,
                vp_to_win: 15,
                discard_limit: 9,
                friendly_robber_max_vp: Some(2),
                dice: DiceMode::Balanced,
                dev_timing: DevTiming::Anytime,
                player_trading: false,
                open_trading: false,
                trade_proposals: false,
                max_offers_per_turn: 0,
                shortage: ShortageRule::Official,
            },
            board: BoardGen::RANDOM_BALANCED,
        }
    }

    /// Official rules adjusted to match the Catanatron engine, for differential testing only:
    /// no player trading (its bots never offer) and its bank-shortage rule.
    pub fn catanatron_compat(num_players: PlayerCount) -> GameConfig {
        let mut c = GameConfig::official(num_players);
        c.rules.player_trading = false;
        c.rules.open_trading = false;
        c.rules.max_offers_per_turn = 0;
        c.rules.shortage = ShortageRule::NobodyGets;
        c
    }

    /// Looks up a preset by its CLI/config name.
    pub fn preset(name: &str, num_players: PlayerCount) -> Option<GameConfig> {
        match name {
            "official" => Some(GameConfig::official(num_players)),
            // The official rules with the trading flow before 2026-11-12: only the player on turn
            // proposes. For the oracles that model that flow (reference model, JSettlers2, the
            // phase model) and to replay what was measured with it.
            "official-no-counters" => {
                let mut c = GameConfig::official(num_players);
                c.rules.open_trading = false;
                Some(c)
            }
            // The official rules with the trading flow of 2026-11-12: counter-offers, and a round
            // of proposals to the player on turn after each roll. To replay what was measured and
            // generated with it (positions-v5).
            "official-proposal-round" => {
                let mut c = GameConfig::official(num_players);
                c.rules.trade_proposals = true;
                Some(c)
            }
            "colonist-1v1" => Some(GameConfig::colonist_1v1()),
            "catanatron-compat" => Some(GameConfig::catanatron_compat(num_players)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::hash::{Hash, Hasher};

    use super::*;

    /// The bytes a value feeds a hasher.
    #[derive(Default)]
    struct Fed(Vec<u8>);

    impl Hasher for Fed {
        fn finish(&self) -> u64 {
            0
        }
        fn write(&mut self, bytes: &[u8]) {
            self.0.extend_from_slice(bytes);
        }
    }

    /// Issue #23: the count was checked as 2 to 4 by the rules, 2 to 6 by the state and the
    /// command line, and not at all in a stored record. Every way a count comes in now takes 2
    /// to 4 and refuses the rest: every byte, as text and as JSON, and the other shapes JSON has.
    #[test]
    fn a_player_count_is_2_to_4_however_it_comes_in() {
        for n in 0..=u8::MAX {
            let fits = (2..=4).contains(&n);
            let by_number = PlayerCount::try_from(n);
            let by_text = n.to_string().parse::<PlayerCount>();
            let by_json = serde_json::from_value::<PlayerCount>(n.into());
            assert_eq!(by_number.is_ok(), fits, "{n}");
            assert_eq!(by_text, by_number, "{n}");
            assert_eq!(by_json.ok(), by_number.clone().ok(), "{n}");
            if let Ok(count) = by_number {
                assert_eq!(count.get(), n);
                // Written as the bare number, read back from what is written.
                assert_eq!(count.to_string().parse(), Ok(count));
                assert_eq!(serde_json::to_string(&count).unwrap(), n.to_string());
                let (mut a, mut b) = (Fed::default(), Fed::default());
                count.hash(&mut a);
                n.hash(&mut b);
                assert_eq!(a.0, b.0, "{n} feeds a hasher what the u8 did");
            } else {
                let err = by_number.unwrap_err().to_string();
                assert_eq!(err, format!("a game has 2 to 4 players, not {n}"));
            }
        }
        for text in [
            "", " 4", "4 ", "4.0", "-1", "256", "1e1", "four", "0x4", "\u{664}",
        ] {
            let err = text.parse::<PlayerCount>().unwrap_err().to_string();
            assert!(
                err.starts_with("a game has 2 to 4 players"),
                "{text:?}: {err}"
            );
        }
        for json in [
            "-1", "256", "65536", "4.0", "\"4\"", "null", "[4]", "{}", "true",
        ] {
            assert!(serde_json::from_str::<PlayerCount>(json).is_err(), "{json}");
        }
        assert_eq!(
            [PlayerCount::TWO, PlayerCount::THREE, PlayerCount::FOUR].map(PlayerCount::get),
            [PlayerCount::MIN, 3, PlayerCount::MAX]
        );
    }

    /// The presets take every count, and the ones with a fixed count keep it.
    #[test]
    fn every_preset_takes_every_player_count() {
        let names = [
            "official",
            "official-no-counters",
            "official-proposal-round",
            "colonist-1v1",
            "catanatron-compat",
        ];
        for name in names {
            for n in [PlayerCount::TWO, PlayerCount::THREE, PlayerCount::FOUR] {
                let c = GameConfig::preset(name, n).unwrap();
                let want = if name == "colonist-1v1" {
                    PlayerCount::TWO
                } else {
                    n
                };
                assert_eq!(c.rules.num_players, want, "{name}");
            }
        }
    }
}
