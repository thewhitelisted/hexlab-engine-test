//! Random playouts, used for fuzzing, benchmarks and as the simplest rollout policy.

use crate::actions::Action;
use crate::legal::OfferSpace;
use crate::rng::Rng;
use crate::state::{GameState, Phase};
use crate::types::{NUM_RESOURCES, PlayerId, Resource, cost};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RandomPolicy {
    /// Uniform over legal actions. Best rules coverage; games are long.
    Uniform,
    /// Random, but builds whenever it can and rarely trades. Games finish in a realistic number
    /// of turns.
    Builder,
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Stop (no winner) after this many turns.
    pub max_turns: u32,
    /// After this many actions in one turn, end the turn as soon as that is legal.
    pub max_actions_per_turn: u32,
    /// Player-to-player offers the random players consider.
    pub offers: OfferSpace,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_turns: 1000,
            max_actions_per_turn: 40,
            offers: OfferSpace::Simple,
        }
    }
}

/// Tracks actions per turn so random players can't stall a game.
#[derive(Clone, Copy, Debug, Default)]
pub struct TurnGuard {
    turn: u32,
    actions: u32,
}

impl TurnGuard {
    /// Picks an action for the acting player from `legal` (which must not be empty).
    pub fn choose(
        &mut self,
        policy: RandomPolicy,
        limits: &Limits,
        state: &GameState,
        legal: &[Action],
        rng: &mut Rng,
    ) -> Action {
        if state.turn != self.turn {
            self.turn = state.turn;
            self.actions = 0;
        }
        self.actions += 1;
        if self.actions > limits.max_actions_per_turn
            && let Some(&end) = legal.iter().find(|a| {
                matches!(
                    a,
                    Action::EndTurn | Action::CancelTrade | Action::RejectTrade
                )
            })
        {
            return end;
        }
        match policy {
            RandomPolicy::Uniform => legal[rng.below(legal.len() as u32) as usize],
            RandomPolicy::Builder => builder_choice(state, legal, rng),
        }
    }
}

fn pick_where(legal: &[Action], rng: &mut Rng, f: impl Fn(&Action) -> bool) -> Option<Action> {
    let count = legal.iter().filter(|a| f(a)).count();
    if count == 0 {
        return None;
    }
    let k = rng.below(count as u32) as usize;
    legal.iter().filter(|a| f(a)).nth(k).copied()
}

fn builder_choice(state: &GameState, legal: &[Action], rng: &mut Rng) -> Action {
    use Action::*;
    let uniform = |rng: &mut Rng| legal[rng.below(legal.len() as u32) as usize];
    match state.phase {
        Phase::PreRoll => {
            if rng.unit() < 0.2
                && let Some(a) = pick_where(legal, rng, |a| *a != RollDice)
            {
                return a;
            }
            RollDice
        }
        Phase::Main => {
            let mut counts = [0u32; Category::COUNT];
            for a in legal {
                if let Some(c) = Category::of(a) {
                    counts[c as usize] += 1;
                }
            }
            let pick = |c: Category, rng: &mut Rng| {
                let k = rng.below(counts[c as usize]) as usize;
                *legal
                    .iter()
                    .filter(|a| Category::of(a) == Some(c))
                    .nth(k)
                    .expect("counted")
            };
            let has = |c: Category| counts[c as usize] > 0;
            if has(Category::City) {
                return pick(Category::City, rng);
            }
            if has(Category::Settlement) {
                return pick(Category::Settlement, rng);
            }
            if has(Category::Maritime)
                && rng.unit() < 0.9
                && let Some(a) = trade_toward_build(state, legal, rng)
            {
                return a;
            }
            for (c, p) in [
                (Category::RoadOrDev, 0.6),
                (Category::DevPlay, 0.3),
                (Category::Maritime, 0.2),
                (Category::Offer, 0.05),
            ] {
                if has(c) && rng.unit() < p {
                    return pick(c, rng);
                }
            }
            EndTurn
        }
        _ => uniform(rng),
    }
}

/// Main-phase action groups the builder policy chooses between.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    City,
    Settlement,
    RoadOrDev,
    DevPlay,
    Maritime,
    Offer,
}

impl Category {
    const COUNT: usize = 6;

