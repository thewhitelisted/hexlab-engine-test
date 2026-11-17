//! Scenario tests for specific rules. The fuzzer covers random play; these pin down the cases
//! where a plausible implementation could still be wrong.

use std::collections::HashSet;
use std::sync::Arc;

use engine_core::actions::RuleError;
use engine_core::board::{Board, BoardError, BoardGen};
use engine_core::edit::EditError;
use engine_core::record::{GameRecord, ReplayError};
use engine_core::rules::ShortageRule;
use engine_core::sim::{Limits, RandomPolicy, TurnGuard, play_out};
use engine_core::state::Resume;
use engine_core::topology::Axial;
use engine_core::*;

fn board(seed: u64) -> Arc<Board> {
    let mut rng = Rng::seed(seed);
    Arc::new(Board::generate(&BoardGen::OFFICIAL, &mut rng))
}

fn position(n: u8) -> GameState {
    GameState::empty_position(GameConfig::official(n).rules, board(1), 99)
}

fn neighbors(s: &GameState, v: VertexId) -> Vec<VertexId> {
    s.board.topology.vertex_neighbors[v as usize]
        .as_slice()
        .to_vec()
}

fn edge(s: &GameState, a: VertexId, b: VertexId) -> EdgeId {
    s.board
        .topology
        .edge_between(a, b)
        .expect("adjacent vertices")
}

/// Vertices plus their neighbours.
fn around(s: &GameState, vs: &[VertexId]) -> HashSet<VertexId> {
    let mut out = HashSet::new();
    for &v in vs {
        out.insert(v);
        out.extend(neighbors(s, v));
    }
    out
}

/// A simple path of `len` edges (len + 1 vertices) avoiding `avoid`.
fn find_path(s: &GameState, len: usize, avoid: &HashSet<VertexId>) -> Vec<VertexId> {
    fn extend(
        s: &GameState,
        path: &mut Vec<VertexId>,
        len: usize,
        avoid: &HashSet<VertexId>,
    ) -> bool {
        if path.len() == len + 1 {
            return true;
        }
        let last = *path.last().unwrap();
        for n in neighbors(s, last) {
            if !avoid.contains(&n) && !path.contains(&n) {
                path.push(n);
                if extend(s, path, len, avoid) {
                    return true;
                }
                path.pop();
            }
        }
        false
    }
    for start in 0..s.board.topology.num_vertices() as VertexId {
        if avoid.contains(&start) {
            continue;
        }
        let mut path = vec![start];
        if extend(s, &mut path, len, avoid) {
            return path;
        }
    }
    panic!("no path of length {len}");
}

/// Settlement at the start of `path`, roads along it.
fn put_path(s: &mut GameState, p: PlayerId, path: &[VertexId]) {
    s.put_settlement(p, path[0]).unwrap();
    for w in path.windows(2) {
        let e = edge(s, w[0], w[1]);
        s.put_road(p, e).unwrap();
    }
}

/// `k` vertices, pairwise non-adjacent, avoiding `avoid`.
fn spread_vertices(s: &GameState, k: usize, avoid: &HashSet<VertexId>) -> Vec<VertexId> {
    let mut chosen: Vec<VertexId> = Vec::new();
    for v in 0..s.board.topology.num_vertices() as VertexId {
        if chosen.len() == k {
            break;
        }
        if !avoid.contains(&v) && !around(s, &chosen).contains(&v) {
            chosen.push(v);
        }
    }
    assert_eq!(chosen.len(), k);
    chosen
}

fn legal(s: &GameState) -> Vec<Action> {
    let mut out = Vec::new();
    s.legal_actions(&mut out);
    out
}

#[test]
fn initial_placement_is_snake_ordered_and_pays_the_second_settlement() {
    let mut s = GameState::new(&GameConfig::official(4), 3);
    let mut order = Vec::new();
    let mut paid = 0;
    while s.turn == 0 {
        let actions = legal(&s);
        match s.phase {
            Phase::SetupSettlement => {
                let p = s.current;
                order.push(p);
                let Action::BuildSettlement { vertex } = actions[actions.len() / 2] else {
                    panic!("expected a settlement")
                };
                let before = s.player(p).hand;
                // One card per resource hex the settlement touches, read from the board.
                let mut expected = Hand::EMPTY;
                if order.len() > 4 {
                    for &h in s.board.topology.vertex_hexes[vertex as usize].as_slice() {
                        if let Some(r) = s.board.tiles[h as usize].resource {
                            expected.add_n(r, 1);
                        }
                    }
                    paid += expected.total();
                }
                s.apply(Action::BuildSettlement { vertex }).unwrap();
                let mut want = before;
                want.add(&expected);
                assert_eq!(s.player(p).hand, want);
                // Distance rule: neither the spot nor its neighbours stay available.
                for v in std::iter::once(vertex).chain(neighbors(&s, vertex)) {
                    assert!(!s.vertex_open(v));
                }
            }
            _ => {
                s.apply(actions[0]).unwrap();
            }
        }
    }
    assert_eq!(order, vec![0, 1, 2, 3, 3, 2, 1, 0]);
    assert!(paid > 0, "the second settlements touched resource hexes");
    assert_eq!((s.phase, s.current, s.turn), (Phase::PreRoll, 0, 1));
    s.check_invariants().unwrap();
}

#[test]
fn settling_on_a_harbour_during_play_gives_its_rate() {
    // Through `apply`, not the editor.
    let mut s = GameState::new(&GameConfig::official(2), 3);
    let ports = s.board.ports.clone();
    let (spot, vs) = ports.iter().find(|(p, _)| p.kind != Port::Generic).unwrap();
    let Port::Special(r) = spot.kind else {
        unreachable!()
    };
    assert_eq!(s.player(0).trade_ratio(r), 4);
    s.apply(Action::BuildSettlement { vertex: vs[0] }).unwrap();
    assert_eq!(s.player(0).trade_ratio(r), 2);
}

#[test]
fn ids_off_the_board_are_refused() {
    let mut s = GameState::new(&GameConfig::official(2), 3);
    let topo = s.board.topology.clone();
    let (nv, ne) = (topo.num_vertices() as VertexId, topo.num_edges() as EdgeId);
    assert_eq!(
        s.check(Action::BuildSettlement { vertex: nv }),
        Err(RuleError::InvalidId)
    );
    let Action::BuildSettlement { vertex } = legal(&s)[0] else {
        panic!("expected a settlement")
    };
    s.apply(Action::BuildSettlement { vertex }).unwrap();
    assert_eq!(
        s.check(Action::BuildRoad { edge: ne }),
        Err(RuleError::InvalidId)
    );

    let mut s = position(2);
    let path = find_path(&s, 1, &HashSet::new());
    put_path(&mut s, 0, &path);
    s.set_turn(0, 2, true).unwrap();
    s.set_hand(0, Hand::new(5, 5, 5, 5, 5)).unwrap();
    s.finish_edit().unwrap();
    for a in [
        Action::BuildRoad { edge: ne },
        Action::BuildSettlement { vertex: nv },
        Action::BuildCity { vertex: nv },
    ] {
        assert_eq!(s.check(a), Err(RuleError::InvalidId), "{a:?}");
    }
}

#[test]
fn forced_chance_outcomes_must_be_possible() {
    // Records and the analysis board supply chance outcomes; impossible ones are refused.
    let mut s = position(3);
    let (h, v, _) = hex_with_two_spots(&s, Resource::Ore);
    s.put_settlement(1, v).unwrap();
    s.set_hand(1, Hand::new(1, 0, 0, 0, 0)).unwrap();
    s.set_hand(0, cost::DEV_CARD).unwrap();
    s.give_dev_card(2, DevCard::Monopoly, false).unwrap();
    s.give_dev_card(2, DevCard::Monopoly, false).unwrap();
    s.finish_edit().unwrap();
    for (a, b) in [(0, 3), (3, 0), (7, 1), (1, 7)] {
        assert_eq!(
            s.apply_forced(Action::RollDice, Some(Forced::Dice(a, b))),
            Err(RuleError::InvalidForcedOutcome),
            "dice {a} and {b}"
        );
    }
    assert_eq!(
        s.apply_forced(Action::RollDice, Some(Forced::Steal(Resource::Wood))),
        Err(RuleError::InvalidForcedOutcome),
        "an outcome of the wrong kind"
    );
    assert_eq!(s.phase, Phase::PreRoll);
    s.apply_forced(Action::RollDice, Some(Forced::Dice(3, 4)))
        .unwrap();
    let rob = Action::MoveRobber {
        hex: h,
        victim: Some(1),
    };
    assert_eq!(
        s.apply_forced(rob, Some(Forced::Steal(Resource::Brick))),
        Err(RuleError::InvalidForcedOutcome),
        "the victim holds no brick"
    );
    s.apply_forced(rob, Some(Forced::Steal(Resource::Wood)))
        .unwrap();
    assert_eq!(s.player(0).hand.get(Resource::Wood), 1);
    assert_eq!(
        s.apply_forced(Action::BuyDevCard, Some(Forced::DevCard(DevCard::Monopoly))),
        Err(RuleError::InvalidForcedOutcome),
        "both monopolies are out of the deck"
    );
    s.apply_forced(Action::BuyDevCard, Some(Forced::DevCard(DevCard::Knight)))
        .unwrap();
}

#[test]
fn apply_legal_matches_apply() {
    let config = GameConfig::official(4);
    let (mut a, mut b) = (GameState::new(&config, 8), GameState::new(&config, 8));
    let (mut rng, mut guard, limits) = (Rng::seed(2), TurnGuard::default(), Limits::default());
    let mut buf = Vec::new();
    // Replays are checked by this hash, so different positions must hash differently.
    let mut hashes = HashSet::new();
    let mut steps = 0;
    while !a.is_over() && a.turn <= limits.max_turns {
        a.legal_actions(&mut buf);
        let action = guard.choose(RandomPolicy::Builder, &limits, &a, &buf, &mut rng);
        assert_eq!(a.apply(action), Ok(b.apply_legal(action)));
        assert_eq!(a.position_hash(), b.position_hash());
        hashes.insert(a.position_hash());
        steps += 1;
    }
    assert!(
        hashes.len() * 10 >= steps * 9,
        "{} hashes for {steps} positions",
        hashes.len()
    );
    assert!(a.turn > 10);
}

#[test]
fn roads_cannot_pass_through_an_opponents_settlement() {
    let mut s = position(2);
    let path = find_path(&s, 2, &HashSet::new());
    put_path(&mut s, 0, &path);
    s.put_settlement(1, path[2]).unwrap();
    s.finish_edit().unwrap();
    for &n in &neighbors(&s, path[2]) {
        if n != path[1] {
            assert!(
                !s.can_road(0, edge(&s, path[2], n)),
                "road through opponent settlement"
            );
        }
    }
    for &n in &neighbors(&s, path[1]) {
        if n != path[0] && n != path[2] {
            assert!(s.can_road(0, edge(&s, path[1], n)));
        }
    }
}

