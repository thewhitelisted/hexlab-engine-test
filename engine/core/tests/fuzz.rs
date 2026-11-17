//! A short fuzz run with every check at every step: invariants, `check` against `legal_actions`,
//! belief tracking, and record round trips and replays. The long runs use `hexlab fuzz`; this
//! keeps the same checks in every test run, so mutation testing sees them too.

use engine_core::fuzz::{FuzzJob, FuzzStats, cross_check_legality, fuzz_game};
use engine_core::sim::{Limits, RandomPolicy, TurnGuard};
use engine_core::{GameConfig, GameState, Rng};

fn run(config: GameConfig, policy: RandomPolicy, games: u64, seed: u64) -> FuzzStats {
    let job = FuzzJob {
        config,
        games,
        seed,
        policy,
        cross_check_every: 1,
    };
    let mut stats = FuzzStats::default();
    for g in 0..games {
        fuzz_game(&job, g, &mut stats).unwrap();
    }
    // Short games never need pruning, so every tracker stays exact and checked at every step.
    assert_eq!(stats.approximate_trackers, 0);
    let n = job.config.rules.num_players as u64;
    assert!(stats.actions > 0 && stats.turns > 0);
    assert_eq!(stats.cross_checked, stats.actions, "a step went unchecked");
    assert_eq!(
        stats.belief_checks,
        stats.actions * n,
        "a belief went unchecked"
    );
    assert_eq!(stats.wins_by_seat.iter().sum::<u64>(), stats.finished);
    stats
}

#[test]
fn random_games_pass_every_check() {
    let stats = run(GameConfig::official(4), RandomPolicy::Uniform, 6, 1);
    assert_eq!(stats.games, 6);
    let stats = run(GameConfig::official(4), RandomPolicy::Builder, 12, 2);
    assert!(stats.finished > 0, "builder games reach a winner");
}

#[test]
fn other_presets_pass_every_check() {
    run(GameConfig::official(3), RandomPolicy::Builder, 6, 3);
    run(GameConfig::colonist_1v1(), RandomPolicy::Builder, 6, 4);
    run(
        GameConfig::catanatron_compat(4),
        RandomPolicy::Builder,
        6,
        5,
    );
}

/// How often each kind of trading move comes up in the fuzzer's games of `config` (the same
/// games `run` plays: the same seeds, policy and choices).
fn trading_moves(config: &GameConfig, policy: RandomPolicy, games: u64, seed: u64) -> [u64; 5] {
    use engine_core::Action;
    let limits = Limits::default();
    let mut counts = [0; 5];
    let mut legal = Vec::new();
    for g in 0..games {
        let game_seed = seed.wrapping_mul(1_000_000_007).wrapping_add(g);
        let mut state = GameState::new(config, game_seed);
        let mut rng = Rng::seed(game_seed ^ 0x0f0f_1234_5678);
        let mut guard = TurnGuard::default();
        while !state.is_over() && state.turn <= limits.max_turns {
            state.legal_actions(&mut legal);
            let action = guard.choose(policy, &limits, &state, &legal, &mut rng);
            let kind = match action {
                Action::OfferTrade { .. } => Some(0),
                Action::AcceptTrade => Some(1),
                Action::RejectTrade => Some(2),
                Action::ProposeTrade { .. } => Some(3),
                Action::ConfirmTrade { .. } => Some(4),
                _ => None,
            };
            if let Some(k) = kind {
                counts[k] += 1;
            }
            state.apply_legal(action);
        }
    }
    counts
}

/// Audit 7.5: no automated check played a two-player `official` game, where open trading has a
/// single responder (who accepts, declines or counters, and the round is over).
#[test]
fn two_player_official_games_with_open_trading_pass_every_check() {
    let config = GameConfig::official(2);
    assert!(config.rules.player_trading && config.rules.open_trading);
    for (policy, games, seed) in [
        (RandomPolicy::Uniform, 8, 21),
        (RandomPolicy::Builder, 12, 22),
    ] {
        let stats = run(config, policy, games, seed);
        assert_eq!(stats.games, games);
        // The games did trade: offers were made, answered both ways, countered and confirmed.
        let [offers, accepts, rejects, proposals, confirms] =
            trading_moves(&config, policy, games, seed);
        assert!(offers > 0, "{policy:?}: no offers");
        if policy == RandomPolicy::Uniform {
            assert!(
                accepts > 0 && rejects > 0 && proposals > 0 && confirms > 0,
                "accepted {accepts}, declined {rejects}, countered {proposals}, confirmed {confirms}"
            );
        }
    }
    let stats = run(config, RandomPolicy::Builder, 12, 22);
    assert!(
        stats.finished > 0,
        "two-player builder games reach a winner"
    );
}

