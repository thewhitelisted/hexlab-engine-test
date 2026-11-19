//! Exact Bayesian tracking of hidden resource hands from one player's point of view.
//!
//! Almost every card movement in the game is public. The exceptions are steals the observer
//! isn't party to; each one branches the joint hypothesis over everyone's hands, and later
//! spends, monopolies and accepted trades prune inconsistent branches.

use std::collections::HashMap;

use crate::actions::{Action, Outcome};
use crate::rng::Rng;
use crate::state::{GameState, Phase};
use crate::types::*;

/// Everyone's hand in one hypothesis.
pub type Hands = [Hand; MAX_PLAYERS];

#[derive(Clone, Debug)]
pub struct BeliefTracker {
    observer: PlayerId,
    num_players: usize,
    hyps: Vec<(Hands, f64)>,
    /// Above this many hypotheses the least likely are dropped and tracking becomes approximate.
    pub max_hypotheses: usize,
    approximate: bool,
}

impl BeliefTracker {
    pub fn new(observer: PlayerId, num_players: usize) -> BeliefTracker {
        BeliefTracker {
            observer,
            num_players,
            hyps: vec![([Hand::EMPTY; MAX_PLAYERS], 1.0)],
            max_hypotheses: 4096,
            approximate: false,
        }
    }

    /// A tracker for a position where every hand is known (e.g. typed into the analysis board).
    pub fn with_hands(observer: PlayerId, num_players: usize, hands: Hands) -> BeliefTracker {
        BeliefTracker {
            hyps: vec![(hands, 1.0)],
            ..BeliefTracker::new(observer, num_players)
        }
    }

    /// A tracker for a position typed in with the others' hands known only by their sizes:
    /// `samples` are dealings of the cards the observer can't see, equally likely. It counts as
    /// approximate, so [`View::determinize`](crate::View::determinize) deals each world afresh
    /// rather than drawing one of the samples, which only give the expected hands.
    pub fn from_samples(
        observer: PlayerId,
        num_players: usize,
        samples: Vec<Hands>,
    ) -> BeliefTracker {
        assert!(!samples.is_empty(), "at least one dealing");
        let w = 1.0 / samples.len() as f64;
        BeliefTracker {
            hyps: samples.into_iter().map(|h| (h, w)).collect(),
            approximate: true,
            ..BeliefTracker::new(observer, num_players)
        }
    }

    pub fn observer(&self) -> PlayerId {
        self.observer
    }

    pub fn num_hypotheses(&self) -> usize {
        self.hyps.len()
    }

    /// True once hypotheses had to be pruned by weight or an impossible observation was seen.
    pub fn is_approximate(&self) -> bool {
        self.approximate
    }

    pub fn hypotheses(&self) -> &[(Hands, f64)] {
        &self.hyps
    }