#[test]
fn longest_road_needs_five_and_strictly_more_to_change_hands() {
    let mut s = position(2);
    let a = find_path(&s, 5, &HashSet::new());
    put_path(&mut s, 0, &a);
    let b = find_path(&s, 6, &around(&s, &a));
    s.put_settlement(1, b[0]).unwrap();
    for w in b[..5].windows(2) {
        let e = edge(&s, w[0], w[1]);
        s.put_road(1, e).unwrap();
    }
    s.finish_edit().unwrap();
    assert_eq!(s.longest_road, Some(0));
    assert_eq!(s.player(1).road_length, 4);

    // Player 1 ties at 5: player 0 keeps it. A sixth road takes it.
    s.set_turn(1, 2, true).unwrap();
    s.set_hand(1, Hand::new(2, 2, 0, 0, 0)).unwrap();
    s.apply(Action::BuildRoad {
        edge: edge(&s, b[4], b[5]),
    })
    .unwrap();
    assert_eq!((s.player(1).road_length, s.longest_road), (5, Some(0)));
    s.apply(Action::BuildRoad {
        edge: edge(&s, b[5], b[6]),
    })
    .unwrap();
    assert_eq!((s.player(1).road_length, s.longest_road), (6, Some(1)));
    s.check_invariants().unwrap();
}

/// Player 0 has a 7-road path; player 2 builds a settlement in its middle during play.
fn cut_scenario() -> (GameState, Vec<VertexId>, Vec<VertexId>) {
    let mut s = position(3);
    let a = find_path(&s, 7, &HashSet::new());
    put_path(&mut s, 0, &a);
    // Player 2 approaches a[3] from outside the path: u - w - a[3].
    let (w, u) = neighbors(&s, a[3])
        .into_iter()
        .filter(|w| !a.contains(w))
        .find_map(|w| {
            neighbors(&s, w)
                .into_iter()
                .find(|&u| u != a[3] && !a.contains(&u) && s.vertex_open(u))
                .map(|u| (w, u))
        })
        .expect("approach to the path");
    s.put_settlement(2, u).unwrap();
    s.put_road(2, edge(&s, u, w)).unwrap();
    s.put_road(2, edge(&s, w, a[3])).unwrap();
    let mut taken: Vec<VertexId> = a.clone();
    taken.extend([w, u]);
    let b = find_path(&s, 5, &around(&s, &taken));
    put_path(&mut s, 1, &b);
    s.finish_edit().unwrap();
    assert_eq!(s.longest_road, Some(0));
    (s, a, b)
}

#[test]
fn cutting_the_longest_road_passes_it_to_a_unique_new_leader() {
    let (mut s, a, _) = cut_scenario();
    s.set_turn(2, 5, true).unwrap();
    s.set_hand(2, Hand::new(1, 1, 1, 1, 0)).unwrap();
    s.apply(Action::BuildSettlement { vertex: a[3] }).unwrap();
    assert_eq!(s.player(0).road_length, 4);
    assert_eq!(s.longest_road, Some(1));
    s.check_invariants().unwrap();
}

#[test]
fn cut_holder_keeps_the_award_while_still_tied_for_longest() {
    let mut s = position(3);
    let a = find_path(&s, 7, &HashSet::new());
    put_path(&mut s, 0, &a);
    let b = find_path(&s, 5, &around(&s, &a));
    put_path(&mut s, 1, &b);
    s.finish_edit().unwrap();
    assert_eq!(s.longest_road, Some(0));
    // Cut at a[5]: segments of 5 and 2, tied with player 1's 5.
    s.put_settlement(2, a[5]).unwrap();
    s.finish_edit().unwrap();
    assert_eq!(s.player(0).road_length, 5);
    assert_eq!(s.longest_road, Some(0));
}

#[test]
fn a_holder_cut_below_five_loses_the_award_even_while_tied() {
    // RULES.md, Longest Road: the 5-road minimum holds for the holder too (found by the
    // reference model; JSettlers2 agrees).
    let mut s = position(3);
    let a = find_path(&s, 6, &HashSet::new());
    put_path(&mut s, 0, &a);
    let b = find_path(&s, 4, &around(&s, &a));
    put_path(&mut s, 1, &b);
    s.finish_edit().unwrap();
    assert_eq!(s.longest_road, Some(0));
    // Cut at a[4]: segments of 4 and 2, tied with player 1's 4.
    s.put_settlement(2, a[4]).unwrap();
    s.finish_edit().unwrap();
    assert_eq!((s.player(0).road_length, s.player(1).road_length), (4, 4));
    assert_eq!(s.longest_road, None);
}

#[test]
fn award_is_set_aside_when_the_cut_leaves_a_tie_between_others() {
    let mut s = position(4);
    let a = find_path(&s, 7, &HashSet::new());
    put_path(&mut s, 0, &a);
    let b = find_path(&s, 5, &around(&s, &a));
    put_path(&mut s, 1, &b);
    let mut taken = a.clone();
    taken.extend(&b);
    let c = find_path(&s, 5, &around(&s, &taken));
    put_path(&mut s, 2, &c);
    s.finish_edit().unwrap();
    assert_eq!(s.longest_road, Some(0));
    s.put_settlement(3, a[3]).unwrap();
    s.finish_edit().unwrap();
    assert_eq!(s.player(0).road_length, 4);
    assert_eq!(s.longest_road, None);
}

#[test]
fn largest_army_needs_three_and_strictly_more_to_change_hands() {
    let mut s = position(2);
    for _ in 0..3 {
        s.add_played_card(0, DevCard::Knight).unwrap();
        s.add_played_card(1, DevCard::Knight).unwrap();
    }
    s.set_awards(None, Some(0)).unwrap();
    s.finish_edit().unwrap();
    assert_eq!(s.largest_army, Some(0));
    s.set_turn(1, 4, false).unwrap();
    s.give_dev_card(1, DevCard::Knight, false).unwrap();
    s.apply(Action::PlayKnight).unwrap();
    assert_eq!(s.largest_army, Some(1));
    assert!(matches!(s.phase, Phase::MoveRobber { .. }));
}

#[test]
fn tied_knights_without_a_holder_must_be_resolved_explicitly() {
    let mut s = position(2);
    for _ in 0..3 {
        s.add_played_card(0, DevCard::Knight).unwrap();
        s.add_played_card(1, DevCard::Knight).unwrap();
    }
    assert!(s.finish_edit().is_err());
}

#[test]
fn development_card_timing() {
    let mut s = position(2);
    s.give_dev_card(0, DevCard::Knight, true).unwrap();
    s.finish_edit().unwrap();
    assert_eq!(s.check(Action::PlayKnight), Err(RuleError::NoCard));

    // Two playable cards, but only one play per turn.
    let mut s = position(2);
    s.give_dev_card(0, DevCard::Knight, false).unwrap();
    s.give_dev_card(0, DevCard::Monopoly, false).unwrap();
    s.finish_edit().unwrap();
    s.apply(Action::PlayKnight).unwrap();
    let hex = (0..19).find(|&h| h != s.robber).unwrap();
    s.apply(Action::MoveRobber { hex, victim: None }).unwrap();
    assert_eq!(
        s.phase,
        Phase::PreRoll,
        "a knight before rolling returns to the roll"
    );
    s.apply_forced(Action::RollDice, Some(Forced::Dice(2, 3)))
        .unwrap();
    assert_eq!(
        s.check(Action::PlayMonopoly {
            resource: Resource::Ore
        }),
        Err(RuleError::DevCardAlreadyPlayed)
    );

    // With the knight-only variant, progress cards wait for the roll.
    let mut rules = GameConfig::official(2).rules;
    rules.dev_timing = rules::DevTiming::KnightBeforeRoll;
    let mut s = GameState::empty_position(rules, board(1), 1);
    s.give_dev_card(0, DevCard::Monopoly, false).unwrap();
    s.finish_edit().unwrap();
    assert_eq!(
        s.check(Action::PlayMonopoly {
            resource: Resource::Ore
        }),
        Err(RuleError::DevCardNotAllowedNow)
    );
    let monopoly_listed = |s: &GameState| {
        legal(s)
            .iter()
            .any(|a| matches!(a, Action::PlayMonopoly { .. }))
    };
    assert!(!monopoly_listed(&s));
    s.apply_forced(Action::RollDice, Some(Forced::Dice(2, 3)))
        .unwrap();
    assert!(monopoly_listed(&s), "after the roll it is");
}

#[test]
fn bought_cards_become_playable_next_turn() {
    let mut s = position(2);
    s.set_turn(0, 3, true).unwrap();
    s.set_hand(0, cost::DEV_CARD).unwrap();
    s.finish_edit().unwrap();
    s.apply_forced(Action::BuyDevCard, Some(Forced::DevCard(DevCard::Knight)))
        .unwrap();
    assert_eq!(s.check(Action::PlayKnight), Err(RuleError::NoCard));
    s.apply(Action::EndTurn).unwrap();
    s.apply_forced(Action::RollDice, Some(Forced::Dice(2, 3)))
        .unwrap();
    s.apply(Action::EndTurn).unwrap();
    assert_eq!(s.current, 0);
    assert_eq!(s.check(Action::PlayKnight), Ok(()));
}

#[test]
fn a_victory_point_card_wins_immediately_on_your_turn() {
    let mut s = position(2);
    let spots = spread_vertices(&s, 5, &HashSet::new());
    for &v in &spots[..4] {
        s.put_city(0, v).unwrap();
    }
    s.put_settlement(0, spots[4]).unwrap();
    s.set_turn(0, 20, true).unwrap();
    s.set_hand(0, cost::DEV_CARD).unwrap();
    s.finish_edit().unwrap();
    assert_eq!(s.total_vp(0), 9);
    s.apply_forced(
        Action::BuyDevCard,
        Some(Forced::DevCard(DevCard::VictoryPoint)),
    )
    .unwrap();
    assert_eq!((s.winner, s.phase), (Some(0), Phase::GameOver));
    // Nothing more can happen.
    assert!(legal(&s).is_empty());
    for a in [
        Action::EndTurn,
        Action::RollDice,
        Action::BuyDevCard,
        Action::PlayKnight,
        Action::BuildRoad { edge: 0 },
    ] {
        assert_eq!(s.check(a), Err(RuleError::GameOver), "{a:?}");
    }
}

#[test]
fn points_reached_on_someone_elses_turn_win_when_your_turn_starts() {
    let mut s = position(2);
    let spots = spread_vertices(&s, 5, &HashSet::new());
    for &v in &spots[..4] {
        s.put_city(0, v).unwrap();
    }
    s.put_settlement(0, spots[4]).unwrap();
    s.give_dev_card(0, DevCard::VictoryPoint, false).unwrap();
    s.set_turn(1, 20, true).unwrap();
    s.finish_edit().unwrap();
    assert_eq!(s.total_vp(0), 10);
    assert_eq!(s.winner, None, "no win on another player's turn");
    s.apply(Action::EndTurn).unwrap();
    assert_eq!(s.winner, Some(0));
}

