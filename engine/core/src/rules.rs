//! Rule variants as data. Every setting that differs between the official rules, online
//! platforms and house rules lives here, never as a code branch elsewhere.

use serde::{Deserialize, Serialize};

use crate::board::BoardGen;

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
    pub num_players: u8,
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
    /// Official base game (6th edition rules) for 2-4 players.
    pub fn official(num_players: u8) -> GameConfig {
        assert!(
            (2..=4).contains(&num_players),
            "base board supports 2-4 players"
        );
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
                num_players: 2,
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
    pub fn catanatron_compat(num_players: u8) -> GameConfig {
        let mut c = GameConfig::official(num_players);
        c.rules.player_trading = false;
        c.rules.open_trading = false;
        c.rules.max_offers_per_turn = 0;
        c.rules.shortage = ShortageRule::NobodyGets;
        c
    }

    /// Looks up a preset by its CLI/config name.
    pub fn preset(name: &str, num_players: u8) -> Option<GameConfig> {
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
