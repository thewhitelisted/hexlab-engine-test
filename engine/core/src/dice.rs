//! Dice sources for each [`DiceMode`].

use serde::{Deserialize, Serialize};

use crate::rng::Rng;
use crate::rules::DiceMode;

/// The balanced deck is refilled once this many outcomes remain.
const BALANCED_REFILL_AT: usize = 12;

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Dice {
    deck: Vec<(u8, u8)>,
}

/// The 36 outcomes of two dice, in the order a fresh balanced deck holds them.
fn full_deck() -> impl Iterator<Item = (u8, u8)> {
    (1..=6u8).flat_map(|a| (1..=6u8).map(move |b| (a, b)))
}

impl Dice {
    /// Refills the balanced deck if the next roll is due a fresh one.
    fn refill_if_due(&mut self) {
        if self.deck.len() <= BALANCED_REFILL_AT {
            self.deck.clear();
            self.deck.extend(full_deck());
        }
    }

    pub fn roll(&mut self, mode: DiceMode, rng: &mut Rng) -> (u8, u8) {
        match mode {
            DiceMode::Random => (rng.die(), rng.die()),
            DiceMode::Balanced => {
                self.refill_if_due();
                let i = rng.below(self.deck.len() as u32) as usize;
                self.deck.swap_remove(i)
            }
        }
    }

    /// Whether the next roll can come up `(a, b)`: any two dice with random dice, an outcome
    /// still in the deck (a fresh one, if the next roll refills it) with balanced dice.
    pub fn can_roll(&self, mode: DiceMode, a: u8, b: u8) -> bool {
        if !(1..=6).contains(&a) || !(1..=6).contains(&b) {
            return false;
        }
        match mode {
            DiceMode::Random => true,
            DiceMode::Balanced => {
                self.deck.len() <= BALANCED_REFILL_AT || self.deck.contains(&(a, b))
            }
        }
    }

    /// The roll `(a, b)` as a given outcome (a replayed game, a line of analysis): with balanced
    /// dice it leaves the deck exactly as `roll` leaves it when it deals that outcome, so the
    /// deck after a replay is the deck of the game. False, and nothing changes, if the next
    /// roll can't come up `(a, b)` (`can_roll`).
    pub fn take(&mut self, mode: DiceMode, a: u8, b: u8) -> bool {
        if !self.can_roll(mode, a, b) {
            return false;
        }
        if mode == DiceMode::Balanced {
            self.refill_if_due();
            match self.deck.iter().position(|&o| o == (a, b)) {
                Some(i) => {
                    self.deck.swap_remove(i);
                }
                None => return false,
            }
        }
        true
    }

    /// The chance that the next roll adds up to `sum`, and one outcome that does (`None` if none
    /// can). Random dice: the 36 outcomes. Balanced dice: the outcomes left in the deck.
    pub fn next_sum(&self, mode: DiceMode, sum: u8) -> (f64, Option<(u8, u8)>) {
        if !(2..=12).contains(&sum) {
            return (0.0, None);
        }
        let fresh = mode == DiceMode::Random || self.deck.len() <= BALANCED_REFILL_AT;
        if fresh {
            let a = sum.saturating_sub(6).max(1);
            let ways = 6 - sum.abs_diff(7);
            (f64::from(ways) / 36.0, Some((a, sum - a)))
        } else {
            let mut with_sum = self.deck.iter().filter(|(a, b)| a + b == sum);
            let first = with_sum.next().copied();
            let ways = with_sum.count() + usize::from(first.is_some());
            (ways as f64 / self.deck.len() as f64, first)
        }
    }