/// A hex of `resource` (not under the robber) and two non-adjacent corners of it that touch no
/// other hex with the same number, so production on that number comes only from this hex.
fn hex_with_two_spots(s: &GameState, resource: Resource) -> (HexId, VertexId, VertexId) {
    let topo = &s.board.topology;
    for h in 0..topo.num_hexes() {
        let tile = s.board.tiles[h];
        if tile.resource != Some(resource) || h as HexId == s.robber {
            continue;
        }
        let clean = |v: VertexId| {
            topo.vertex_hexes[v as usize]
                .as_slice()
                .iter()
                .all(|&o| o as usize == h || s.board.tiles[o as usize].number != tile.number)
        };
        let corners = topo.hex_vertices[h];
        for k in 0..6 {
            let (a, b) = (corners[k], corners[(k + 2) % 6]);
            if clean(a) && clean(b) {
                return (h as HexId, a, b);
            }
        }
    }
    panic!("no suitable {resource:?} hex");
}

#[test]
fn robber_must_steal_when_it_can_and_only_from_players_with_cards() {
    let mut s = position(3);
    let (h, v1, v2) = hex_with_two_spots(&s, Resource::Ore);
    s.put_settlement(1, v1).unwrap();
    s.put_settlement(2, v2).unwrap();
    s.set_hand(1, Hand::new(0, 0, 0, 1, 0)).unwrap();
    s.finish_edit().unwrap();
    s.apply_forced(Action::RollDice, Some(Forced::Dice(3, 4)))
        .unwrap();
    assert_eq!(
        s.check(Action::MoveRobber {
            hex: h,
            victim: None
        }),
        Err(RuleError::InvalidVictim)
    );
    assert_eq!(
        s.check(Action::MoveRobber {
            hex: h,
            victim: Some(2)
        }),
        Err(RuleError::InvalidVictim),
        "player 2 has no cards"
    );
    let out = s
        .apply(Action::MoveRobber {
            hex: h,
            victim: Some(1),
        })
        .unwrap();
    assert_eq!(
        out,
        Outcome::Stole {
            resource: Some(Resource::Wheat)
        }
    );
    assert_eq!(s.player(0).hand, Hand::new(0, 0, 0, 1, 0));
    assert_eq!(s.robber, h);
}

#[test]
fn friendly_robber_protects_players_with_two_points_or_fewer() {
    let config = GameConfig::colonist_1v1();
    let mut s = GameState::empty_position(config.rules, board(1), 5);
    let (h, v1, _) = hex_with_two_spots(&s, Resource::Wheat);
    s.put_settlement(1, v1).unwrap();
    let far = spread_vertices(
        &s,
        1,
        &around(&s, &s.board.topology.hex_vertices[h as usize]),
    );
    s.put_settlement(1, far[0]).unwrap();
    s.set_hand(1, Hand::new(1, 0, 0, 0, 0)).unwrap();
    // Player 0's own settlement protects nothing.
    let topo = s.board.topology.clone();
    let hexes_of = |v: VertexId| topo.vertex_hexes[v as usize].as_slice().to_vec();
    let protected: HashSet<HexId> = [v1, far[0]].into_iter().flat_map(hexes_of).collect();
    let theirs = around(&s, &[v1, far[0]]);
    let mine = (0..topo.num_vertices() as VertexId)
        .find(|v| {
            !theirs.contains(v)
                && hexes_of(*v)
                    .iter()
                    .any(|h| !protected.contains(h) && *h != s.robber)
        })
        .unwrap();
    s.put_settlement(0, mine).unwrap();
    s.finish_edit().unwrap();
    s.apply_forced(Action::RollDice, Some(Forced::Dice(3, 4)))
        .unwrap();
    assert_eq!(s.visible_vp(1), 2);
    assert_eq!(
        s.check(Action::MoveRobber {
            hex: h,
            victim: Some(1)
        }),
        Err(RuleError::InvalidRobberHex)
    );
    // Open: exactly the other hexes that touch none of player 1's buildings.
    let want: HashSet<HexId> = (0..topo.num_hexes() as HexId)
        .filter(|h| *h != s.robber && !protected.contains(h))
        .collect();
    let open: HashSet<HexId> = legal(&s)
        .iter()
        .filter_map(|a| match a {
            Action::MoveRobber { hex, .. } => Some(*hex),
            _ => None,
        })
        .collect();
    assert_eq!(open, want);
}

#[test]
fn sevens_make_big_hands_discard_half() {
    let mut s = position(3);
    s.set_hand(0, Hand::new(2, 2, 2, 2, 0)).unwrap(); // 8 cards: discard 4
    s.set_hand(1, Hand::new(2, 2, 2, 1, 0)).unwrap(); // 7 cards: keep
    s.set_hand(2, Hand::new(3, 3, 3, 0, 0)).unwrap(); // 9 cards: discard 4
    s.finish_edit().unwrap();
    s.apply_forced(Action::RollDice, Some(Forced::Dice(6, 1)))
        .unwrap();
    assert_eq!(s.phase, Phase::Discard);
    assert_eq!(s.discard_pending[..3], [4, 0, 4]);
    assert_eq!(s.acting_player(), 0);
    let mut guard = 0;
    while s.phase == Phase::Discard {
        let a = legal(&s)[0];
        s.apply(a).unwrap();
        guard += 1;
        assert!(guard < 10);
    }
    assert_eq!(s.player(0).hand.total(), 4);
    assert_eq!(s.player(1).hand.total(), 7);
    assert_eq!(s.player(2).hand.total(), 5);
    assert!(matches!(s.phase, Phase::MoveRobber { .. }));

    // The 1v1 preset only discards above 9.
    let mut s = GameState::empty_position(GameConfig::colonist_1v1().rules, board(1), 5);
    s.set_hand(1, Hand::new(3, 3, 3, 0, 0)).unwrap();
    s.finish_edit().unwrap();
    s.apply_forced(Action::RollDice, Some(Forced::Dice(6, 1)))
        .unwrap();
    assert!(matches!(s.phase, Phase::MoveRobber { .. }));
}

#[test]
fn bank_shortages_follow_the_official_rule() {
    // Two players owed wheat with one left: nobody gets any.
    let mut s = position(3);
    let (h, v1, v2) = hex_with_two_spots(&s, Resource::Wheat);
    let number = s.board.tiles[h as usize].number.unwrap();
    s.put_settlement(0, v1).unwrap();
    s.put_settlement(1, v2).unwrap();
    s.set_hand(2, Hand::new(0, 0, 0, 18, 0)).unwrap();
    s.finish_edit().unwrap();
    let gains = s.production_gains(number);
    assert_eq!(
        (gains[0].get(Resource::Wheat), gains[1].get(Resource::Wheat)),
        (0, 0)
    );

    // One player owed two (a city) with one left: they take what's left.
    let mut s = position(3);
    s.put_city(0, v1).unwrap();
    s.set_hand(2, Hand::new(0, 0, 0, 18, 0)).unwrap();
    s.finish_edit().unwrap();
    assert_eq!(s.production_gains(number)[0].get(Resource::Wheat), 1);

    // Plenty in the bank: cities collect two, and the robber blocks the hex.
    let mut s = position(3);
    s.put_city(0, v1).unwrap();
    s.put_settlement(1, v2).unwrap();
    s.finish_edit().unwrap();
    let gains = s.production_gains(number);
    assert_eq!(
        (gains[0].get(Resource::Wheat), gains[1].get(Resource::Wheat)),
        (2, 1)
    );
    s.set_robber(h).unwrap();
    assert!(s.production_gains(number)[0].is_empty());

    // The Catanatron rule: nobody gets it, even when only one player is owed it.
    let mut rules = GameConfig::official(3).rules;
    rules.shortage = ShortageRule::NobodyGets;
    let mut s = GameState::empty_position(rules, board(1), 99);
    s.put_city(0, v1).unwrap();
    s.set_hand(2, Hand::new(0, 0, 0, 18, 0)).unwrap();
    s.finish_edit().unwrap();
    assert_eq!(s.production_gains(number)[0].get(Resource::Wheat), 0);
}

#[test]
fn ports_set_bank_trade_ratios() {
    let mut s = position(2);
    assert_eq!(s.player(0).trade_ratio(Resource::Wood), 4);
    let ports = s.board.ports.clone();
    let generic = ports
        .iter()
        .find(|(p, _)| p.kind == Port::Generic)
        .unwrap()
        .1[0];
    s.put_settlement(0, generic).unwrap();
    assert_eq!(s.player(0).trade_ratio(Resource::Wood), 3);

    let mut s = position(2);
    let (spot, vs) = ports.iter().find(|(p, _)| p.kind != Port::Generic).unwrap();
    let Port::Special(r) = spot.kind else {
        unreachable!()
    };
    s.put_settlement(1, vs[0]).unwrap();
    assert_eq!(s.player(1).trade_ratio(r), 2);
    let other = Resource::ALL.into_iter().find(|&o| o != r).unwrap();
    assert_eq!(s.player(1).trade_ratio(other), 4);

    // Bank trades use the best ratio.
    s.set_turn(1, 2, true).unwrap();
    s.set_hand(1, Hand::of(r, 2)).unwrap();
    s.finish_edit().unwrap();
    s.apply(Action::MaritimeTrade {
        give: r,
        get: other,
    })
    .unwrap();
    assert_eq!(s.player(1).hand, Hand::of(other, 1));
}

/// The flow without `open_trading` (every version before 2026-11-12): only the player on turn
/// offers; the others accept or decline.
#[test]
fn player_trade_flow() {
    let mut s = position(3);
    s.rules.open_trading = false;
    s.set_turn(0, 2, true).unwrap();
    s.set_hand(0, Hand::new(1, 0, 0, 0, 0)).unwrap();
    s.set_hand(1, Hand::new(0, 0, 0, 0, 0)).unwrap();
    s.set_hand(2, Hand::new(0, 1, 0, 0, 0)).unwrap();
    s.finish_edit().unwrap();
    let like_for_like = Action::OfferTrade {
        give: Hand::new(1, 0, 0, 0, 0),
        get: Hand::new(1, 0, 0, 0, 0),
    };
    assert_eq!(s.check(like_for_like), Err(RuleError::InvalidTrade));
    let gift = Action::OfferTrade {
        give: Hand::new(1, 0, 0, 0, 0),
        get: Hand::EMPTY,
    };
    assert_eq!(s.check(gift), Err(RuleError::InvalidTrade));

    let offer = Action::OfferTrade {
        give: Hand::new(1, 0, 0, 0, 0),
        get: Hand::new(0, 1, 0, 0, 0),
    };
    s.apply(offer).unwrap();
    assert_eq!(s.phase, Phase::TradeResponse);
    assert_eq!(s.acting_player(), 2, "player 1 can't pay and is skipped");
    s.apply(Action::AcceptTrade).unwrap();
    assert_eq!(s.phase, Phase::TradeConfirm);
    s.apply(Action::ConfirmTrade { with: 2 }).unwrap();
    assert_eq!(s.player(0).hand, Hand::new(0, 1, 0, 0, 0));
    assert_eq!(s.player(2).hand, Hand::new(1, 0, 0, 0, 0));
    assert_eq!(s.phase, Phase::Main);

    // An offer nobody accepts lapses at once, with no confirm step; one nobody can pay asks
    // nobody. Both count toward the per-turn cap (3, the third offer here).
    assert_eq!(s.rules.max_offers_per_turn, 3);
    s.apply(Action::OfferTrade {
        give: Hand::new(0, 1, 0, 0, 0),
        get: Hand::new(1, 0, 0, 0, 0),
    })
    .unwrap();
    assert_eq!(s.acting_player(), 2);
    s.apply(Action::RejectTrade).unwrap();
    assert_eq!(s.phase, Phase::Main);
    s.apply(Action::OfferTrade {
        give: Hand::new(0, 1, 0, 0, 0),
        get: Hand::new(0, 0, 0, 0, 1),
    })
    .unwrap();
    assert_eq!(s.phase, Phase::Main, "nobody holds ore");
    assert_eq!(
        s.check(Action::OfferTrade {
            give: Hand::new(0, 1, 0, 0, 0),
            get: Hand::new(1, 0, 0, 0, 0),
        }),
        Err(RuleError::OfferLimitReached)
    );
}

