//! The fuzzer's per-game checks: play one random game with every check enabled. Shared by
//! `hexlab fuzz` and the engine's own tests, so mutation testing sees the same checks the long
//! fuzz runs make.

use std::collections::HashSet;

use crate::action_bytes;
use crate::record::GameRecord;
use crate::sim::{Limits, RandomPolicy, TurnGuard};
use crate::{
    Action, BeliefTracker, GameConfig, GameState, Hand, HexId, PlayerId, Resource, Rng, VertexId,
};

#[derive(Default)]
pub struct FuzzStats {
    pub games: u64,
    pub finished: u64,
    pub actions: u64,
    pub turns: u64,
    pub wins_by_seat: [u64; 6],
    pub max_hypotheses: usize,
    pub approximate_trackers: u64,
    /// Steps whose legal action list was cross-checked against `check`.
    pub cross_checked: u64,
    /// Observations after which an exact belief tracker was checked against the true hands.
    pub belief_checks: u64,
}

impl FuzzStats {
    pub fn merge(&mut self, o: &FuzzStats) {
        self.games += o.games;
        self.finished += o.finished;
        self.actions += o.actions;
        self.turns += o.turns;
        for (a, b) in self.wins_by_seat.iter_mut().zip(o.wins_by_seat) {
            *a += b;
        }
        self.max_hypotheses = self.max_hypotheses.max(o.max_hypotheses);
        self.approximate_trackers += o.approximate_trackers;
        self.cross_checked += o.cross_checked;
        self.belief_checks += o.belief_checks;
    }
}

pub struct FuzzJob {
    pub config: GameConfig,
    pub games: u64,
    pub seed: u64,
    pub policy: RandomPolicy,
    pub cross_check_every: u64,
}

/// Plays game number `g` of a fuzz job with every check enabled.
pub fn fuzz_game(job: &FuzzJob, g: u64, stats: &mut FuzzStats) -> Result<(), String> {
    let config = &job.config;
    let n = config.rules.num_players.get() as usize;
    let limits = Limits::default();
    let mut legal = Vec::new();
    let game_seed = job.seed.wrapping_mul(1_000_000_007).wrapping_add(g);
    let fail = |step: usize, msg: String| format!("seed {game_seed}, step {step}: {msg}");
    let mut state = GameState::new(config, game_seed);
    let mut record = GameRecord::start(config, game_seed, &state);
    let mut trackers: Vec<_> = (0..n as PlayerId)
        .map(|p| BeliefTracker::new(p, n))
        .collect();
    let mut rng = Rng::seed(game_seed ^ 0x0f0f_1234_5678);
    let mut guard = TurnGuard::default();
    let cross_check = job.cross_check_every > 0 && g.is_multiple_of(job.cross_check_every);

    while !state.is_over() && state.turn <= limits.max_turns {
        let step = record.steps.len();
        state.legal_actions(&mut legal);
        if legal.is_empty() {
            return Err(fail(step, "no legal actions in a live game".into()));
        }
        if cross_check {
            cross_check_legality(&state, &legal).map_err(|m| fail(step, m))?;
            stats.cross_checked += 1;
        }
        let action = guard.choose(job.policy, &limits, &state, &legal, &mut rng);
        let actor = state.acting_player();
        let pre = state.clone();
        let outcome = record
            .play(&mut state, action)
            .map_err(|e| fail(step, format!("{action:?} rejected: {e}")))?;
        state
            .check_invariants()
            .map_err(|m| fail(step, format!("after {action:?}: {m}")))?;
        for t in &mut trackers {
            let seen = outcome.visible_to(t.observer(), actor, &action);
            t.observe(&pre, actor, &action, &seen);
            if !t.is_approximate() {
                let truth = t
                    .hypotheses()
                    .iter()
                    .any(|(h, _)| (0..n).all(|q| h[q] == state.players[q].hand));
                if !truth {
                    return Err(fail(
                        step,
                        format!("player {} belief lost the true hands", t.observer()),
                    ));
                }
                stats.belief_checks += 1;
            }
            stats.max_hypotheses = stats.max_hypotheses.max(t.num_hypotheses());
        }
    }
    record.finish(&state);

    let json = serde_json::to_string(&record).map_err(|e| e.to_string())?;
    let back: GameRecord = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    if back != record {
        return Err(fail(
            record.steps.len(),
            "record JSON round trip changed it".into(),
        ));
    }
    // The same moves as bytes (`hexlab positions --moves`) read back as they were played.
    let mut bytes = Vec::new();
    for s in &record.steps {
        action_bytes::write(&s.action, &mut bytes);
    }
    let mut rest = bytes.as_slice();
    for (i, s) in record.steps.iter().enumerate() {
        let (action, used) = action_bytes::read(rest)
            .map_err(|e| fail(i, format!("{:?} as bytes: {e}", s.action)))?;
        if action != s.action {
            return Err(fail(
                i,
                format!("{:?} came back from bytes as {action:?}", s.action),
            ));
        }
        rest = rest.get(used..).unwrap_or_default();
    }
    if !rest.is_empty() {
        return Err(fail(
            record.steps.len(),
            "bytes left after the last move".into(),
        ));
    }
    let replayed = back
        .replay()
        .map_err(|e| fail(record.steps.len(), format!("replay failed: {e}")))?;
    // The position hash leaves the dice out: with balanced dice, the replayed rolls must have
    // come out of the deck as the game's did.
    if replayed.dice != state.dice {
        return Err(fail(
            record.steps.len(),
            "the replay left a different dice deck".into(),
        ));
    }

    stats.games += 1;
    stats.actions += record.steps.len() as u64;
    stats.turns += state.turn as u64;
    stats.approximate_trackers += trackers.iter().filter(|t| t.is_approximate()).count() as u64;
    if let Some(w) = state.winner {
        stats.finished += 1;
        stats.wins_by_seat[w as usize] += 1;
    }
    Ok(())
}

