//! A player's view of the game, and determinization for search.
//!
//! Bots never get the full [`GameState`]: they get a [`View`], which exposes public information
//! and the viewer's own cards. When a bot needs a complete state to simulate from, it calls
//! [`View::determinize`], which overwrites every piece of hidden information (opponents' hands
//! and development cards, the deck order, the RNG) with a sample consistent with what the viewer
//! knows. Nothing from the true hidden state survives into the sample.

use crate::actions::{Action, Outcome};
use crate::belief::BeliefTracker;
use crate::bits::Bits;
use crate::board::Board;
use crate::legal::OfferSpace;
use crate::rng::Rng;
use crate::rules::Rules;
use crate::state::{Building, GameState, PendingTrade, Phase, PlayerState};
use crate::types::*;

/// Read-only access to what one player can see.
#[derive(Clone, Copy)]
pub struct View<'a> {
    state: &'a GameState,
    me: PlayerId,
}

/// Public information about one player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublicPlayer {
    pub hand_size: u32,
    /// Unplayed development cards, including ones bought this turn.
    pub dev_cards: u32,
    /// Development cards bought this turn (only ever non-zero for the current player).
    pub dev_bought_this_turn: u32,
    pub dev_played: DevCounts,
    pub roads_left: u8,
    pub settlements_left: u8,
    pub cities_left: u8,
    pub road_length: u8,
    pub ports: u8,
}

impl GameState {
    pub fn view(&self, me: PlayerId) -> View<'_> {
        assert!((me as usize) < self.num_players(), "no such player");
        View { state: self, me }
    }
}

impl<'a> View<'a> {
    pub fn me(&self) -> PlayerId {
        self.me
    }

    pub fn rules(&self) -> &Rules {
        &self.state.rules
    }

    pub fn board(&self) -> &Board {
        &self.state.board
    }

    pub fn num_players(&self) -> usize {
        self.state.num_players()
    }

    pub fn phase(&self) -> Phase {
        self.state.phase
    }

    pub fn current(&self) -> PlayerId {
        self.state.current
    }

    pub fn acting(&self) -> PlayerId {
        self.state.acting_player()
    }

    pub fn turn(&self) -> u32 {
        self.state.turn
    }

    pub fn is_over(&self) -> bool {
        self.state.is_over()
    }

    pub fn winner(&self) -> Option<PlayerId> {
        self.state.winner
    }

    pub fn building(&self, v: VertexId) -> Option<Building> {
        self.state.building(v)
    }

    pub fn road_owner(&self, e: EdgeId) -> Option<PlayerId> {
        self.state.road_owner(e)
    }

    pub fn robber(&self) -> HexId {
        self.state.robber
    }

    pub fn bank(&self) -> Hand {
        self.state.bank
    }

    pub fn dev_deck_size(&self) -> usize {
        self.state.dev_deck.len()
    }

    pub fn longest_road(&self) -> Option<PlayerId> {
        self.state.longest_road
    }

    pub fn largest_army(&self) -> Option<PlayerId> {
        self.state.largest_army
    }

    pub fn trade(&self) -> Option<PendingTrade> {
        self.state.trade
    }

    pub fn last_roll(&self) -> Option<(u8, u8)> {
        self.state.last_roll
    }

    pub fn visible_vp(&self, p: PlayerId) -> u8 {
        self.state.visible_vp(p)
    }

    /// Whether the player on turn has played a development card this turn.
    pub fn dev_played_this_turn(&self) -> bool {
        self.state.dev_played_this_turn
    }

    /// Trade offers the player on turn has made this turn.
    pub fn offers_this_turn(&self) -> u8 {
        self.state.offers_this_turn
    }

    /// [`GameState::settlement_spots`] for seat `q`; `None` if no such seat is in the game.
    pub fn settlement_spots(&self, q: PlayerId) -> Option<Bits> {
        self.seated(q).then(|| self.state.settlement_spots(q))
    }

    /// [`GameState::road_spots`] for seat `q`; `None` if no such seat is in the game.
    pub fn road_spots(&self, q: PlayerId) -> Option<Bits> {
        self.seated(q).then(|| self.state.road_spots(q))
    }

    fn seated(&self, q: PlayerId) -> bool {
        usize::from(q) < self.num_players()
    }