    /// Updates from one action. `pre` is the state before the action (only its public parts are
    /// read), `actor` the player who took it, and `outcome` as seen by the observer
    /// (see [`Outcome::visible_to`]).
    pub fn observe(
        &mut self,
        pre: &GameState,
        actor: PlayerId,
        action: &Action,
        outcome: &Outcome,
    ) {
        let a = actor as usize;
        match *action {
            Action::BuildSettlement { vertex } => {
                if pre.phase == Phase::SetupSettlement {
                    if pre.setup_step >= pre.rules.num_players.get() {
                        self.gain(a, &pre.setup_gains(vertex));
                    }
                } else {
                    self.spend(a, &cost::SETTLEMENT);
                }
            }
            Action::BuildRoad { .. } if pre.phase == Phase::Main => self.spend(a, &cost::ROAD),
            Action::BuildCity { .. } => self.spend(a, &cost::CITY),
            Action::BuyDevCard => self.spend(a, &cost::DEV_CARD),
            Action::RollDice => {
                if let Outcome::Dice { a: d1, b: d2 } = *outcome
                    && d1 + d2 != 7
                {
                    let gains = pre.production_gains(d1 + d2);
                    for (q, g) in gains.iter().enumerate().take(self.num_players) {
                        self.gain(q, g);
                    }
                }
            }
            Action::PlayYearOfPlenty { a: r1, b: r2 } => {
                let mut h = Hand::of(r1, 1);
                h.add_n(r2, 1);
                self.gain(a, &h);
            }
            Action::PlayMonopoly { resource } => {
                if let Outcome::Monopoly { taken } = *outcome {
                    for q in (0..self.num_players).filter(|&q| q != a) {
                        let n = taken[q];
                        self.condition(|h| h[q].get(resource) == n);
                        self.transfer(q, a, &Hand::of(resource, n));
                    }
                }
            }
            Action::MoveRobber {
                victim: Some(v), ..
            } => match *outcome {
                Outcome::Stole { resource: Some(r) } => {
                    self.transfer(v as usize, a, &Hand::of(r, 1))
                }
                Outcome::Stole { resource: None } => self.steal_unseen(v as usize, a),
                _ => {}
            },
            Action::Discard { resource } => self.spend(a, &Hand::of(resource, 1)),
            Action::MaritimeTrade { give, get } => {
                let ratio = pre.players[a].trade_ratio(give);
                self.spend(a, &Hand::of(give, ratio));
                self.gain(a, &Hand::of(get, 1));
            }
            Action::AcceptTrade => {
                if let Some(t) = pre.trade {
                    self.condition(|h| h[a].contains(&t.get));
                }
            }
            Action::ProposeTrade { give, .. } => self.condition(|h| h[a].contains(&give)),
            Action::ConfirmTrade { with } => {
                if let Some((give, get)) = pre.trade.and_then(|t| t.terms(with)) {
                    self.transfer(a, with as usize, &give);
                    self.transfer(with as usize, a, &get);
                }
            }
            _ => {}
        }
    }

    /// Expected count of each resource in `q`'s hand.
    pub fn expected_hand(&self, q: PlayerId) -> [f64; NUM_RESOURCES] {
        let mut out = [0.0; NUM_RESOURCES];
        for (hands, w) in &self.hyps {
            for (i, o) in out.iter_mut().enumerate() {
                *o += w * hands[q as usize].0[i] as f64;
            }
        }
        out
    }

    /// Marginal distribution over `q`'s hand, most likely first.
    pub fn hand_distribution(&self, q: PlayerId) -> Vec<(Hand, f64)> {
        let mut m: HashMap<Hand, f64> = HashMap::new();
        for (hands, w) in &self.hyps {
            *m.entry(hands[q as usize]).or_default() += w;
        }
        let mut out: Vec<_> = m.into_iter().collect();
        out.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
        out
    }

    /// Probability that `q` holds at least `cards`.
    pub fn prob_has(&self, q: PlayerId, cards: &Hand) -> f64 {
        self.hyps
            .iter()
            .filter(|(hands, _)| hands[q as usize].contains(cards))
            .map(|(_, w)| w)
            .sum()
    }

    /// Draws one joint hypothesis in proportion to its probability (for determinized search).
    pub fn sample(&self, rng: &mut Rng) -> Hands {
        let mut x = rng.unit();
        for (hands, w) in &self.hyps {
            if x < *w {
                return *hands;
            }
            x -= w;
        }
        self.hyps.last().expect("at least one hypothesis").0
    }

    fn gain(&mut self, q: usize, h: &Hand) {
        if h.is_empty() {
            return;
        }
        for (hands, _) in &mut self.hyps {
            hands[q].add(h);
        }
    }

    fn spend(&mut self, q: usize, h: &Hand) {
        self.condition(|hands| hands[q].contains(h));
        for (hands, _) in &mut self.hyps {
            for i in 0..NUM_RESOURCES {
                hands[q].0[i] = hands[q].0[i].saturating_sub(h.0[i]);
            }
        }
    }

    fn transfer(&mut self, from: usize, to: usize, h: &Hand) {
        self.spend(from, h);
        self.gain(to, h);
    }

    /// Keeps hypotheses satisfying `pred`. If none do (only possible once approximate), keeps
    /// everything and marks the tracker approximate.
    fn condition(&mut self, pred: impl Fn(&Hands) -> bool) {
        let before = self.hyps.len();
        let kept: Vec<_> = self.hyps.iter().filter(|(h, _)| pred(h)).copied().collect();
        if kept.is_empty() {
            self.approximate = true;
            return;
        }
        if kept.len() != before {
            self.hyps = kept;
            self.normalize();
        }
    }