/// Trading as the rulebook has it (`open_trading`, 5th edition "Domestic Trade"), with the round of
/// 2026-11-12 (`trade_proposals`, the preset `official-proposal-round`): after the roll the others
/// may propose trades to the player on turn, and an offer may be answered with other terms; the
/// player on turn takes one or none.
#[test]
fn open_trading_flow() {
    use Resource::*;
    let mut s = position(4);
    s.rules.trade_proposals = true;
    assert!(s.rules.open_trading);
    s.set_turn(0, 2, false).unwrap();
    s.set_hand(0, Hand::new(2, 0, 0, 1, 0)).unwrap();
    s.set_hand(1, Hand::EMPTY).unwrap();
    s.set_hand(2, Hand::new(0, 2, 0, 0, 0)).unwrap();
    s.set_hand(3, Hand::new(0, 0, 1, 0, 1)).unwrap();
    s.finish_edit().unwrap();
    let propose = |give: Hand, get: Hand| Action::ProposeTrade { give, get };

    // The round after the roll: player 1 holds nothing and is skipped; 2 and 3 may propose.
    s.apply_forced(Action::RollDice, Some(Forced::Dice(1, 1)))
        .unwrap();
    assert_eq!((s.phase, s.acting_player()), (Phase::TradeResponse, 2));
    assert!(s.trade.unwrap().is_round());
    assert!(
        !legal(&s).contains(&Action::AcceptTrade),
        "nothing to accept in a round"
    );
    // Any cards held for any others; not like for like, not a gift, not cards it lacks.
    assert_eq!(
        s.check(propose(Hand::of(Brick, 1), Hand::of(Brick, 1))),
        Err(RuleError::InvalidTrade)
    );
    assert_eq!(
        s.check(propose(Hand::of(Brick, 1), Hand::EMPTY)),
        Err(RuleError::InvalidTrade)
    );
    assert_eq!(
        s.check(propose(Hand::of(Brick, 3), Hand::of(Wood, 1))),
        Err(RuleError::CannotAfford)
    );
    s.apply(propose(Hand::of(Brick, 2), Hand::of(Wood, 1)))
        .unwrap();
    assert_eq!(s.acting_player(), 3);
    // Asks for ore, which player 0 lacks: it can't be taken.
    s.apply(propose(Hand::of(Sheep, 1), Hand::of(Ore, 1)))
        .unwrap();
    assert_eq!((s.phase, s.acting_player()), (Phase::TradeConfirm, 0));
    assert_eq!(
        legal(&s),
        vec![Action::ConfirmTrade { with: 2 }, Action::CancelTrade]
    );
    assert_eq!(
        s.check(Action::ConfirmTrade { with: 3 }),
        Err(RuleError::CannotAfford)
    );
    assert_eq!(
        s.check(Action::ConfirmTrade { with: 1 }),
        Err(RuleError::InvalidTrade)
    );
    s.apply(Action::ConfirmTrade { with: 2 }).unwrap();
    assert_eq!(s.player(0).hand, Hand::new(1, 2, 0, 1, 0));
    assert_eq!(s.player(2).hand, Hand::new(1, 0, 0, 0, 0));
    assert_eq!(s.phase, Phase::Main);

    // An offer: player 2 can't pay but counters, player 3 accepts.
    s.apply(Action::OfferTrade {
        give: Hand::of(Wheat, 1),
        get: Hand::of(Ore, 1),
    })
    .unwrap();
    assert_eq!(
        s.acting_player(),
        2,
        "player 1 has no cards; player 2 may counter"
    );
    assert!(!legal(&s).contains(&Action::AcceptTrade));
    s.apply(propose(Hand::of(Wood, 1), Hand::of(Brick, 2)))
        .unwrap();
    s.apply(Action::AcceptTrade).unwrap();
    assert_eq!(s.phase, Phase::TradeConfirm);
    assert_eq!(
        legal(&s),
        vec![
            Action::ConfirmTrade { with: 2 },
            Action::ConfirmTrade { with: 3 },
            Action::CancelTrade
        ]
    );
    // Taking the counter trades on its terms, not the offer's.
    let mut countered = s.clone();
    countered.apply(Action::ConfirmTrade { with: 2 }).unwrap();
    assert_eq!(countered.player(0).hand, Hand::new(2, 0, 0, 1, 0));
    assert_eq!(countered.player(2).hand, Hand::new(0, 2, 0, 0, 0));
    s.apply(Action::ConfirmTrade { with: 3 }).unwrap();
    assert_eq!(s.player(0).hand, Hand::new(1, 2, 0, 0, 1));
    assert_eq!(s.player(3).hand, Hand::new(0, 0, 1, 1, 0));

    // A round happens once per turn, and only after a roll: none on the next turn's knight.
    assert_eq!(
        s.offers_this_turn, 1,
        "proposals don't count toward the cap"
    );
    s.apply(Action::EndTurn).unwrap();
    assert_eq!(s.phase, Phase::PreRoll);
}

/// The official rules since 2026-11-13 (`trade_proposals` off, the owner's ruling): no round after
/// the roll, a 7 included; the others propose only as counter-offers to the player on turn's offer.
#[test]
fn official_counter_offers_answer_an_offer_and_nothing_else() {
    use Resource::*;
    let mut s = position(4);
    assert!(s.rules.open_trading && !s.rules.trade_proposals);
    s.set_turn(0, 2, false).unwrap();
    s.set_hand(0, Hand::new(2, 0, 0, 1, 0)).unwrap();
    s.set_hand(1, Hand::EMPTY).unwrap();
    s.set_hand(2, Hand::new(0, 2, 0, 0, 0)).unwrap();
    s.set_hand(3, Hand::new(0, 0, 1, 0, 1)).unwrap();
    s.finish_edit().unwrap();
    let after_7 = s.clone();
    s.apply_forced(Action::RollDice, Some(Forced::Dice(1, 1)))
        .unwrap();
    assert_eq!(
        (s.phase, s.acting_player()),
        (Phase::Main, 0),
        "no round after the roll"
    );
    assert!(s.trade.is_none());
    // An offer: player 2 can't pay but counters, player 3 accepts, and the counter is taken.
    s.apply(Action::OfferTrade {
        give: Hand::of(Wheat, 1),
        get: Hand::of(Ore, 1),
    })
    .unwrap();
    assert_eq!(s.acting_player(), 2);
    s.apply(Action::ProposeTrade {
        give: Hand::of(Brick, 1),
        get: Hand::of(Wood, 1),
    })
    .unwrap();
    s.apply(Action::AcceptTrade).unwrap();
    assert_eq!(s.phase, Phase::TradeConfirm);
    s.apply(Action::ConfirmTrade { with: 2 }).unwrap();
    assert_eq!(s.player(0).hand, Hand::new(1, 1, 0, 1, 0));
    assert_eq!(s.player(2).hand, Hand::new(1, 1, 0, 0, 0));
    assert_eq!(s.phase, Phase::Main);

    // After a 7, play goes straight on once the robber has moved.
    let mut s = after_7;
    s.apply_forced(Action::RollDice, Some(Forced::Dice(3, 4)))
        .unwrap();
    assert!(matches!(s.phase, Phase::MoveRobber { .. }));
    // Nobody has built yet, so the robber steals from no one.
    let robber = legal(&s)[0];
    assert!(matches!(robber, Action::MoveRobber { victim: None, .. }));
    s.apply(robber).unwrap();
    assert_eq!(s.phase, Phase::Main);
    assert!(s.trade.is_none());
}

#[test]
fn open_trading_round_comes_after_the_robber_and_needs_cards() {
    use Resource::*;
    let mut s = position(3);
    s.rules.trade_proposals = true;
    s.set_turn(0, 2, false).unwrap();
    s.set_hand(0, Hand::EMPTY).unwrap();
    s.set_hand(1, Hand::new(1, 0, 0, 0, 0)).unwrap();
    s.set_hand(2, Hand::new(0, 1, 0, 0, 0)).unwrap();
    s.finish_edit().unwrap();
    // Player 0 holds nothing, so nobody can propose anything it could pay: no round.
    s.apply_forced(Action::RollDice, Some(Forced::Dice(1, 1)))
        .unwrap();
    assert_eq!(s.phase, Phase::Main);
    assert!(s.trade.is_none());

    // After a 7 the round comes once the robber has stolen, with the stolen card in hand.
    let mut s = position(3);
    s.rules.trade_proposals = true;
    let (h, v, _) = hex_with_two_spots(&s, Ore);
    s.put_settlement(1, v).unwrap();
    s.set_turn(0, 2, false).unwrap();
    s.set_hand(0, Hand::EMPTY).unwrap();
    s.set_hand(1, Hand::new(1, 0, 0, 0, 0)).unwrap();
    s.set_hand(2, Hand::new(0, 1, 0, 0, 0)).unwrap();
    s.finish_edit().unwrap();
    s.apply_forced(Action::RollDice, Some(Forced::Dice(3, 4)))
        .unwrap();
    assert!(matches!(s.phase, Phase::MoveRobber { .. }));
    let robber = Action::MoveRobber {
        hex: h,
        victim: Some(1),
    };
    s.apply_forced(robber, Some(Forced::Steal(Wood))).unwrap();
    assert_eq!(s.player(0).hand, Hand::of(Wood, 1));
    assert_eq!(
        (s.phase, s.acting_player()),
        (Phase::TradeResponse, 2),
        "player 1 has no cards left"
    );
    s.apply(Action::RejectTrade).unwrap();
    assert_eq!(s.phase, Phase::Main);

    // A knight played in the main phase moves the robber and goes back to it, with no new round.
    // Without open trading there is never a round.
    let mut s = position(3);
    s.rules.open_trading = false;
    s.set_turn(0, 2, false).unwrap();
    s.set_hand(0, Hand::of(Wood, 1)).unwrap();
    s.set_hand(1, Hand::of(Brick, 1)).unwrap();
    s.finish_edit().unwrap();
    s.apply_forced(Action::RollDice, Some(Forced::Dice(1, 1)))
        .unwrap();
    assert_eq!(s.phase, Phase::Main);
    s.apply(Action::OfferTrade {
        give: Hand::of(Wood, 1),
        get: Hand::of(Brick, 1),
    })
    .unwrap();
    assert_eq!(
        s.check(Action::ProposeTrade {
            give: Hand::of(Brick, 1),
            get: Hand::of(Ore, 1)
        }),
        Err(RuleError::TradingDisabled)
    );
}