    pub fn player(&self, p: PlayerId) -> PublicPlayer {
        let ps = &self.state.players[p as usize];
        PublicPlayer {
            hand_size: ps.hand.total(),
            dev_cards: ps.dev_cards_held(),
            dev_bought_this_turn: ps.dev_new.iter().map(|&c| c as u32).sum(),
            dev_played: ps.dev_played,
            roads_left: ps.roads_left,
            settlements_left: ps.settlements_left,
            cities_left: ps.cities_left,
            road_length: ps.road_length,
            ports: ps.ports,
        }
    }

    /// Everyone's hands, while they are public knowledge: until the first robber steal every
    /// card movement is visible to all players. `None` after that.
    pub fn public_hands(&self) -> Option<crate::belief::Hands> {
        if self.state.steals != 0 {
            return None;
        }
        let mut hands = [Hand::EMPTY; MAX_PLAYERS];
        for (h, p) in hands
            .iter_mut()
            .zip(&self.state.players[..self.num_players()])
        {
            *h = p.hand;
        }
        Some(hands)
    }

    /// The viewer's own full player state (hand and development cards included).
    pub fn me_state(&self) -> &PlayerState {
        &self.state.players[self.me as usize]
    }

    /// The viewer's points, counting their hidden victory point cards.
    pub fn my_total_vp(&self) -> u8 {
        self.state.total_vp(self.me)
    }

    /// Every seat's result, hidden cards included, once the game is over; `None` before.
    pub fn final_standings(&self) -> Option<Vec<crate::state::Standing>> {
        self.state.final_standings()
    }

    /// Legal actions. Only meaningful when the viewer is the acting player: legal actions
    /// depend on the acting player's own cards and public information only.
    pub fn legal_actions(&self, offers: OfferSpace, out: &mut Vec<Action>) {
        debug_assert_eq!(
            self.acting(),
            self.me,
            "legal actions belong to the acting player"
        );
        self.state.legal_actions_with(offers, out);
    }

    /// Development cards whose identity the viewer can't see: the deck plus opponents'
    /// unplayed cards.
    pub fn unseen_dev_pool(&self) -> DevCounts {
        let mut pool = BASE_DEV_DECK;
        let mine = &self.state.players[self.me as usize];
        for (i, count) in pool.iter_mut().enumerate() {
            *count -= mine.dev[i] + mine.dev_new[i];
            for p in &self.state.players[..self.num_players()] {
                *count -= p.dev_played[i];
            }
        }
        pool
    }

    /// Updates the viewer's belief with an action, given the view from before it.
    pub fn update_belief(
        &self,
        belief: &mut BeliefTracker,
        actor: PlayerId,
        action: &Action,
        outcome: &Outcome,
    ) {
        debug_assert_eq!(belief.observer(), self.me);
        // The tracker reads only public parts of the state.
        belief.observe(self.state, actor, action, outcome);
    }

    /// A complete game state consistent with everything the viewer knows: opponents' hands are
    /// drawn from `belief`, opponents' development cards and the deck order from the unseen pool,
    /// and the game RNG is reseeded.
    pub fn determinize(&self, belief: &BeliefTracker, rng: &mut Rng) -> GameState {
        debug_assert_eq!(belief.observer(), self.me);
        let mut s = self.state.clone();
        s.rng = Rng::seed(rng.next_u64());
        self.sample_hands(&mut s, belief, rng);
        self.sample_dev_cards(&mut s, rng);
        s
    }

    fn sample_hands(&self, s: &mut GameState, belief: &BeliefTracker, rng: &mut Rng) {
        let n = self.num_players();
        let me = self.me as usize;
        let mine = self.state.players[me].hand;
        // Cards of each resource held by opponents in total (public: bank and own hand known).
        let mut opp_total = Hand::EMPTY;
        for r in Resource::ALL {
            let held = BANK_PER_RESOURCE - self.state.bank.get(r) - mine.get(r);
            opp_total.add_n(r, held);
        }
        let sizes: Vec<u32> = (0..n).map(|q| self.state.players[q].hand.total()).collect();

        let consistent = |hands: &[Hand; MAX_PLAYERS]| {
            let mut sum = Hand::EMPTY;
            for q in (0..n).filter(|&q| q != me) {
                if hands[q].total() != sizes[q] {
                    return false;
                }
                sum.add(&hands[q]);
            }
            sum == opp_total && hands[me] == mine
        };
        if !belief.is_approximate() {
            let hands = belief.sample(rng);
            if consistent(&hands) {
                for q in (0..n).filter(|&q| q != me) {
                    s.players[q].hand = hands[q];
                }
                return;
            }
        }
        // Fallback (approximate belief): deal the opponents' cards at random.
        Self::deal_hands(s, me, opp_total, &sizes, rng);
    }