    /// The chance that at least one of the next `rolls` rolls is a 7. The balanced deck is
    /// public: every roll it dealt was seen.
    pub fn seven_within(&self, mode: DiceMode, rolls: u32) -> f64 {
        let mut none = 1.0;
        match mode {
            DiceMode::Random => {
                for _ in 0..rolls {
                    none *= 5.0 / 6.0;
                }
            }
            DiceMode::Balanced => {
                // Only how many outcomes are left, and how many of them are 7s, matters; with no 7
                // dealt, the 7s stay and one other outcome goes.
                let mut left = self.deck.len();
                let mut sevens = self.deck.iter().filter(|(a, b)| a + b == 7).count();
                for _ in 0..rolls {
                    if left <= BALANCED_REFILL_AT {
                        (left, sevens) = (36, 6);
                    }
                    none *= (left - sevens) as f64 / left as f64;
                    left -= 1;
                }
            }
        }
        1.0 - none
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_dice_follow_the_deck_distribution() {
        let mut dice = Dice::default();
        let mut rng = Rng::seed(9);
        let mut sums = [0u32; 13];
        for _ in 0..24 * 1000 {
            let (a, b) = dice.roll(DiceMode::Balanced, &mut rng);
            sums[(a + b) as usize] += 1;
        }
        // Every deck cycle deals 24 of the 36 outcomes, so frequencies match the dice odds.
        let sevens = sums[7] as f64 / 24_000.0;
        assert!((sevens - 6.0 / 36.0).abs() < 0.01, "{sevens}");
    }

    /// Audit 2.8: a roll given from outside (a replayed game, an analysis line) never came out
    /// of the balanced deck, so the deck after a replay was not the game's and the chance of a 7
    /// shown from it was the fresh deck's.
    #[test]
    fn a_given_roll_comes_out_of_the_balanced_deck() {
        let mut rng = Rng::seed(7);
        let mut dealt = Dice::default();
        let mut given = Dice::default();
        // 100 rolls cross four refills: taking what was dealt keeps the two decks the same,
        // in the same order.
        for i in 0..100 {
            let (a, b) = dealt.roll(DiceMode::Balanced, &mut rng);
            assert!(given.can_roll(DiceMode::Balanced, a, b), "roll {i}");
            assert!(given.take(DiceMode::Balanced, a, b), "roll {i}");
            assert_eq!(given, dealt, "roll {i}");
            for rolls in 0..4 {
                assert_eq!(
                    given.seven_within(DiceMode::Balanced, rolls),
                    dealt.seven_within(DiceMode::Balanced, rolls)
                );
            }
        }
        // An outcome that has gone can't be given again until the deck is refilled.
        let mut dice = Dice::default();
        assert!(dice.take(DiceMode::Balanced, 3, 4));
        assert_eq!(dice.deck.len(), 35);
        let before = dice.clone();
        assert!(!dice.can_roll(DiceMode::Balanced, 3, 4));
        assert!(!dice.take(DiceMode::Balanced, 3, 4));
        assert_eq!(dice, before, "a refused roll changes nothing");
        assert!(
            dice.can_roll(DiceMode::Balanced, 4, 3),
            "the other 7s are still there"
        );
        // Not dice at all, in either mode.
        for (a, b) in [(0, 1), (1, 0), (7, 1), (1, 7), (0, 0), (255, 255), (6, 250)] {
            for mode in [DiceMode::Balanced, DiceMode::Random] {
                assert!(!dice.can_roll(mode, a, b), "{a} {b}");
                assert!(!dice.take(mode, a, b), "{a} {b}");
            }
        }
        assert_eq!(dice, before);
        // Random dice have no deck: anything can be given, any number of times.
        let mut random = Dice::default();
        for _ in 0..50 {
            assert!(random.take(DiceMode::Random, 3, 4));
        }
        assert_eq!(random, Dice::default());
        // Once 12 are left the next roll is from a fresh deck, so anything can be given.
        let mut dice = Dice::default();
        for (a, b) in full_deck().take(24) {
            assert!(dice.take(DiceMode::Balanced, a, b));
        }
        assert_eq!(dice.deck.len(), 12);
        assert!(dice.take(DiceMode::Balanced, 1, 1));
        assert_eq!(dice.deck.len(), 35);
    }

    /// In a game: a roll given twice from one balanced deck is refused, and the game goes on
    /// unchanged; with random dice any roll can be given.
    #[test]
    fn a_game_refuses_a_given_roll_the_deck_has_dealt() {
        use crate::{Action, Forced, GameConfig, GameState, RuleError};
        let to_the_next_roll = |s: &mut GameState| {
            let mut legal = Vec::new();
            let mut rng = Rng::seed(1);
            while s.phase != crate::state::Phase::PreRoll || s.turn == 0 {
                s.legal_actions(&mut legal);
                let a = legal
                    .iter()
                    .copied()
                    .find(|a| *a == Action::EndTurn)
                    .unwrap_or(legal[rng.below(legal.len() as u32) as usize]);
                s.apply(a).unwrap();
            }
        };
        for (config, repeat_ok) in [
            (GameConfig::colonist_1v1(), false),
            (GameConfig::official(4), true),
        ] {
            let mut s = GameState::new(&config, 3);
            to_the_next_roll(&mut s);
            s.apply_forced(Action::RollDice, Some(Forced::Dice(2, 3)))
                .unwrap();
            assert_eq!(
                s.dice.deck.len(),
                if repeat_ok { 0 } else { 35 },
                "the roll came out of the deck"
            );
            to_the_next_roll(&mut s);
            let before = s.clone();
            let again = s.apply_forced(Action::RollDice, Some(Forced::Dice(2, 3)));
            if repeat_ok {
                assert!(again.is_ok());
            } else {
                assert_eq!(again, Err(RuleError::InvalidForcedOutcome));
                assert_eq!(s.position_hash(), before.position_hash());
                assert_eq!(s.dice, before.dice);
                s.apply_forced(Action::RollDice, Some(Forced::Dice(3, 2)))
                    .unwrap();
                assert_eq!(s.dice.deck.len(), 34);
            }
            assert!(
                s.clone()
                    .apply_forced(Action::RollDice, Some(Forced::Dice(0, 7)))
                    .is_err()
            );
        }
    }

    #[test]
    fn the_next_sum_follows_the_dice() {
        let ways = [0u8, 0, 1, 2, 3, 4, 5, 6, 5, 4, 3, 2, 1];
        let fresh = Dice::default();
        for mode in [DiceMode::Random, DiceMode::Balanced] {
            let mut total = 0.0;
            for sum in 0..=14u8 {
                let (p, one) = fresh.next_sum(mode, sum);
                if (2..=12).contains(&sum) {
                    assert_eq!(p, f64::from(ways[sum as usize]) / 36.0);
                    let (a, b) = one.unwrap();
                    assert_eq!((a + b, a), (sum, sum.saturating_sub(6).max(1)));
                    assert!(fresh.can_roll(mode, a, b));
                } else {
                    assert_eq!((p, one), (0.0, None));
                }
                total += p;
            }
            assert!((total - 1.0).abs() < 1e-12);
        }
        // A deck with both 2 and 12 gone, and one 7.
        let mut dice = Dice::default();
        for (a, b) in [(1, 1), (6, 6), (1, 6), (2, 5), (3, 4), (4, 3), (5, 2)] {
            assert!(dice.take(DiceMode::Balanced, a, b));
        }
        assert_eq!(dice.next_sum(DiceMode::Balanced, 2), (0.0, None));
        assert_eq!(dice.next_sum(DiceMode::Balanced, 12), (0.0, None));
        assert_eq!(
            dice.next_sum(DiceMode::Balanced, 7),
            (1.0 / 29.0, Some((6, 1)))
        );
        let total: f64 = (2..=12)
            .map(|s| dice.next_sum(DiceMode::Balanced, s).0)
            .sum();
        assert!((total - 1.0).abs() < 1e-12);
        for sum in 2..=12 {
            if let (_, Some((a, b))) = dice.next_sum(DiceMode::Balanced, sum) {
                assert!(dice.can_roll(DiceMode::Balanced, a, b));
            }
        }
        // The same deck with random dice is ignored.
        assert_eq!(
            dice.next_sum(DiceMode::Random, 2),
            (1.0 / 36.0, Some((1, 1)))
        );
    }

    #[test]
    fn the_chance_of_a_seven_follows_the_dice() {
        let fresh = Dice::default();
        assert_eq!(fresh.seven_within(DiceMode::Random, 0), 0.0);
        let three = 1.0 - (5.0f64 / 6.0).powi(3);
        assert!((fresh.seven_within(DiceMode::Random, 3) - three).abs() < 1e-12);
        // A fresh deck is filled before its first roll: the odds of the dice, without replacement.
        let deck = 1.0 - (30.0 / 36.0) * (29.0 / 35.0) * (28.0 / 34.0);
        assert!((fresh.seven_within(DiceMode::Balanced, 3) - deck).abs() < 1e-12);

        // Against what the deck then deals.
        let mut rng = Rng::seed(4);
        for _ in 0..30 {
            let mut dice = Dice::default();
            for _ in 0..rng.below(40) {
                dice.roll(DiceMode::Balanced, &mut rng);
            }
            let expected = dice.seven_within(DiceMode::Balanced, 2);
            let tries = 20_000;
            let hits = (0..tries)
                .filter(|_| {
                    let mut d = dice.clone();
                    (0..2).any(|_| {
                        let (a, b) = d.roll(DiceMode::Balanced, &mut rng);
                        a + b == 7
                    })
                })
                .count();
            let seen = hits as f64 / tries as f64;
            assert!((seen - expected).abs() < 0.015, "{seen} vs {expected}");
        }
        // A deck without 7s left, and not about to be refilled.
        let dice = Dice {
            deck: (1..=6)
                .flat_map(|a| (1..=6).map(move |b| (a, b)))
                .filter(|(a, b)| a + b != 7)
                .collect(),
        };
        assert_eq!(dice.seven_within(DiceMode::Balanced, 3), 0.0);
    }
}