/// Audit 7.5: the two older trade flows are still presets (to replay what was measured with
/// them) and never went through this fuzzer.
#[test]
fn the_older_trade_flows_pass_every_check() {
    for name in ["official-no-counters", "official-proposal-round"] {
        for players in [4, 3, 2] {
            let config = GameConfig::preset(name, players).expect("a preset");
            assert!(config.rules.player_trading, "{name}");
            for (policy, games) in [(RandomPolicy::Uniform, 4), (RandomPolicy::Builder, 6)] {
                let seed = 30 + u64::from(players);
                let stats = run(config, policy, games, seed);
                assert_eq!(stats.games, games, "{name}, {players} players");
                let [offers, _, _, proposals, confirms] =
                    trading_moves(&config, policy, games, seed);
                assert!(
                    offers > 0,
                    "{name}, {players} players, {policy:?}: no offers"
                );
                if policy == RandomPolicy::Uniform {
                    assert!(
                        confirms > 0,
                        "{name}, {players} players: no trade went through"
                    );
                    // What sets the flows apart: counter-offers, and the round of proposals.
                    assert_eq!(
                        proposals > 0,
                        config.rules.open_trading,
                        "{name}, {players} players: {proposals} proposals"
                    );
                }
            }
        }
    }
    let old = GameConfig::preset("official-no-counters", 4).unwrap();
    let round = GameConfig::preset("official-proposal-round", 4).unwrap();
    assert!(!old.rules.open_trading && round.rules.open_trading && round.rules.trade_proposals);
}

#[test]
fn the_legality_cross_check_catches_a_wrong_list() {
    let mut s = GameState::new(&GameConfig::official(4), 9);
    let (mut rng, mut guard, limits) = (Rng::seed(9), TurnGuard::default(), Limits::default());
    let mut legal = Vec::new();
    loop {
        s.legal_actions(&mut legal);
        if s.turn >= 6 && legal.len() > 1 {
            break;
        }
        let a = guard.choose(RandomPolicy::Builder, &limits, &s, &legal, &mut rng);
        s.apply_legal(a);
    }
    cross_check_legality(&s, &legal).unwrap();
    let mut doubled = legal.clone();
    doubled.push(legal[0]);
    assert!(cross_check_legality(&s, &doubled).is_err());
    assert!(cross_check_legality(&s, &legal[1..]).is_err());
}

#[test]
fn stats_from_several_threads_add_up() {
    let a = FuzzStats {
        games: 3,
        finished: 2,
        actions: 900,
        turns: 120,
        wins_by_seat: [1, 0, 1, 0, 0, 0],
        max_hypotheses: 4,
        approximate_trackers: 1,
        cross_checked: 300,
        belief_checks: 3600,
    };
    let b = FuzzStats {
        games: 2,
        finished: 1,
        actions: 500,
        turns: 80,
        wins_by_seat: [0, 0, 0, 1, 0, 0],
        max_hypotheses: 9,
        approximate_trackers: 2,
        cross_checked: 500,
        belief_checks: 2000,
    };
    let mut sum = FuzzStats::default();
    sum.merge(&a);
    sum.merge(&b);
    assert_eq!(
        (sum.games, sum.finished, sum.actions, sum.turns),
        (5, 3, 1400, 200)
    );
    assert_eq!(sum.wins_by_seat, [1, 0, 1, 1, 0, 0]);
    assert_eq!((sum.max_hypotheses, sum.approximate_trackers), (9, 3));
    assert_eq!((sum.cross_checked, sum.belief_checks), (800, 5600));
}

#[test]
fn cross_checks_run_on_every_nth_game_or_never() {
    let job = |cross_check_every| FuzzJob {
        config: GameConfig::official(4),
        games: 3,
        seed: 8,
        policy: RandomPolicy::Builder,
        cross_check_every,
    };
    let actions = |stats: &FuzzStats| stats.actions;
    let mut per_game = Vec::new();
    for g in 0..3 {
        let mut stats = FuzzStats::default();
        fuzz_game(&job(1), g, &mut stats).unwrap();
        per_game.push(actions(&stats));
    }
    let mut never = FuzzStats::default();
    let mut every_other = FuzzStats::default();
    for g in 0..3 {
        fuzz_game(&job(0), g, &mut never).unwrap();
        fuzz_game(&job(2), g, &mut every_other).unwrap();
    }
    assert_eq!(never.cross_checked, 0);
    assert_eq!(every_other.cross_checked, per_game[0] + per_game[2]);
}