    /// Deals `opp_total` at random to every player but `me`, `sizes[q]` cards each.
    fn deal_hands(s: &mut GameState, me: usize, opp_total: Hand, sizes: &[u32], rng: &mut Rng) {
        let mut cards: Vec<Resource> = Resource::ALL
            .iter()
            .flat_map(|&r| std::iter::repeat_n(r, opp_total.get(r) as usize))
            .collect();
        rng.shuffle(&mut cards);
        let mut it = cards.into_iter();
        for q in (0..sizes.len()).filter(|&q| q != me) {
            let mut h = Hand::EMPTY;
            for r in it.by_ref().take(sizes[q] as usize) {
                h.add_n(r, 1);
            }
            s.players[q].hand = h;
        }
    }

    /// A complete game state consistent with the viewer's own cards and the public information,
    /// without a belief tracker: the cards the viewer can't see (opponents' hands and development
    /// cards, the deck order) are dealt at random, keeping every count. For players that keep no
    /// beliefs, such as those simulated in a search's rollouts. The game RNG is kept.
    pub fn deal_unseen(&self, rng: &mut Rng) -> GameState {
        let mut s = self.state.clone();
        let me = self.me as usize;
        let mine = self.state.players[me].hand;
        let mut opp_total = Hand::EMPTY;
        for r in Resource::ALL {
            opp_total.add_n(r, BANK_PER_RESOURCE - self.state.bank.get(r) - mine.get(r));
        }
        let sizes: Vec<u32> = (0..self.num_players())
            .map(|q| self.state.players[q].hand.total())
            .collect();
        Self::deal_hands(&mut s, me, opp_total, &sizes, rng);
        self.sample_dev_cards(&mut s, rng);
        s
    }