    #[inline]
    fn of(a: &Action) -> Option<Category> {
        use Action::*;
        Some(match a {
            BuildCity { .. } => Category::City,
            BuildSettlement { .. } => Category::Settlement,
            BuildRoad { .. } | BuyDevCard => Category::RoadOrDev,
            PlayKnight | PlayRoadBuilding | PlayYearOfPlenty { .. } | PlayMonopoly { .. } => {
                Category::DevPlay
            }
            MaritimeTrade { .. } => Category::Maritime,
            OfferTrade { .. } => Category::Offer,
            _ => return None,
        })
    }
}

/// A bank trade that brings the most valuable reachable build closer: gives a surplus resource
/// for one the build is missing, when the surplus can cover the whole shortfall.
fn trade_toward_build(state: &GameState, legal: &[Action], rng: &mut Rng) -> Option<Action> {
    let p = state.current;
    let ps = state.player(p);
    let targets = [
        (
            cost::CITY,
            ps.cities_left > 0 && ps.settlements_on_board() > 0,
        ),
        (
            cost::SETTLEMENT,
            ps.settlements_left > 0 && !state.settlement_spots(p).is_empty(),
        ),
        (cost::DEV_CARD, !state.dev_deck.is_empty()),
        (cost::ROAD, ps.roads_left > 0),
    ];
    for (target, possible) in targets {
        if !possible {
            continue;
        }
        let mut short = [0u8; NUM_RESOURCES];
        let mut surplus_trades = 0u32;
        for r in Resource::ALL {
            let (have, need) = (ps.hand.get(r), target.get(r));
            short[r.index()] = need.saturating_sub(have);
            surplus_trades += (have.saturating_sub(need) / ps.trade_ratio(r)) as u32;
        }
        let short_total: u32 = short.iter().map(|&s| s as u32).sum();
        if short_total == 0 || surplus_trades < short_total {
            continue;
        }
        return pick_where(legal, rng, |a| match *a {
            Action::MaritimeTrade { give, get } => {
                short[get.index()] > 0
                    && ps.hand.get(give).saturating_sub(target.get(give)) >= ps.trade_ratio(give)
            }
            _ => false,
        });
    }
    None
}

/// Plays `state` to the end with a random policy. Returns the winner, or `None` if the turn
/// limit was hit.
pub fn play_out(
    state: &mut GameState,
    policy: RandomPolicy,
    limits: &Limits,
    rng: &mut Rng,
    buf: &mut Vec<Action>,
) -> Option<PlayerId> {
    let mut guard = TurnGuard::default();
    while !state.is_over() && state.turn <= limits.max_turns {
        state.legal_actions_with(limits.offers, buf);
        let action = guard.choose(policy, limits, state, buf, rng);
        state.apply_legal(action);
    }
    state.winner
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::GameConfig;

    #[test]
    fn play_out_ends_with_a_winner_or_at_the_turn_limit() {
        let mut buf = Vec::new();
        let mut s = GameState::new(&GameConfig::official(4), 5);
        let limits = Limits::default();
        let w = play_out(
            &mut s,
            RandomPolicy::Builder,
            &limits,
            &mut Rng::seed(1),
            &mut buf,
        );
        assert!(s.is_over());
        assert_eq!(w, s.winner);
        assert!(s.total_vp(w.unwrap()) >= s.rules.vp_to_win);

        let mut s = GameState::new(&GameConfig::official(4), 5);
        let limits = Limits {
            max_turns: 3,
            ..Limits::default()
        };
        let w = play_out(
            &mut s,
            RandomPolicy::Uniform,
            &limits,
            &mut Rng::seed(1),
            &mut buf,
        );
        assert_eq!((w, s.turn), (None, 4));
    }

    #[test]
    fn the_guard_ends_a_turn_that_runs_too_long() {
        let s = GameState::new(&GameConfig::official(2), 5);
        let limits = Limits {
            max_actions_per_turn: 2,
            ..Limits::default()
        };
        let (mut guard, mut rng) = (TurnGuard::default(), Rng::seed(3));
        let legal = [Action::BuyDevCard, Action::EndTurn];
        let picks: Vec<Action> = (0..12)
            .map(|_| guard.choose(RandomPolicy::Uniform, &limits, &s, &legal, &mut rng))
            .collect();
        assert!(
            picks[2..].iter().all(|&a| a == Action::EndTurn),
            "{picks:?}"
        );
        // With nothing that ends the turn on offer, it keeps choosing.
        let only = [Action::RollDice];
        let a = guard.choose(RandomPolicy::Uniform, &limits, &s, &only, &mut rng);
        assert_eq!(a, Action::RollDice);
    }
}