    fn steal_unseen(&mut self, victim: usize, thief: usize) {
        let mut merged: HashMap<Hands, f64> = HashMap::with_capacity(self.hyps.len() * 3);
        for (hands, w) in &self.hyps {
            let total = hands[victim].total();
            if total == 0 {
                continue;
            }
            for r in Resource::ALL {
                let c = hands[victim].get(r);
                if c == 0 {
                    continue;
                }
                let mut next = *hands;
                next[victim].sub_n(r, 1);
                next[thief].add_n(r, 1);
                *merged.entry(next).or_default() += w * c as f64 / total as f64;
            }
        }
        if merged.is_empty() {
            self.approximate = true;
            return;
        }
        let mut hyps: Vec<_> = merged.into_iter().collect();
        if hyps.len() > self.max_hypotheses {
            hyps.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
            hyps.truncate(self.max_hypotheses);
            self.approximate = true;
        } else {
            // HashMap order is random; keep results reproducible.
            hyps.sort_by_key(|h| h.0);
        }
        self.hyps = hyps;
        self.normalize();
    }

    fn normalize(&mut self) {
        let total: f64 = self.hyps.iter().map(|(_, w)| w).sum();
        for (_, w) in &mut self.hyps {
            *w /= total;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::GameConfig;
    use crate::rules::PlayerCount;
    use crate::state::PendingTrade;

    /// Observer 0 watches player 2 rob player 1, who held `victim`; everyone else holds `others`.
    fn after_unseen_steal(victim: Hand, others: Hand) -> (GameState, BeliefTracker) {
        let pre = GameState::new(&GameConfig::official(PlayerCount::THREE), 1);
        let hands = std::array::from_fn(|q| match q {
            1 => victim,
            0 | 2 => others,
            _ => Hand::EMPTY,
        });
        let mut b = BeliefTracker::with_hands(0, 3, hands);
        let rob = Action::MoveRobber {
            hex: 0,
            victim: Some(1),
        };
        b.observe(&pre, 2, &rob, &Outcome::Stole { resource: None });
        (pre, b)
    }

    fn close(x: f64, y: f64) -> bool {
        (x - y).abs() < 1e-12
    }

    #[test]
    fn unseen_steals_weigh_each_card_by_how_many_the_victim_held() {
        use Resource::*;
        let (_, b) = after_unseen_steal(Hand::new(2, 1, 0, 0, 0), Hand::EMPTY);
        assert_eq!(b.num_hypotheses(), 2);
        assert!(!b.is_approximate());
        let thief = b.expected_hand(2);
        let victim = b.expected_hand(1);
        let third = 1.0 / 3.0;
        for (got, want) in [
            (thief, [2.0 * third, third, 0.0, 0.0, 0.0]),
            (victim, [4.0 * third, 2.0 * third, 0.0, 0.0, 0.0]),
        ] {
            assert!(got.iter().zip(want).all(|(&g, w)| close(g, w)), "{got:?}");
        }
        assert!(close(b.prob_has(2, &Hand::of(Wood, 1)), 2.0 * third));
        assert!(close(b.prob_has(2, &Hand::of(Brick, 1)), third));
        assert!(close(b.prob_has(1, &Hand::of(Wood, 2)), third));
        assert!(close(b.prob_has(0, &Hand::of(Wood, 1)), 0.0));

        // A victim holding one kind of card loses it for certain.
        let (_, b) = after_unseen_steal(Hand::new(0, 0, 3, 0, 0), Hand::EMPTY);
        assert_eq!(b.num_hypotheses(), 1);
        assert_eq!(b.expected_hand(2), [0.0, 0.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn samples_of_a_typed_in_position_weigh_equally_and_count_as_approximate() {
        let mine = Hand::new(1, 0, 0, 0, 0);
        let a: Hands = std::array::from_fn(|q| match q {
            0 => mine,
            1 => Hand::new(2, 0, 0, 0, 0),
            2 => Hand::new(0, 2, 0, 0, 0),
            _ => Hand::EMPTY,
        });
        let b: Hands = std::array::from_fn(|q| match q {
            0 => mine,
            1 => Hand::new(0, 2, 0, 0, 0),
            2 => Hand::new(2, 0, 0, 0, 0),
            _ => Hand::EMPTY,
        });
        let t = BeliefTracker::from_samples(0, 3, vec![a, a, b, a]);
        assert!(t.is_approximate());
        assert_eq!((t.observer(), t.num_hypotheses()), (0, 4));
        assert!(t.hypotheses().iter().all(|h| close(h.1, 0.25)));
        // Player 1's wood: 2 in three dealings of four.
        assert!(close(t.expected_hand(1)[0], 1.5));
        assert!(close(t.expected_hand(1)[1], 0.5));
        assert!(close(t.expected_hand(0)[0], 1.0));
    }

    #[test]
    fn sampling_follows_the_weights() {
        let (_, b) = after_unseen_steal(Hand::new(1, 1, 2, 0, 0), Hand::EMPTY);
        let hyps = b.hypotheses().to_vec();
        assert_eq!(hyps.len(), 3);
        let mut counts = [0u32; 3];
        let mut rng = Rng::seed(3);
        let n = 30_000;
        for _ in 0..n {
            let h = b.sample(&mut rng);
            counts[hyps.iter().position(|(x, _)| *x == h).unwrap()] += 1;
        }
        for (i, (_, w)) in hyps.iter().enumerate() {
            let f = counts[i] as f64 / n as f64;
            assert!((f - w).abs() < 0.015, "hypothesis {i}: {f} against {w}");
        }
    }

    #[test]
    fn too_many_hypotheses_are_cut_and_marked_approximate() {
        let pre = GameState::new(&GameConfig::official(PlayerCount::THREE), 1);
        let hands = std::array::from_fn(|q| {
            if q == 1 {
                Hand::new(1, 1, 1, 0, 0)
            } else {
                Hand::EMPTY
            }
        });
        let rob = Action::MoveRobber {
            hex: 0,
            victim: Some(1),
        };
        let unseen = Outcome::Stole { resource: None };
        for (max, kept, approximate) in [(3, 3, false), (2, 2, true)] {
            let mut t = BeliefTracker::with_hands(0, 3, hands);
            t.max_hypotheses = max;
            t.observe(&pre, 2, &rob, &unseen);
            assert_eq!(
                (t.num_hypotheses(), t.is_approximate()),
                (kept, approximate),
                "at most {max}"
            );
        }
    }

    #[test]
    fn accepting_a_trade_and_monopolies_reveal_cards() {
        use Resource::*;
        // After the steal, player 2 holds a wood (2 in 3) or a brick (1 in 3).
        let (mut pre, b) = after_unseen_steal(Hand::new(2, 1, 0, 0, 0), Hand::EMPTY);
        let mut accepted = b.clone();
        pre.trade = Some(PendingTrade {
            responder_offset: 2,
            ..PendingTrade::offer(Hand::of(Ore, 1), Hand::of(Brick, 1))
        });
        accepted.observe(&pre, 2, &Action::AcceptTrade, &Outcome::None);
        assert_eq!(accepted.num_hypotheses(), 1);
        assert_eq!(accepted.expected_hand(2), [0.0, 1.0, 0.0, 0.0, 0.0]);

        let mut monopoly = b;
        let mut taken = [0; MAX_PLAYERS];
        taken[1] = 2;
        taken[2] = 0;
        monopoly.observe(
            &pre,
            0,
            &Action::PlayMonopoly { resource: Wood },
            &Outcome::Monopoly { taken },
        );
        assert_eq!(
            monopoly.num_hypotheses(),
            1,
            "player 2 had no wood: the brick was stolen"
        );
        assert_eq!(monopoly.expected_hand(2), [0.0, 1.0, 0.0, 0.0, 0.0]);
        assert_eq!(monopoly.expected_hand(0), [2.0, 0.0, 0.0, 0.0, 0.0]);
        assert!(!monopoly.is_approximate());
    }
}