    fn sample_dev_cards(&self, s: &mut GameState, rng: &mut Rng) {
        let n = self.num_players();
        let me = self.me as usize;
        let mut pool = self.unseen_dev_pool();
        // When the game is over, the winner has revealed their cards (that's how they show their
        // points), so theirs stay as they are and only the rest is resampled.
        let revealed = self.state.winner.filter(|&w| w as usize != me);
        if let Some(w) = revealed {
            let ps = &self.state.players[w as usize];
            for (i, count) in pool.iter_mut().enumerate() {
                *count -= ps.dev[i] + ps.dev_new[i];
            }
        }
        let mut cards: Vec<DevCard> = DevCard::ALL
            .iter()
            .flat_map(|&c| std::iter::repeat_n(c, pool[c.index()] as usize))
            .collect();
        let target = s.rules.vp_to_win;
        // Opponents who haven't won can't secretly hold enough points to win; retry a few times
        // to respect that, then accept the sample.
        for attempt in 0..20 {
            rng.shuffle(&mut cards);
            let mut it = cards.iter().copied();
            for q in (0..n).filter(|&q| q != me && Some(q as PlayerId) != revealed) {
                let real = &self.state.players[q];
                let (old, new): (u32, u32) = (
                    real.dev.iter().map(|&c| c as u32).sum(),
                    real.dev_new.iter().map(|&c| c as u32).sum(),
                );
                let ps = &mut s.players[q];
                ps.dev = [0; NUM_DEV_TYPES];
                ps.dev_new = [0; NUM_DEV_TYPES];
                for c in it.by_ref().take(old as usize) {
                    ps.dev[c.index()] += 1;
                }
                for c in it.by_ref().take(new as usize) {
                    ps.dev_new[c.index()] += 1;
                }
            }
            s.dev_deck = it.collect();
            let plausible = (0..n as PlayerId)
                .filter(|&q| q as usize != me && Some(q) != s.winner)
                .all(|q| s.total_vp(q) < target);
            if plausible || attempt == 19 {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::PlayerCount;
    use std::sync::Arc;

    use super::*;
    use crate::actions::Forced;
    use crate::belief::Hands;
    use crate::board::BoardGen;
    use crate::rules::GameConfig;
    use crate::sim::{Limits, RandomPolicy, TurnGuard};

    /// Plays a random game, calling `f` with the state and every player's belief at each step.
    fn walk(seed: u64, f: impl FnMut(&GameState, &[BeliefTracker])) {
        walk_with(seed, 4096, RandomPolicy::Builder, f);
    }

    /// [`walk`] with trackers that keep at most `max_hypotheses`, playing `policy`.
    fn walk_with(
        seed: u64,
        max_hypotheses: usize,
        policy: RandomPolicy,
        mut f: impl FnMut(&GameState, &[BeliefTracker]),
    ) {
        let config = GameConfig::official(PlayerCount::FOUR);
        let mut state = GameState::new(&config, seed);
        let mut beliefs: Vec<_> = (0..4)
            .map(|p| {
                let mut b = BeliefTracker::new(p, 4);
                b.max_hypotheses = max_hypotheses;
                b
            })
            .collect();
        let (mut rng, mut guard, limits) =
            (Rng::seed(seed), TurnGuard::default(), Limits::default());
        let mut legal = Vec::new();
        while !state.is_over() && state.turn < 200 {
            state.legal_actions(&mut legal);
            let action = guard.choose(policy, &limits, &state, &legal, &mut rng);
            let actor = state.acting_player();
            let pre = state.clone();
            let outcome = state.apply(action).unwrap();
            for (p, b) in beliefs.iter_mut().enumerate() {
                let seen = outcome.visible_to(p as PlayerId, actor, &action);
                pre.view(p as PlayerId)
                    .update_belief(b, actor, &action, &seen);
            }
            f(&state, &beliefs);
        }
    }

    #[test]
    fn determinized_states_are_valid_and_match_public_information() {
        let mut rng = Rng::seed(1);
        let mut checked = 0;
        walk(7, |state, beliefs| {
            for me in 0..4u8 {
                let s = state.view(me).determinize(&beliefs[me as usize], &mut rng);
                s.check_invariants().unwrap();
                assert_eq!(s.players[me as usize], state.players[me as usize]);
                assert_eq!(s.bank, state.bank);
                for q in 0..4 {
                    assert_eq!(s.players[q].hand.total(), state.players[q].hand.total());
                    assert_eq!(
                        s.players[q].dev_cards_held(),
                        state.players[q].dev_cards_held()
                    );
                }
                assert_eq!(s.dev_deck.len(), state.dev_deck.len());
                checked += 1;
            }
        });
        assert!(checked > 100);
    }

    #[test]
    fn determinization_does_not_depend_on_hidden_information() {
        let mut done = false;
        walk(11, |state, beliefs| {
            // Find a position where two opponents of player 0 hold different cards, swap one card
            // between them and reshuffle the hidden deck: everything player 0 sees is unchanged.
            if done || state.turn < 30 {
                return;
            }
            let (a, b) = (1usize, 2usize);
            let Some(r) = Resource::ALL
                .into_iter()
                .find(|&r| state.players[a].hand.get(r) > 0)
            else {
                return;
            };
            let Some(t) = Resource::ALL
                .into_iter()
                .find(|&t| t != r && state.players[b].hand.get(t) > 0)
            else {
                return;
            };
            let mut other = state.clone();
            other.players[a].hand.sub_n(r, 1);
            other.players[a].hand.add_n(t, 1);
            other.players[b].hand.sub_n(t, 1);
            other.players[b].hand.add_n(r, 1);
            other.dev_deck.reverse();
            other.rng = Rng::seed(12345);

            let x = state.view(0).determinize(&beliefs[0], &mut Rng::seed(5));
            let y = other.view(0).determinize(&beliefs[0], &mut Rng::seed(5));
            assert_eq!(x.position_hash(), y.position_hash());
            assert_eq!(x.dev_deck, y.dev_deck);
            for q in 0..4 {
                assert_eq!(x.players[q], y.players[q]);
            }
            done = true;
        });
        assert!(done, "no suitable position found");
    }

    #[test]
    fn dealing_the_unseen_keeps_every_public_count_and_the_dice() {
        let mut rng = Rng::seed(2);
        let mut changed = 0;
        walk(9, |state, _| {
            for me in 0..4u8 {
                let s = state.view(me).deal_unseen(&mut rng);
                s.check_invariants().unwrap();
                assert_eq!(s.players[me as usize], state.players[me as usize]);
                assert_eq!((&s.bank, &s.rng), (&state.bank, &state.rng));
                for q in 0..4 {
                    assert_eq!(s.players[q].hand.total(), state.players[q].hand.total());
                    assert_eq!(s.players[q].dev_played, state.players[q].dev_played);
                    assert_eq!(
                        s.players[q].dev_cards_held(),
                        state.players[q].dev_cards_held()
                    );
                    changed += (s.players[q].hand != state.players[q].hand) as u32;
                }
                assert_eq!(s.dev_deck.len(), state.dev_deck.len());
            }
        });
        assert!(changed > 100, "opponents' hands were hardly ever redealt");
    }

    #[test]
    fn dealing_the_unseen_does_not_depend_on_it() {
        let mut done = false;
        walk(11, |state, _| {
            if done || state.turn < 30 {
                return;
            }
            let (a, b) = (1usize, 2usize);
            let Some(r) = Resource::ALL
                .into_iter()
                .find(|&r| state.players[a].hand.get(r) > 0)
            else {
                return;
            };
            let Some(t) = Resource::ALL
                .into_iter()
                .find(|&t| t != r && state.players[b].hand.get(t) > 0)
            else {
                return;
            };
            let mut other = state.clone();
            other.players[a].hand.sub_n(r, 1);
            other.players[a].hand.add_n(t, 1);
            other.players[b].hand.sub_n(t, 1);
            other.players[b].hand.add_n(r, 1);
            other.dev_deck.reverse();

            let x = state.view(0).deal_unseen(&mut Rng::seed(5));
            let y = other.view(0).deal_unseen(&mut Rng::seed(5));
            assert_eq!(x.dev_deck, y.dev_deck);
            for q in 0..4 {
                assert_eq!(x.players[q], y.players[q]);
            }
            done = true;
        });
        assert!(done, "no suitable position found");
    }

    #[test]
    fn the_view_shows_the_game_as_it_is() {
        let mut steps = 0;
        let (mut stolen, mut ended) = (false, false);
        walk(3, |state, beliefs| {
            for me in 0..4u8 {
                let v = state.view(me);
                assert_eq!(v.me(), me);
                assert_eq!(
                    (v.current(), v.acting(), v.turn()),
                    (state.current, state.acting_player(), state.turn)
                );
                assert_eq!((v.is_over(), v.winner()), (state.is_over(), state.winner));
                assert_eq!(
                    (v.robber(), v.bank(), v.dev_deck_size()),
                    (state.robber, state.bank, state.dev_deck.len())
                );
                assert_eq!(
                    (v.longest_road(), v.largest_army()),
                    (state.longest_road, state.largest_army)
                );
                assert_eq!((v.trade(), v.last_roll()), (state.trade, state.last_roll));
                for q in 0..4 {
                    assert_eq!(v.visible_vp(q), state.visible_vp(q));
                }
                for x in 0..state.board.topology.num_vertices() as VertexId {
                    assert_eq!(v.building(x), state.building(x));
                }
                for e in 0..state.board.topology.num_edges() as EdgeId {
                    assert_eq!(v.road_owner(e), state.road_owner(e));
                }
                assert_eq!(v.me_state(), &state.players[me as usize]);
                assert_eq!(v.my_total_vp(), state.total_vp(me));
                let truth: Hands = std::array::from_fn(|q| state.players[q].hand);
                match v.public_hands() {
                    Some(h) => assert_eq!((h, state.steals), (truth, 0)),
                    None => assert!(state.steals > 0),
                }
                if me == state.acting_player() && !state.is_over() {
                    let (mut a, mut b) = (Vec::new(), Vec::new());
                    v.legal_actions(OfferSpace::Simple, &mut a);
                    state.legal_actions_with(OfferSpace::Simple, &mut b);
                    assert_eq!(a, b);
                }
                assert_eq!(beliefs[me as usize].observer(), me);
            }
            stolen |= state.steals > 0;
            ended |= state.is_over();
            steps += 1;
        });
        assert!(steps > 200 && stolen && ended);

        // Largest Army, a pending trade, and a viewer's belief updated through the view.
        let board = Arc::new(Board::generate(&BoardGen::OFFICIAL, &mut Rng::seed(1)));
        let mut s =
            GameState::empty_position(GameConfig::official(PlayerCount::TWO).rules, board, 1)
                .unwrap();
        for _ in 0..3 {
            s.add_played_card(1, DevCard::Knight).unwrap();
        }
        s.finish_edit().unwrap();
        assert_eq!(s.view(0).largest_army(), Some(1));
        let mut s = GameState::new(&GameConfig::official(PlayerCount::TWO), 1);
        s.trade = Some(PendingTrade {
            responder_offset: 1,
            ..PendingTrade::offer(Hand::of(Resource::Wood, 1), Hand::of(Resource::Ore, 1))
        });
        assert_eq!(s.view(1).trade(), s.trade);
        let mut via_view = BeliefTracker::new(1, 2);
        let mut direct = via_view.clone();
        let roll = Outcome::Dice { a: 2, b: 3 };
        s.view(1)
            .update_belief(&mut via_view, 0, &Action::RollDice, &roll);
        direct.observe(&s, 0, &Action::RollDice, &roll);
        assert_eq!(via_view.hypotheses(), direct.hypotheses());
    }

    #[test]
    fn an_exact_belief_deals_the_true_hands() {
        let mut rng = Rng::seed(2);
        let mut checked = 0;
        walk(7, |state, beliefs| {
            for me in 0..4u8 {
                let b = &beliefs[me as usize];
                if b.num_hypotheses() != 1 || b.is_approximate() {
                    continue;
                }
                let s = state.view(me).determinize(b, &mut rng);
                for q in 0..4 {
                    assert_eq!(s.players[q].hand, state.players[q].hand);
                }
                checked += 1;
            }
        });
        assert!(checked > 100);
    }

    #[test]
    fn an_approximate_belief_falls_back_on_public_information_only() {
        // Trackers that keep one hypothesis turn approximate at the first hidden steal; the
        // fallback deals opponents' cards from what is public.
        let mut done = 0;
        walk_with(11, 1, RandomPolicy::Builder, |state, beliefs| {
            let b = &beliefs[0];
            if done >= 5 || !b.is_approximate() {
                return;
            }
            let (a, c) = (1usize, 2usize);
            let Some(r) = Resource::ALL
                .into_iter()
                .find(|&r| state.players[a].hand.get(r) > 0)
            else {
                return;
            };
            let Some(t) = Resource::ALL
                .into_iter()
                .find(|&t| t != r && state.players[c].hand.get(t) > 0)
            else {
                return;
            };
            let mut other = state.clone();
            other.players[a].hand.sub_n(r, 1);
            other.players[a].hand.add_n(t, 1);
            other.players[c].hand.sub_n(t, 1);
            other.players[c].hand.add_n(r, 1);
            let x = state.view(0).determinize(b, &mut Rng::seed(5));
            let y = other.view(0).determinize(b, &mut Rng::seed(5));
            for q in 0..4 {
                assert_eq!(x.players[q].hand, y.players[q].hand);
                assert_eq!(x.players[q].hand.total(), state.players[q].hand.total());
            }
            done += 1;
        });
        assert_eq!(done, 5, "too few approximate positions");
    }

    #[test]
    fn the_unseen_pool_leaves_out_my_cards_and_played_ones() {
        let board = Arc::new(Board::generate(&BoardGen::OFFICIAL, &mut Rng::seed(1)));
        let mut s =
            GameState::empty_position(GameConfig::official(PlayerCount::TWO).rules, board, 1)
                .unwrap();
        s.give_dev_card(0, DevCard::Knight, false).unwrap();
        s.give_dev_card(0, DevCard::Monopoly, true).unwrap();
        s.add_played_card(1, DevCard::Knight).unwrap();
        s.add_played_card(1, DevCard::Knight).unwrap();
        s.give_dev_card(1, DevCard::VictoryPoint, false).unwrap();
        s.finish_edit().unwrap();
        let mut want = BASE_DEV_DECK;
        want[DevCard::Knight.index()] -= 3;
        want[DevCard::Monopoly.index()] -= 1;
        assert_eq!(s.view(0).unseen_dev_pool(), want);
        let seen = s.observe(0);
        assert_eq!(seen.unseen_dev_pool(), want);
        let total: u32 = want.iter().map(|&c| c as u32).sum();
        assert_eq!(total, s.dev_deck.len() as u32 + 1);
        let p = seen.unseen_dev_probabilities();
        for c in DevCard::ALL {
            assert!((p[c.index()] - want[c.index()] as f64 / total as f64).abs() < 1e-12);
        }
        // Holding every card: nothing unseen, and no division by zero.
        let mut s = GameState::empty_position(s.rules, s.board.clone(), 1).unwrap();
        for c in DevCard::ALL {
            for _ in 0..BASE_DEV_DECK[c.index()] {
                s.give_dev_card(0, c, false).unwrap();
            }
        }
        s.finish_edit().unwrap();
        assert_eq!(
            s.observe(0).unseen_dev_probabilities(),
            [0.0; NUM_DEV_TYPES]
        );
    }

    #[test]
    fn determinized_opponents_never_hold_a_winning_hand() {
        // Player 1 has 9 points and one hidden card on their own turn: a victory point card
        // would have won already. The viewer has 10, waiting for their turn to win.
        let mut s = ten_and_nine();
        s.give_dev_card(1, DevCard::Knight, false).unwrap();
        s.set_turn(1, 30, true).unwrap();
        s.finish_edit().unwrap();
        assert_eq!((s.total_vp(0), s.total_vp(1), s.winner), (10, 9, None));

        let hands = std::array::from_fn(|q| s.players[q].hand);
        let belief = BeliefTracker::with_hands(0, 2, hands);
        let mut rng = Rng::seed(4);
        let mut kinds = std::collections::HashSet::new();
        for _ in 0..300 {
            let d = s.view(0).determinize(&belief, &mut rng);
            assert_eq!(d.players[1].dev[DevCard::VictoryPoint.index()], 0);
            assert_eq!(d.players[1].dev_cards_held(), 1);
            kinds.insert(d.players[1].dev);
        }
        assert!(kinds.len() > 1, "the hidden card still varies");
    }

    /// Eleven pairwise non-adjacent vertices: player 0 gets 4 cities and 2 settlements (10
    /// points), player 1 4 cities and 1 settlement (9).
    fn ten_and_nine() -> GameState {
        let board = Arc::new(Board::generate(&BoardGen::OFFICIAL, &mut Rng::seed(1)));
        let mut s =
            GameState::empty_position(GameConfig::official(PlayerCount::TWO).rules, board, 1)
                .unwrap();
        let topo = s.board.topology.clone();
        let mut spots: Vec<VertexId> = Vec::new();
        for v in 0..topo.num_vertices() as VertexId {
            let near =
                |w: &VertexId| *w == v || topo.vertex_neighbors[v as usize].as_slice().contains(w);
            if spots.len() < 11 && !spots.iter().any(near) {
                spots.push(v);
            }
        }
        assert_eq!(spots.len(), 11);
        for (i, &v) in spots.iter().enumerate() {
            match i {
                0..4 => s.put_city(0, v),
                4..6 => s.put_settlement(0, v),
                6..10 => s.put_city(1, v),
                _ => s.put_settlement(1, v),
            }
            .unwrap();
        }
        s
    }

    #[test]
    fn a_winners_cards_stay_revealed() {
        // Player 1 wins by buying a victory point card, which everyone then sees.
        let mut s = ten_and_nine();
        s.set_turn(1, 30, true).unwrap();
        s.set_hand(1, cost::DEV_CARD).unwrap();
        s.finish_edit().unwrap();
        s.apply_forced(
            Action::BuyDevCard,
            Some(Forced::DevCard(DevCard::VictoryPoint)),
        )
        .unwrap();
        assert_eq!(s.winner, Some(1));
        let belief = BeliefTracker::with_hands(0, 2, std::array::from_fn(|q| s.players[q].hand));
        let mut rng = Rng::seed(6);
        for _ in 0..50 {
            let d = s.view(0).determinize(&belief, &mut rng);
            assert_eq!(
                (d.players[1].dev, d.players[1].dev_new),
                (s.players[1].dev, s.players[1].dev_new)
            );
            assert_eq!(d.dev_deck.len(), s.dev_deck.len());
        }
    }

    #[test]
    fn a_belief_that_disagrees_with_public_counts_is_not_used() {
        // For example hands typed into the analysis board that don't add up: player 1 holds two
        // wood, the belief says two brick. The public counts say the two cards are wood.
        let board = Arc::new(Board::generate(&BoardGen::OFFICIAL, &mut Rng::seed(1)));
        let mut s =
            GameState::empty_position(GameConfig::official(PlayerCount::TWO).rules, board, 1)
                .unwrap();
        s.set_hand(1, Hand::new(2, 0, 0, 0, 0)).unwrap();
        s.finish_edit().unwrap();
        let wrong = std::array::from_fn(|q| {
            if q == 1 {
                Hand::new(0, 2, 0, 0, 0)
            } else {
                Hand::EMPTY
            }
        });
        let belief = BeliefTracker::with_hands(0, 2, wrong);
        let d = s.view(0).determinize(&belief, &mut Rng::seed(1));
        assert_eq!(d.players[1].hand, Hand::new(2, 0, 0, 0, 0));
    }

    /// Every seat number a caller could pass: each seat in the game, the empty seats up to
    /// `MAX_PLAYERS`, one past that, and the largest `PlayerId`.
    fn every_seat_id() -> impl Iterator<Item = PlayerId> {
        (0..=MAX_PLAYERS as PlayerId).chain([PlayerId::MAX])
    }

    /// The four turn facts a view gives, for every seat id in [`every_seat_id`].
    type TurnFacts = (bool, u8, Vec<Option<Bits>>, Vec<Option<Bits>>);

    fn turn_facts(v: &View) -> TurnFacts {
        (
            v.dev_played_this_turn(),
            v.offers_this_turn(),
            every_seat_id().map(|q| v.settlement_spots(q)).collect(),
            every_seat_id().map(|q| v.road_spots(q)).collect(),
        )
    }

    /// Games with 2, 3 and 4 players, played by both random policies, so that offers, played
    /// development cards and settlement spots all occur.
    fn walk_games(mut f: impl FnMut(&GameState)) {
        for (seed, policy) in [
            (3, RandomPolicy::Builder),
            (5, RandomPolicy::Uniform),
            (8, RandomPolicy::Uniform),
        ] {
            walk_with(seed, 64, policy, |state, _| f(state));
        }
        for (players, seed) in [(PlayerCount::TWO, 21), (PlayerCount::THREE, 22)] {
            let config = GameConfig::official(players);
            let mut state = GameState::new(&config, seed);
            let (mut rng, mut guard, limits) =
                (Rng::seed(seed), TurnGuard::default(), Limits::default());
            let mut legal = Vec::new();
            while !state.is_over() && state.turn < 200 {
                state.legal_actions(&mut legal);
                let action = guard.choose(RandomPolicy::Uniform, &limits, &state, &legal, &mut rng);
                state.apply(action).unwrap();
                f(&state);
            }
        }
    }

    #[test]
    fn the_turn_facts_are_the_states_own() {
        let (mut played, mut offered, mut opponent_spots, mut steps) = (0, 0, 0, 0);
        walk_games(|state| {
            let n = state.num_players();
            for me in 0..n as PlayerId {
                let v = state.view(me);
                assert_eq!(v.dev_played_this_turn(), state.dev_played_this_turn);
                assert_eq!(v.offers_this_turn(), state.offers_this_turn);
                for q in every_seat_id() {
                    let seated = (q as usize) < n;
                    assert_eq!(
                        v.settlement_spots(q),
                        seated.then(|| state.settlement_spots(q))
                    );
                    assert_eq!(v.road_spots(q), seated.then(|| state.road_spots(q)));
                    if seated && q != me && !state.settlement_spots(q).is_empty() {
                        opponent_spots += 1;
                    }
                }
            }
            played += state.dev_played_this_turn as u32;
            offered += (state.offers_this_turn > 0) as u32;
            steps += 1;
        });
        // The walk must reach every case the accessors report, or the test proves nothing.
        assert!(steps > 1000, "only {steps} steps");
        assert!(
            played > 20,
            "a development card was played at only {played} steps"
        );
        assert!(
            offered > 20,
            "an offer had been made at only {offered} steps"
        );
        assert!(
            opponent_spots > 100,
            "opponents had settlement spots only {opponent_spots} times"
        );
    }

    #[test]
    fn the_turn_facts_do_not_depend_on_hidden_cards() {
        // For each viewer, a second state that differs only in what the viewer can't see:
        // opponents' hands and development cards redealt, the deck reversed, the dice reseeded.
        let mut rng = Rng::seed(4);
        let (mut differed, mut compared) = (0, 0);
        walk_games(|state| {
            for me in 0..state.num_players() as PlayerId {
                let mut other = state.view(me).deal_unseen(&mut rng);
                other.dev_deck.reverse();
                other.rng = Rng::seed(rng.next_u64());
                other.check_invariants().unwrap();
                let hidden_differs = other.dev_deck != state.dev_deck
                    || (0..state.num_players()).any(|q| {
                        let (a, b) = (&state.players[q], &other.players[q]);
                        (a.hand, a.dev, a.dev_new) != (b.hand, b.dev, b.dev_new)
                    });
                differed += hidden_differs as u32;
                compared += 1;
                assert_eq!(turn_facts(&state.view(me)), turn_facts(&other.view(me)));
            }
        });
        assert!(
            differed * 2 > compared,
            "the hidden cards differed in only {differed} of {compared} comparisons"
        );
    }
}