/// Every candidate action must pass `check` exactly when `legal_actions` lists it.
pub fn cross_check_legality(state: &GameState, legal: &[Action]) -> Result<(), String> {
    let set: HashSet<Action> = legal.iter().copied().collect();
    if set.len() != legal.len() {
        return Err("duplicate legal actions".into());
    }
    let topo = &state.board.topology;
    let mut candidates = vec![
        Action::BuyDevCard,
        Action::RollDice,
        Action::PlayKnight,
        Action::PlayRoadBuilding,
        Action::AcceptTrade,
        Action::RejectTrade,
        Action::CancelTrade,
        Action::EndTurn,
    ];
    for v in 0..topo.num_vertices() as VertexId {
        candidates.push(Action::BuildSettlement { vertex: v });
        candidates.push(Action::BuildCity { vertex: v });
    }
    for e in 0..topo.num_edges() as u16 {
        candidates.push(Action::BuildRoad { edge: e });
    }
    for (i, a) in Resource::ALL.into_iter().enumerate() {
        candidates.push(Action::PlayMonopoly { resource: a });
        candidates.push(Action::Discard { resource: a });
        for b in Resource::ALL.into_iter().skip(i) {
            candidates.push(Action::PlayYearOfPlenty { a, b });
        }
        for b in Resource::ALL {
            candidates.push(Action::MaritimeTrade { give: a, get: b });
        }
    }
    for h in 0..topo.num_hexes() as HexId {
        candidates.push(Action::MoveRobber {
            hex: h,
            victim: None,
        });
        for q in 0..state.rules.num_players.get() {
            candidates.push(Action::MoveRobber {
                hex: h,
                victim: Some(q),
            });
        }
    }
    for q in 0..state.rules.num_players.get() {
        candidates.push(Action::ConfirmTrade { with: q });
    }
    for a in candidates {
        let ok = state.check(a).is_ok();
        if ok != set.contains(&a) {
            return Err(format!(
                "{a:?}: check says {ok}, legal_actions says {}",
                set.contains(&a)
            ));
        }
    }
    for a in legal {
        if let Action::OfferTrade { .. } | Action::ProposeTrade { .. } = a {
            state
                .check(*a)
                .map_err(|e| format!("listed trade {a:?} rejected: {e}"))?;
        }
    }
    // A sample of trades outside the enumerated space must still follow the trade rules.
    let like = (Hand::new(1, 1, 0, 0, 0), Hand::new(1, 0, 0, 0, 0));
    let gift = (Hand::new(1, 0, 0, 0, 0), Hand::EMPTY);
    let too_many = (Hand::new(1, 0, 0, 0, 0), Hand::new(0, 20, 0, 0, 0));
    for (give, get) in [like, gift, too_many] {
        for a in [
            Action::OfferTrade { give, get },
            Action::ProposeTrade { give, get },
        ] {
            if state.check(a).is_ok() {
                return Err(format!(
                    "{a:?} was accepted: no gifts, like for like or more than exist"
                ));
            }
        }
    }
    // A proposal outside the enumerated space, of cards the player holds, is legal whenever
    // proposing is.
    let hand = state.players[state.acting_player() as usize].hand;
    let held: Vec<Resource> = Resource::ALL
        .into_iter()
        .filter(|&r| hand.get(r) > 0)
        .collect();
    if held.len() >= 2 {
        let (a, b) = (held[0], held[1]);
        let other = Resource::ALL
            .into_iter()
            .find(|&r| r != a && r != b)
            .expect("five");
        let mut give = Hand::of(a, 1);
        give.add_n(b, 1);
        let wide = Action::ProposeTrade {
            give,
            get: Hand::of(other, 3),
        };
        let can_propose = set.iter().any(|x| matches!(x, Action::RejectTrade))
            && state.phase == crate::Phase::TradeResponse
            && state.rules.open_trading;
        if state.check(wide).is_ok() != can_propose {
            return Err(format!(
                "{wide:?}: check says {}, proposing is {can_propose}",
                state.check(wide).is_ok()
            ));
        }
    }
    Ok(())
}