#[test]
fn monopoly_and_year_of_plenty() {
    let mut s = position(3);
    s.set_turn(0, 2, true).unwrap();
    s.set_hand(1, Hand::new(0, 0, 3, 0, 1)).unwrap();
    s.set_hand(2, Hand::new(0, 0, 2, 0, 0)).unwrap();
    s.give_dev_card(0, DevCard::Monopoly, false).unwrap();
    s.finish_edit().unwrap();
    let out = s
        .apply(Action::PlayMonopoly {
            resource: Resource::Sheep,
        })
        .unwrap();
    assert_eq!(
        out,
        Outcome::Monopoly {
            taken: [0, 3, 2, 0, 0, 0]
        }
    );
    assert_eq!(s.player(0).hand.get(Resource::Sheep), 5);
    assert_eq!(s.player(1).hand, Hand::new(0, 0, 0, 0, 1));

    let mut s = position(2);
    s.set_turn(0, 2, true).unwrap();
    s.set_hand(1, Hand::new(0, 0, 0, 0, 19)).unwrap();
    s.give_dev_card(0, DevCard::YearOfPlenty, false).unwrap();
    s.finish_edit().unwrap();
    assert_eq!(
        s.check(Action::PlayYearOfPlenty {
            a: Resource::Ore,
            b: Resource::Wood
        }),
        Err(RuleError::BankEmpty)
    );
    s.apply(Action::PlayYearOfPlenty {
        a: Resource::Wood,
        b: Resource::Wood,
    })
    .unwrap();
    assert_eq!(s.player(0).hand, Hand::new(2, 0, 0, 0, 0));
}

#[test]
fn road_building_places_up_to_two_free_roads() {
    let mut s = position(2);
    let path = find_path(&s, 1, &HashSet::new());
    put_path(&mut s, 0, &path);
    s.set_turn(0, 2, true).unwrap();
    s.give_dev_card(0, DevCard::RoadBuilding, false).unwrap();
    s.finish_edit().unwrap();
    s.apply(Action::PlayRoadBuilding).unwrap();
    for _ in 0..2 {
        assert!(matches!(s.phase, Phase::RoadBuilding { .. }));
        // Engine choice: the free roads come before anything else, ending the turn included.
        assert!(
            legal(&s)
                .iter()
                .all(|a| matches!(a, Action::BuildRoad { .. }))
        );
        assert!(s.check(Action::EndTurn).is_err());
        let a = legal(&s)[0];
        s.apply(a).unwrap();
    }
    assert_eq!(s.phase, Phase::Main);
    assert_eq!(s.player(0).roads_on_board(), 3);
    assert!(s.player(0).hand.is_empty(), "free roads cost nothing");
}

#[test]
fn belief_tracking_branches_on_hidden_steals_and_prunes_on_spends() {
    let mut s = position(3);
    let (h, v1, v2) = hex_with_two_spots(&s, Resource::Ore);
    let path = find_path(&s, 1, &around(&s, &[v1, v2]));
    put_path(&mut s, 1, &path);
    s.put_settlement(2, v1).unwrap();
    s.set_hand(1, Hand::new(1, 0, 0, 0, 0)).unwrap();
    s.set_hand(2, Hand::new(1, 1, 0, 0, 1)).unwrap();
    s.set_turn(1, 6, false).unwrap();
    let _ = v2;
    s.finish_edit().unwrap();

    let hands: [Hand; MAX_PLAYERS] = std::array::from_fn(|q| {
        if q < 3 {
            s.player(q as PlayerId).hand
        } else {
            Hand::EMPTY
        }
    });
    let mut belief = BeliefTracker::with_hands(0, 3, hands);
    let step =
        |s: &mut GameState, belief: &mut BeliefTracker, action: Action, forced: Option<Forced>| {
            let pre = s.clone();
            let actor = s.acting_player();
            let out = s.apply_forced(action, forced).unwrap();
            belief.observe(&pre, actor, &action, &out.visible_to(0, actor, &action));
        };
    step(
        &mut s,
        &mut belief,
        Action::RollDice,
        Some(Forced::Dice(3, 4)),
    );
    step(
        &mut s,
        &mut belief,
        Action::MoveRobber {
            hex: h,
            victim: Some(2),
        },
        Some(Forced::Steal(Resource::Brick)),
    );
    assert_eq!(
        belief.num_hypotheses(),
        3,
        "observer 0 didn't see which card moved"
    );
    // The others pass in the round of proposals after the roll.
    while s.phase == Phase::TradeResponse {
        step(&mut s, &mut belief, Action::RejectTrade, None);
    }
    let e = s.road_spots(1).ones().next().unwrap();
    step(&mut s, &mut belief, Action::BuildRoad { edge: e }, None);
    assert_eq!(
        belief.num_hypotheses(),
        1,
        "only a stolen brick allows the road"
    );
    assert_eq!(
        belief.hand_distribution(2),
        vec![(Hand::new(1, 0, 0, 0, 1), 1.0)]
    );
}

#[test]
fn records_round_trip_and_replay_exactly() {
    let config = GameConfig::official(4);
    let mut s = GameState::new(&config, 42);
    let mut record = GameRecord::start(&config, 42, &s);
    let mut rng = Rng::seed(1);
    let mut guard = TurnGuard::default();
    let limits = Limits::default();
    let mut buf = Vec::new();
    while !s.is_over() && s.turn <= limits.max_turns {
        s.legal_actions(&mut buf);
        let a = guard.choose(RandomPolicy::Builder, &limits, &s, &buf, &mut rng);
        record.play(&mut s, a).unwrap();
    }
    record.finish(&s);
    let json = serde_json::to_string(&record).unwrap();
    let back: GameRecord = serde_json::from_str(&json).unwrap();
    let replayed = back.replay().unwrap();
    assert_eq!(replayed.position_hash(), s.position_hash());

    let mut tampered = back.clone();
    tampered.final_hash ^= 1;
    assert!(matches!(tampered.replay(), Err(ReplayError::FinalPosition)));
    let mut tampered = back.clone();
    tampered.winner = None;
    assert!(matches!(tampered.replay(), Err(ReplayError::FinalPosition)));
    let mut tampered = back.clone();
    tampered.format += 1;
    assert!(matches!(tampered.replay(), Err(ReplayError::Format(_))));
    // A roll stored without its dice is rolled afresh, and the result can't match.
    let roll = back
        .steps
        .iter()
        .position(|step| step.action == Action::RollDice)
        .unwrap();
    let mut tampered = back.clone();
    tampered.steps[roll].outcome = Outcome::None;
    assert!(matches!(
        tampered.replay(),
        Err(ReplayError::OutcomeMismatch { step }) if step == roll
    ));

    // A step by the wrong player, or after the end, is caught as such.
    assert!(s.is_over());
    let mut wrong = back.clone();
    wrong.steps[10].actor = (wrong.steps[10].actor + 1) % 4;
    assert!(matches!(
        wrong.replay(),
        Err(ReplayError::WrongActor { step: 10 })
    ));
    let mut extra = back.clone();
    extra.steps.push(extra.steps.last().unwrap().clone());
    let n = back.steps.len();
    assert!(matches!(extra.replay(), Err(ReplayError::WrongActor { step }) if step == n));

    // Steps without a chance outcome are stored without one.
    for step in &record.steps {
        let stored = serde_json::to_value(step).unwrap();
        assert_eq!(
            stored.get("outcome").is_some(),
            step.outcome != Outcome::None,
            "{step:?}"
        );
    }
}

/// A free edge next to `path` (one of whose ends is off the path).
fn edge_off(s: &GameState, path: &[VertexId]) -> EdgeId {
    for &v in path {
        if let Some(w) = neighbors(s, v).into_iter().find(|w| !path.contains(w)) {
            return edge(s, v, w);
        }
    }
    panic!("no free edge next to the path");
}

#[test]
fn builds_cost_what_the_rulebook_says() {
    // Road: wood and brick. Settlement: wood, brick, sheep, wheat. City: two wheat, three ore.
    // Development card: sheep, wheat, ore.
    let setup = |hand: Hand| {
        let mut s = position(2);
        let path = find_path(&s, 2, &HashSet::new());
        s.put_settlement(0, path[0]).unwrap();
        s.put_road(0, edge(&s, path[0], path[1])).unwrap();
        s.set_turn(0, 2, true).unwrap();
        s.set_hand(0, hand).unwrap();
        s.finish_edit().unwrap();
        (s, path)
    };
    let builds = |path: &[VertexId], s: &GameState| {
        [
            (
                Action::BuildRoad {
                    edge: edge(s, path[1], path[2]),
                },
                Hand::new(1, 1, 0, 0, 0),
            ),
            (
                Action::BuildSettlement { vertex: path[2] },
                Hand::new(1, 1, 1, 1, 0),
            ),
            (
                Action::BuildCity { vertex: path[0] },
                Hand::new(0, 0, 0, 2, 3),
            ),
            (Action::BuyDevCard, Hand::new(0, 0, 1, 1, 1)),
        ]
    };
    // Each build takes exactly its cost.
    let (mut s, path) = setup(Hand::new(2, 2, 2, 4, 4));
    for (a, cost) in builds(&path, &s) {
        let mut expected = s.player(0).hand;
        expected.sub(&cost);
        s.apply(a).unwrap();
        assert_eq!(s.player(0).hand, expected, "{a:?}");
    }
    assert!(s.player(0).hand.is_empty());
    // One card short of any part of a cost, and the build is refused.
    let (s, path) = setup(Hand::new(0, 0, 0, 0, 0));
    for (a, cost) in builds(&path, &s) {
        for r in Resource::ALL {
            if cost.get(r) == 0 {
                continue;
            }
            let mut short = cost;
            short.sub(&Hand::of(r, 1));
            let (mut t, _) = setup(short);
            if matches!(a, Action::BuildSettlement { .. }) {
                // The settlement needs the road first: lay it in the editor.
                t.put_road(0, edge(&t, path[1], path[2])).unwrap();
                t.finish_edit().unwrap();
            }
            assert_eq!(
                t.check(a),
                Err(RuleError::CannotAfford),
                "{a:?} short of {r:?}"
            );
        }
    }
}

#[test]
fn piece_limits_stop_building_and_a_city_returns_its_settlement() {
    // 15 roads: a 16th is refused even with the cards for it.
    let mut s = position(2);
    let path = find_path(&s, 15, &HashSet::new());
    put_path(&mut s, 0, &path);
    s.set_turn(0, 2, true).unwrap();
    s.set_hand(0, Hand::new(1, 1, 0, 0, 0)).unwrap();
    s.finish_edit().unwrap();
    assert_eq!(s.player(0).roads_left, 0);
    let road = Action::BuildRoad {
        edge: edge_off(&s, &path),
    };
    assert_eq!(s.check(road), Err(RuleError::NoPiecesLeft));
    assert!(
        !legal(&s)
            .iter()
            .any(|a| matches!(a, Action::BuildRoad { .. }))
    );

    // 5 settlements: a 6th is refused until a city puts one back in the supply.
    let mut s = position(2);
    let path = find_path(&s, 2, &HashSet::new());
    put_path(&mut s, 0, &path);
    let others = spread_vertices(&s, 4, &around(&s, &path));
    for &v in &others {
        s.put_settlement(0, v).unwrap();
    }
    s.set_turn(0, 2, true).unwrap();
    s.set_hand(0, Hand::new(1, 1, 1, 3, 3)).unwrap();
    s.finish_edit().unwrap();
    let spot = Action::BuildSettlement { vertex: path[2] };
    assert_eq!(s.check(spot), Err(RuleError::NoPiecesLeft));
    s.apply(Action::BuildCity { vertex: others[0] }).unwrap();
    assert_eq!(s.player(0).settlements_left, 1);
    s.apply(spot).unwrap();

    // 4 cities: a 5th is refused.
    let mut s = position(2);
    let spots = spread_vertices(&s, 5, &HashSet::new());
    for &v in &spots[..4] {
        s.put_city(0, v).unwrap();
    }
    s.put_settlement(0, spots[4]).unwrap();
    s.set_turn(0, 2, true).unwrap();
    s.set_hand(0, Hand::new(0, 0, 0, 2, 3)).unwrap();
    s.finish_edit().unwrap();
    assert_eq!(
        s.check(Action::BuildCity { vertex: spots[4] }),
        Err(RuleError::NoPiecesLeft)
    );
    assert!(
        !legal(&s)
            .iter()
            .any(|a| matches!(a, Action::BuildCity { .. }))
    );
}

#[test]
fn road_building_with_one_piece_left_places_one_and_with_none_cannot_be_played() {
    let mut s = position(2);
    let path = find_path(&s, 14, &HashSet::new());
    put_path(&mut s, 0, &path);
    s.set_turn(0, 2, true).unwrap();
    s.give_dev_card(0, DevCard::RoadBuilding, false).unwrap();
    s.finish_edit().unwrap();
    s.apply(Action::PlayRoadBuilding).unwrap();
    assert!(matches!(s.phase, Phase::RoadBuilding { .. }));
    let a = legal(&s)[0];
    s.apply(a).unwrap();
    assert_eq!(s.phase, Phase::Main, "one piece left: one free road");
    assert_eq!(s.player(0).roads_on_board(), 15);

    let mut s = position(2);
    let path = find_path(&s, 15, &HashSet::new());
    put_path(&mut s, 0, &path);
    s.set_turn(0, 2, true).unwrap();
    s.give_dev_card(0, DevCard::RoadBuilding, false).unwrap();
    s.finish_edit().unwrap();
    assert!(s.check(Action::PlayRoadBuilding).is_err());
    assert!(!legal(&s).contains(&Action::PlayRoadBuilding));
}

#[test]
fn trading_waits_for_the_roll() {
    let mut s = position(3);
    s.set_hand(0, Hand::new(4, 0, 0, 0, 0)).unwrap();
    s.set_hand(1, Hand::new(0, 1, 0, 0, 0)).unwrap();
    s.finish_edit().unwrap();
    assert_eq!(s.phase, Phase::PreRoll);
    let offer = Action::OfferTrade {
        give: Hand::new(1, 0, 0, 0, 0),
        get: Hand::new(0, 1, 0, 0, 0),
    };
    let bank = Action::MaritimeTrade {
        give: Resource::Wood,
        get: Resource::Ore,
    };
    assert_eq!(s.check(offer), Err(RuleError::WrongPhase));
    assert_eq!(s.check(bank), Err(RuleError::WrongPhase));
    // With the round of 2026-11-12, the others may first propose trades.
    let mut round = s.clone();
    round.rules.trade_proposals = true;
    round
        .apply_forced(Action::RollDice, Some(Forced::Dice(2, 3)))
        .unwrap();
    assert_eq!(
        (round.phase, round.acting_player()),
        (Phase::TradeResponse, 1)
    );
    assert_eq!(round.check(offer), Err(RuleError::WrongPhase));
    round.apply(Action::RejectTrade).unwrap();
    assert_eq!(round.phase, Phase::Main);
    // Under the official rules trading opens straight after the roll.
    s.apply_forced(Action::RollDice, Some(Forced::Dice(2, 3)))
        .unwrap();
    assert_eq!((s.phase, s.acting_player()), (Phase::Main, 0));
    assert!(s.check(offer).is_ok());
    assert!(s.check(bank).is_ok());
}

#[test]
fn bank_trades_need_the_card_in_the_bank() {
    let mut s = position(2);
    s.set_turn(0, 2, true).unwrap();
    s.set_hand(0, Hand::new(4, 0, 0, 0, 0)).unwrap();
    s.set_hand(1, Hand::new(0, 19, 0, 0, 0)).unwrap();
    s.finish_edit().unwrap();
    let trade = |get| Action::MaritimeTrade {
        give: Resource::Wood,
        get,
    };
    assert_eq!(s.check(trade(Resource::Brick)), Err(RuleError::BankEmpty));
    assert!(s.check(trade(Resource::Ore)).is_ok());
}

#[test]
fn the_robber_must_move_to_another_hex() {
    // On a 7.
    let mut s = position(2);
    s.finish_edit().unwrap();
    s.apply_forced(Action::RollDice, Some(Forced::Dice(3, 4)))
        .unwrap();
    let stay = Action::MoveRobber {
        hex: s.robber,
        victim: None,
    };
    assert_eq!(s.check(stay), Err(RuleError::InvalidRobberHex));
    assert!(!legal(&s).contains(&stay));
    let nh = s.board.topology.num_hexes();
    assert_eq!(
        s.robber_hexes(),
        ((1u128 << nh) - 1) & !(1 << s.robber),
        "every other hex on the board, and nothing off it"
    );

    // With a knight.
    let mut s = position(2);
    s.give_dev_card(0, DevCard::Knight, false).unwrap();
    s.finish_edit().unwrap();
    s.apply(Action::PlayKnight).unwrap();
    let stay = Action::MoveRobber {
        hex: s.robber,
        victim: None,
    };
    assert_eq!(s.check(stay), Err(RuleError::InvalidRobberHex));
}

#[test]
fn settlements_after_setup_need_your_own_road_and_room() {
    let mut s = position(2);
    let path = find_path(&s, 2, &HashSet::new());
    s.put_settlement(0, path[0]).unwrap();
    s.put_road(0, edge(&s, path[0], path[1])).unwrap();
    s.set_turn(0, 2, true).unwrap();
    s.set_hand(0, Hand::new(2, 2, 1, 1, 0)).unwrap();
    s.finish_edit().unwrap();
    let at = |v| Action::BuildSettlement { vertex: v };
    // Next to your own settlement: the distance rule.
    assert_eq!(s.check(at(path[1])), Err(RuleError::IllegalLocation));
    // Two away but with no road of yours there yet.
    assert_eq!(s.check(at(path[2])), Err(RuleError::IllegalLocation));
    // With your road there, it's legal.
    let mut s = position(2);
    put_path(&mut s, 0, &path);
    s.set_turn(0, 2, true).unwrap();
    s.set_hand(0, Hand::new(1, 1, 1, 1, 0)).unwrap();
    s.finish_edit().unwrap();
    assert!(s.check(at(path[2])).is_ok());
}

#[test]
fn any_valid_offer_is_accepted_though_only_small_ones_are_listed() {
    let mut s = position(3);
    s.set_turn(0, 2, true).unwrap();
    s.set_hand(0, Hand::new(3, 0, 0, 0, 0)).unwrap();
    s.finish_edit().unwrap();
    let big = Action::OfferTrade {
        give: Hand::new(3, 0, 0, 0, 0),
        get: Hand::new(0, 1, 1, 0, 0),
    };
    assert!(s.check(big).is_ok(), "3-for-2 is a valid offer");
    assert!(
        !legal(&s).contains(&big),
        "the listing keeps to 1-for-1 and 2-for-1"
    );
    // Offering cards you don't have isn't valid.
    let unpaid = Action::OfferTrade {
        give: Hand::new(4, 0, 0, 0, 0),
        get: Hand::new(0, 1, 0, 0, 0),
    };
    assert!(s.check(unpaid).is_err());
}

/// Gives player 1 the road `a`-`b`, reached from a player 1 settlement on or next to `a`.
fn block(s: &mut GameState, a: VertexId, b: VertexId) {
    let e = edge(s, a, b);
    let reached = s.building(a).is_some_and(|x| x.owner == 1)
        || s.board.topology.vertex_edges[a as usize]
            .as_slice()
            .iter()
            .any(|&f| s.road_owner(f) == Some(1));
    if !reached && s.put_settlement(1, a).is_err() {
        let x = neighbors(s, a)
            .into_iter()
            .find(|&x| x != b && s.put_settlement(1, x).is_ok())
            .expect("room for a settlement");
        s.put_road(1, edge(s, x, a)).unwrap();
    }
    s.put_road(1, e).unwrap();
}

/// A vertex with three neighbours where player 0 has a settlement.
fn inland_settlement(s: &mut GameState) -> VertexId {
    let v = (0..s.board.topology.num_vertices() as VertexId)
        .find(|&v| neighbors(s, v).len() == 3)
        .unwrap();
    s.put_settlement(0, v).unwrap();
    v
}

#[test]
fn road_building_needs_somewhere_to_build() {
    // Pieces left, but every edge out of player 0's only settlement is taken.
    let mut s = position(2);
    let v = inland_settlement(&mut s);
    for w in neighbors(&s, v) {
        block(&mut s, w, v);
    }
    s.set_turn(0, 2, true).unwrap();
    s.give_dev_card(0, DevCard::RoadBuilding, false).unwrap();
    s.finish_edit().unwrap();
    assert_eq!(s.player(0).roads_left, 15);
    assert_eq!(
        s.check(Action::PlayRoadBuilding),
        Err(RuleError::IllegalLocation)
    );
    assert!(!legal(&s).contains(&Action::PlayRoadBuilding));

    // One free edge, leading to a corner whose other edges are taken: one free road, then the
    // card ends.
    let mut s = position(2);
    let v = inland_settlement(&mut s);
    let ws = neighbors(&s, v);
    for y in neighbors(&s, ws[0]).into_iter().filter(|&y| y != v) {
        block(&mut s, y, ws[0]);
    }
    for &w in &ws[1..] {
        block(&mut s, w, v);
    }
    s.set_turn(0, 2, true).unwrap();
    s.give_dev_card(0, DevCard::RoadBuilding, false).unwrap();
    s.finish_edit().unwrap();
    s.apply(Action::PlayRoadBuilding).unwrap();
    assert_eq!(
        legal(&s),
        vec![Action::BuildRoad {
            edge: edge(&s, v, ws[0])
        }]
    );
    s.apply(legal(&s)[0]).unwrap();
    assert_eq!(s.phase, Phase::Main, "nowhere left to build");
    assert_eq!(s.player(0).roads_left, 14);
}

#[test]
fn year_of_plenty_takes_only_what_the_bank_holds() {
    use Resource::*;
    let mut s = position(2);
    s.set_turn(0, 2, true).unwrap();
    // The bank keeps 1 wood, 2 brick and no sheep.
    s.set_hand(1, Hand::new(18, 17, 19, 0, 0)).unwrap();
    s.give_dev_card(0, DevCard::YearOfPlenty, false).unwrap();
    s.finish_edit().unwrap();
    for (a, b, ok) in [
        (Wood, Wood, false),
        (Brick, Brick, true),
        (Wood, Brick, true),
        (Brick, Wood, true),
        (Sheep, Wood, false),
        (Wood, Sheep, false),
        (Wheat, Ore, true),
    ] {
        let want = if ok {
            Ok(())
        } else {
            Err(RuleError::BankEmpty)
        };
        assert_eq!(
            s.check(Action::PlayYearOfPlenty { a, b }),
            want,
            "{a:?} and {b:?}"
        );
    }
    fuzz::cross_check_legality(&s, &legal(&s)).unwrap();
}

#[test]
fn offers_are_listed_by_offer_space_until_the_cap() {
    use engine_core::legal::OfferSpace;
    let mut s = position(2);
    s.set_turn(0, 2, true).unwrap();
    s.set_hand(0, Hand::new(2, 0, 0, 0, 0)).unwrap();
    s.set_hand(1, Hand::new(0, 0, 0, 0, 5)).unwrap();
    s.finish_edit().unwrap();
    // Cards given in each listed offer.
    let offers = |s: &GameState, space| {
        let mut out = Vec::new();
        s.legal_actions_with(space, &mut out);
        out.into_iter()
            .filter_map(|a| match a {
                Action::OfferTrade { give, .. } => Some(give.total()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert!(offers(&s, OfferSpace::None).is_empty());
    assert_eq!(offers(&s, OfferSpace::OneForOne), vec![1; 4]);
    let simple = offers(&s, OfferSpace::Simple);
    assert_eq!(simple.len(), 8);
    assert_eq!(simple.iter().filter(|&&n| n == 2).count(), 4);
    // "2-for-1" means two cards of one resource: with a wood and a sheep, only 1-for-1 offers.
    let mut mixed = s.clone();
    mixed.set_hand(0, Hand::new(1, 0, 1, 0, 0)).unwrap();
    mixed.finish_edit().unwrap();
    assert_eq!(offers(&mixed, OfferSpace::Simple), vec![1; 8]);
    for _ in 0..s.rules.max_offers_per_turn {
        s.apply(Action::OfferTrade {
            give: Hand::new(1, 0, 0, 0, 0),
            get: Hand::new(0, 0, 0, 0, 1),
        })
        .unwrap();
        s.apply(Action::RejectTrade).unwrap();
    }
    assert_eq!(s.phase, Phase::Main);
    assert!(offers(&s, OfferSpace::Simple).is_empty());
}

#[test]
fn awards_count_two_points_each() {
    let mut s = position(2);
    let path = find_path(&s, 5, &HashSet::new());
    put_path(&mut s, 0, &path);
    for v in spread_vertices(&s, 2, &around(&s, &path)) {
        s.put_settlement(0, v).unwrap();
    }
    for _ in 0..3 {
        s.add_played_card(0, DevCard::Knight).unwrap();
    }
    s.give_dev_card(0, DevCard::VictoryPoint, false).unwrap();
    s.finish_edit().unwrap();
    assert_eq!((s.longest_road, s.largest_army), (Some(0), Some(0)));
    assert_eq!(s.visible_vp(0), 3 + 2 + 2);
    assert_eq!(s.total_vp(0), 3 + 2 + 2 + 1);
}

#[test]
fn held_development_cards_count_new_and_old_but_not_played() {
    let mut s = position(2);
    s.give_dev_card(0, DevCard::Knight, false).unwrap();
    s.give_dev_card(0, DevCard::Monopoly, true).unwrap();
    s.give_dev_card(0, DevCard::VictoryPoint, false).unwrap();
    s.add_played_card(0, DevCard::Knight).unwrap();
    s.finish_edit().unwrap();
    assert_eq!(s.player(0).dev_cards_held(), 3);
    assert_eq!(s.player(1).dev_cards_held(), 0);
}

#[test]
fn presets_are_found_by_name() {
    assert_eq!(
        GameConfig::preset("official", 3),
        Some(GameConfig::official(3))
    );
    assert_eq!(
        GameConfig::preset("colonist-1v1", 2),
        Some(GameConfig::colonist_1v1())
    );
    assert_eq!(
        GameConfig::preset("catanatron-compat", 4),
        Some(GameConfig::catanatron_compat(4))
    );
    assert_eq!(GameConfig::preset("nope", 4), None);
}

#[test]
fn action_and_outcome_codes_are_stable() {
    // The differential test and the bot bridge parse these (tools/difftest).
    use Resource::*;
    let codes = [
        (Action::BuildSettlement { vertex: 7 }, "S7"),
        (Action::BuildRoad { edge: 12 }, "R12"),
        (Action::BuildCity { vertex: 3 }, "C3"),
        (Action::BuyDevCard, "BUY"),
        (Action::RollDice, "ROLL"),
        (Action::PlayKnight, "KN"),
        (Action::PlayRoadBuilding, "RB"),
        (Action::PlayYearOfPlenty { a: Wood, b: Ore }, "Y04"),
        (Action::PlayMonopoly { resource: Sheep }, "MO2"),
        (
            Action::MoveRobber {
                hex: 5,
                victim: Some(1),
            },
            "MV5:1",
        ),
        (
            Action::MoveRobber {
                hex: 5,
                victim: None,
            },
            "MV5:-",
        ),
        (Action::Discard { resource: Brick }, "DI1"),
        (
            Action::MaritimeTrade {
                give: Wheat,
                get: Wood,
            },
            "MT30",
        ),
        (Action::EndTurn, "END"),
        (
            Action::OfferTrade {
                give: Hand::new(2, 0, 0, 0, 0),
                get: Hand::new(0, 0, 0, 0, 1),
            },
            "OF2,0,0,0,0/0,0,0,0,1",
        ),
        (Action::AcceptTrade, "ACC"),
        (Action::RejectTrade, "REJ"),
        (Action::ConfirmTrade { with: 3 }, "CF3"),
        (Action::CancelTrade, "CAN"),
    ];
    for (a, code) in codes {
        assert_eq!(a.code(), code);
    }
    use serde_json::json;
    assert_eq!(Outcome::Dice { a: 3, b: 4 }.code(), json!([3, 4]));
    assert_eq!(
        Outcome::DevCard {
            card: Some(DevCard::Monopoly)
        }
        .code(),
        json!(DevCard::Monopoly.index())
    );
    assert_eq!(
        Outcome::Stole {
            resource: Some(Ore)
        }
        .code(),
        json!(4)
    );
    assert_eq!(
        Outcome::DevCard { card: None }.code(),
        serde_json::Value::Null
    );
    assert_eq!(Outcome::None.code(), serde_json::Value::Null);
}

#[test]
fn hidden_outcomes_are_seen_only_by_those_involved() {
    let card = Outcome::DevCard {
        card: Some(DevCard::Knight),
    };
    assert_eq!(card.visible_to(1, 1, &Action::BuyDevCard), card);
    assert_eq!(
        card.visible_to(0, 1, &Action::BuyDevCard),
        Outcome::DevCard { card: None }
    );
    let rob = Action::MoveRobber {
        hex: 4,
        victim: Some(2),
    };
    let stole = Outcome::Stole {
        resource: Some(Resource::Wheat),
    };
    let unseen = Outcome::Stole { resource: None };
    assert_eq!(stole.visible_to(1, 1, &rob), stole, "the thief");
    assert_eq!(stole.visible_to(2, 1, &rob), stole, "the victim");
    assert_eq!(stole.visible_to(0, 1, &rob), unseen);
    assert_eq!(stole.visible_to(3, 1, &rob), unseen);
    let dice = Outcome::Dice { a: 2, b: 5 };
    assert_eq!(dice.visible_to(0, 1, &Action::RollDice), dice);
}

#[test]
fn road_length_matches_a_brute_force_search() {
    // Random networks grown from one or two settlements, so loops and branches occur, then
    // sometimes cut by opponents' settlements.
    let mut rng = Rng::seed(8);
    let (mut loops, mut cuts) = (0, 0);
    for _ in 0..300 {
        let mut s = position(2);
        let nv = s.board.topology.num_vertices() as u32;
        let starts = 1 + rng.below(2);
        while s.player(0).settlements_on_board() < starts as u8 {
            let _ = s.put_settlement(0, rng.below(nv) as VertexId);
        }
        let target = 5 + rng.below(11) as u8;
        while s.player(0).roads_on_board() < target {
            let spots: Vec<EdgeId> = s.road_spots(0).ones().collect();
            if spots.is_empty() {
                break;
            }
            s.put_road(0, spots[rng.below(spots.len() as u32) as usize])
                .unwrap();
        }
        let on_roads: Vec<VertexId> = (0..nv as VertexId)
            .filter(|&v| {
                s.building(v).is_none()
                    && s.board.topology.vertex_edges[v as usize]
                        .as_slice()
                        .iter()
                        .any(|&e| s.road_owner(e) == Some(0))
            })
            .collect();
        for _ in 0..rng.below(3) {
            let v = on_roads[rng.below(on_roads.len() as u32) as usize];
            if s.put_settlement(1, v).is_ok() {
                cuts += 1;
            }
        }
        s.finish_edit().unwrap();
        let ends: HashSet<VertexId> = (0..s.board.topology.num_edges() as EdgeId)
            .filter(|&e| s.road_owner(e) == Some(0))
            .flat_map(|e| s.board.topology.edge_vertices[e as usize])
            .collect();
        if s.player(0).roads_on_board() as usize >= ends.len() {
            loops += 1;
        }
        let want = s.compute_road_length_reference(0);
        assert_eq!(s.compute_road_length(0), want);
        assert_eq!(s.player(0).road_length, want);
    }
    assert!(loops > 30 && cuts > 30, "{loops} with loops, {cuts} cuts");
}

#[test]
fn errors_explain_themselves() {
    // The server passes these messages on to players.
    assert_eq!(RuleError::GameOver.to_string(), "the game is over");
    assert!(BoardError::NoRobberStart.to_string().contains("robber"));
    assert!(!ReplayError::FinalPosition.to_string().is_empty());
    assert!(EditError::NoPiecesLeft.to_string().contains("pieces"));
}

#[test]
fn numbers_out_of_range_and_empty_offers_are_refused() {
    let mut s = position(3);
    let (h, v1, _) = hex_with_two_spots(&s, Resource::Ore);
    s.put_settlement(1, v1).unwrap();
    s.set_hand(1, Hand::new(1, 0, 0, 0, 0)).unwrap();
    s.finish_edit().unwrap();
    s.apply_forced(Action::RollDice, Some(Forced::Dice(3, 4)))
        .unwrap();
    let nh = s.board.topology.num_hexes() as HexId;
    assert_eq!(
        s.check(Action::MoveRobber {
            hex: nh,
            victim: None
        }),
        Err(RuleError::InvalidId)
    );
    // Player numbers past the table are refused before they are used as bit positions.
    for victim in [3, 200] {
        assert_eq!(
            s.check(Action::MoveRobber {
                hex: h,
                victim: Some(victim)
            }),
            Err(RuleError::InvalidVictim),
            "{victim}"
        );
    }

    let mut s = position(3);
    s.set_turn(0, 2, true).unwrap();
    s.set_hand(0, Hand::new(1, 0, 0, 0, 0)).unwrap();
    s.set_hand(1, Hand::new(0, 1, 0, 0, 0)).unwrap();
    s.finish_edit().unwrap();
    let nothing_for_brick = Action::OfferTrade {
        give: Hand::EMPTY,
        get: Hand::new(0, 1, 0, 0, 0),
    };
    assert_eq!(s.check(nothing_for_brick), Err(RuleError::InvalidTrade));
    s.apply(Action::OfferTrade {
        give: Hand::new(1, 0, 0, 0, 0),
        get: Hand::new(0, 1, 0, 0, 0),
    })
    .unwrap();
    s.apply(Action::AcceptTrade).unwrap();
    assert_eq!(s.phase, Phase::TradeConfirm, "player 2 can't pay");
    for with in [2, 3, 200] {
        assert_eq!(
            s.check(Action::ConfirmTrade { with }),
            Err(RuleError::InvalidTrade),
            "{with}"
        );
    }
}

#[test]
fn a_free_road_can_win_the_game() {
    // Player 0 has 8 points and a road of four; the first free road makes it five.
    let mut s = position(2);
    let path = find_path(&s, 5, &HashSet::new());
    put_path(&mut s, 0, &path[..5]);
    for v in spread_vertices(&s, 3, &around(&s, &path)) {
        s.put_city(0, v).unwrap();
    }
    s.give_dev_card(0, DevCard::VictoryPoint, false).unwrap();
    s.give_dev_card(0, DevCard::RoadBuilding, false).unwrap();
    s.set_turn(0, 2, true).unwrap();
    s.finish_edit().unwrap();
    assert_eq!((s.total_vp(0), s.longest_road), (8, None));
    s.apply(Action::PlayRoadBuilding).unwrap();
    s.apply(Action::BuildRoad {
        edge: edge(&s, path[4], path[5]),
    })
    .unwrap();
    assert_eq!(s.longest_road, Some(0));
    assert_eq!((s.winner, s.phase), (Some(0), Phase::GameOver));
    assert_eq!(s.player(0).roads_on_board(), 5, "no second free road");
    s.check_invariants().unwrap();
}

#[test]
fn a_friendly_robber_with_every_hex_protected_may_go_anywhere() {
    // No preset can protect every hex on the standard board, so this uses the seven inner
    // hexes and three opponents, each settled on a different pair of the outer six.
    let mut layout = board(1).to_layout();
    layout
        .hexes
        .retain(|h| Axial::new(h.q, h.r).distance(Axial::new(0, 0)) <= 1);
    layout.ports.clear();
    layout.robber = Some(Axial::new(1, 0));
    let mut rules = GameConfig::official(4).rules;
    rules.friendly_robber_max_vp = Some(2);
    let b = Arc::new(Board::from_layout(&layout).unwrap());
    let mut s = GameState::empty_position(rules, b, 5);
    let topo = s.board.topology.clone();
    let centre = topo.hex_at(Axial::new(0, 0)).unwrap();
    let corners = topo.hex_vertices[centre as usize];
    for (q, k) in [(1, 0), (2, 2), (3, 4)] {
        s.put_settlement(q, corners[k]).unwrap();
    }
    s.finish_edit().unwrap();
    s.apply_forced(Action::RollDice, Some(Forced::Dice(3, 4)))
        .unwrap();
    let protected = |h: HexId| {
        topo.hex_vertices[h as usize]
            .iter()
            .any(|&v| s.building(v).is_some())
    };
    assert!((0..topo.num_hexes() as HexId).all(protected));
    let open: HashSet<HexId> = legal(&s)
        .iter()
        .filter_map(|a| match a {
            Action::MoveRobber { hex, .. } => Some(*hex),
            _ => None,
        })
        .collect();
    let others: HashSet<HexId> = (0..topo.num_hexes() as HexId)
        .filter(|&h| h != s.robber)
        .collect();
    assert_eq!(open, others);
}

#[test]
fn a_finished_game_shows_the_winner_as_acting() {
    let mut s = GameState::new(&GameConfig::official(4), 6);
    let w = play_out(
        &mut s,
        RandomPolicy::Builder,
        &Limits::default(),
        &mut Rng::seed(6),
        &mut Vec::new(),
    )
    .unwrap();
    for q in 0..4 {
        assert_eq!(s.observe(q).acting, w);
    }
}

#[test]
fn the_editor_refuses_impossible_positions() {
    let mut s = position(2);
    let topo = s.board.topology.clone();
    let nv = topo.num_vertices() as VertexId;
    let ne = topo.num_edges() as EdgeId;
    let nh = topo.num_hexes() as HexId;
    assert_eq!(
        s.put_settlement(2, 0),
        Err(EditError::InvalidId),
        "no player 2"
    );
    assert_eq!(s.put_settlement(0, nv), Err(EditError::InvalidId));
    assert_eq!(s.put_city(0, nv), Err(EditError::InvalidId));
    assert_eq!(s.put_road(0, ne), Err(EditError::InvalidId));
    assert_eq!(s.set_robber(nh), Err(EditError::InvalidId));
    assert!(matches!(
        s.set_turn(0, 0, false),
        Err(EditError::Invalid(_))
    ));
    assert_eq!(
        s.set_hand(0, Hand::new(0, 20, 0, 0, 0)),
        Err(EditError::BankShort(Resource::Brick))
    );
    assert!(matches!(
        s.add_played_card(0, DevCard::VictoryPoint),
        Err(EditError::Invalid(_))
    ));

    let path = find_path(&s, 1, &HashSet::new());
    put_path(&mut s, 0, &path);
    let road = edge(&s, path[0], path[1]);
    assert_eq!(s.put_road(1, road), Err(EditError::Occupied));
    assert_eq!(s.put_city(1, path[0]), Err(EditError::NotOwnSettlement));
    s.put_city(0, path[0]).unwrap();
    assert_eq!(
        s.put_city(0, path[0]),
        Err(EditError::NotOwnSettlement),
        "already a city"
    );

    // Out of pieces: five settlements, four cities, fifteen roads.
    let mut s = position(2);
    let spots = spread_vertices(&s, 10, &HashSet::new());
    for &v in &spots[..5] {
        s.put_settlement(1, v).unwrap();
    }
    assert_eq!(s.put_settlement(1, spots[5]), Err(EditError::NoPiecesLeft));
    for &v in &spots[5..9] {
        s.put_city(0, v).unwrap();
    }
    assert_eq!(s.put_city(0, spots[9]), Err(EditError::NoPiecesLeft));
    let free: Vec<EdgeId> = (0..ne).filter(|&e| s.road_owner(e).is_none()).collect();
    for &e in &free[..15] {
        s.put_road(1, e).unwrap();
    }
    assert_eq!(s.put_road(1, free[15]), Err(EditError::NoPiecesLeft));
}

#[test]
fn nobody_wins_during_initial_placement() {
    // Even with a one-point target, points from placement count only once play starts.
    let mut config = GameConfig::official(2);
    config.rules.vp_to_win = 1;
    let mut s = GameState::new(&config, 3);
    let a = legal(&s)[0];
    s.apply(a).unwrap();
    assert_eq!(s.total_vp(0), 1);
    s.finish_edit().unwrap();
    assert_eq!((s.winner, s.turn), (None, 0));
}

#[test]
fn an_edited_position_equals_the_same_position_reached_by_play() {
    let config = GameConfig::official(4);
    let mut played = GameState::new(&config, 12);
    let (mut rng, mut guard, limits) = (Rng::seed(12), TurnGuard::default(), Limits::default());
    while played.turn == 0 {
        let legal = legal(&played);
        let a = guard.choose(RandomPolicy::Builder, &limits, &played, &legal, &mut rng);
        played.apply(a).unwrap();
    }
    let mut edited = GameState::empty_position(config.rules, played.board.clone(), 34);
    let topo = played.board.topology.clone();
    for v in 0..topo.num_vertices() as VertexId {
        if let Some(b) = played.building(v) {
            edited.put_settlement(b.owner, v).unwrap();
        }
    }
    for e in 0..topo.num_edges() as EdgeId {
        if let Some(q) = played.road_owner(e) {
            edited.put_road(q, e).unwrap();
        }
    }
    for q in 0..4 {
        edited.set_hand(q, played.player(q).hand).unwrap();
    }
    edited.set_turn(0, 1, false).unwrap();
    edited.finish_edit().unwrap();
    assert_eq!(edited.position_hash(), played.position_hash());
}

#[test]
fn any_position_can_be_turned_into_a_robber_move_at_the_start_of_a_turn() {
    let mut s = GameState::new(&GameConfig::official(4), 8);
    let between = |r: Result<GameState, EditError>| matches!(r, Err(EditError::Invalid(m)) if m.contains("between the initial placement and the end"));
    assert!(between(s.robber_to_move(0)), "not in the initial placement");
    let limits = Limits {
        max_turns: 30,
        ..Limits::default()
    };
    play_out(
        &mut s,
        RandomPolicy::Builder,
        &limits,
        &mut Rng::seed(8),
        &mut Vec::new(),
    );
    assert!(!s.is_over() && s.turn > 30);
    // Someone mid-turn with a card bought this turn.
    s.phase = Phase::Main;
    s.players[s.current as usize].dev_new[DevCard::Knight.index()] += 1;
    let k = s
        .dev_deck
        .iter()
        .position(|&c| c == DevCard::Knight)
        .unwrap();
    s.dev_deck.remove(k);
    s.check_invariants().unwrap();
    let mover = s.current;
    for p in 0..4 {
        let r = s.robber_to_move(p).unwrap();
        assert_eq!((r.current, r.turn), (p, s.turn));
        assert_eq!(
            r.phase,
            Phase::MoveRobber {
                resume: Resume::PreRoll
            }
        );
        // The mover's card is theirs to play from now on, unless the turn is still theirs.
        let held = &r.players[mover as usize];
        let knights = (
            held.dev[DevCard::Knight.index()],
            held.dev_new[DevCard::Knight.index()],
        );
        let before = s.players[mover as usize].dev[DevCard::Knight.index()];
        assert_eq!(
            knights,
            if p == mover {
                (before, 1)
            } else {
                (before + 1, 0)
            }
        );
        let moves = legal(&r);
        assert!(!moves.is_empty());
        for a in &moves {
            assert!(
                matches!(a, Action::MoveRobber { hex, .. } if *hex != s.robber),
                "{a:?}"
            );
        }
        let mut after = r.clone();
        after.apply(moves[0]).unwrap();
        assert_eq!((after.current, after.phase), (p, Phase::PreRoll));
    }
    assert!(s.robber_to_move(4).is_err());
    // Mid-discard, the discards still owed are let go.
    let mut owed = s.clone();
    owed.phase = Phase::Discard;
    owed.discard_pending[0] = 2;
    let r = owed.robber_to_move(1).unwrap();
    assert!(r.discard_pending.iter().all(|&d| d == 0));
    assert_eq!(r.players[0].hand, s.players[0].hand);
    play_out(
        &mut s,
        RandomPolicy::Builder,
        &Limits::default(),
        &mut Rng::seed(9),
        &mut Vec::new(),
    );
    assert!(
        s.is_over() && between(s.robber_to_move(0)),
        "not after the end"
    );
}
