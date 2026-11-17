//! Positions typed in from games played elsewhere, as one seat sees them: the endgame
//! calculator's editor (docs/STATISTICS.md, "Endgame calculator"). Built here, in the engine, so
//! the server and the browser (engine/wasm) check a position the same way.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::belief::Hands;
use crate::board::Board;
use crate::edit::EditError;
use crate::rng::Rng;
use crate::rules::GameConfig;
use crate::state::{GameState, Phase};
use crate::types::*;

/// Typed-in positions are of 4-player games.
pub const PLAYERS: usize = 4;

/// A building in a typed-in position.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct EnteredBuilding {
    pub vertex: VertexId,
    pub owner: PlayerId,
    pub city: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct EnteredRoad {
    pub edge: EdgeId,
    pub owner: PlayerId,
}

/// What everyone can see of a player in a typed-in position.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
pub struct EnteredPlayer {
    /// Resource cards and unplayed development cards held (the seat's own come from `hand` and
    /// `dev`).
    pub cards: u32,
    pub dev_cards: u32,
    /// Development cards played, in `DevCard::ALL` order (victory points are never played).
    pub played: [u8; 5],
}

/// A position from a game played elsewhere, as the seat sees it: the pieces, everyone's public
/// counts, the seat's own cards, and the bank, which is public (its piles are on the table).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Entered {
    pub seat: PlayerId,
    /// Whose turn, which turn (every player's counts), and whether they have rolled.
    pub current: PlayerId,
    pub turn: u32,
    pub rolled: bool,
    pub robber: HexId,
    pub buildings: Vec<EnteredBuilding>,
    pub roads: Vec<EnteredRoad>,
    /// The seat's resource cards, in `Resource::ALL` order, and its unplayed development cards,
    /// in `DevCard::ALL` order.
    pub hand: [u8; 5],
    pub dev: [u8; 5],
    pub players: Vec<EnteredPlayer>,
    pub bank: [u8; 5],
    /// Who holds Longest Road and Largest Army where the pieces don't settle it (a tie the
    /// holder keeps); otherwise worked out.
    #[serde(default)]
    pub longest_road: Option<PlayerId>,
    #[serde(default)]
    pub largest_army: Option<PlayerId>,
}

impl Entered {
    /// What `seat` can see of `state`, as it would be typed in. Whether the player to move has
    /// rolled is kept; a discard, robber move or trade in progress isn't.
    pub fn seen(state: &GameState, seat: PlayerId) -> Entered {
        let view = state.view(seat);
        let topo = &state.board.topology;
        let buildings = (0..topo.num_vertices() as VertexId)
            .filter_map(|v| {
                view.building(v).map(|b| EnteredBuilding {
                    vertex: v,
                    owner: b.owner,
                    city: b.city,
                })
            })
            .collect();
        let roads = (0..topo.num_edges() as EdgeId)
            .filter_map(|e| {
                view.road_owner(e)
                    .map(|owner| EnteredRoad { edge: e, owner })
            })
            .collect();
        let counts = |h: Hand| Resource::ALL.map(|r| h.get(r));
        let me = view.me_state();
        let n = view.num_players() as PlayerId;
        Entered {
            seat,
            current: view.current(),
            turn: view.turn(),
            rolled: !matches!(view.phase(), Phase::PreRoll),
            robber: view.robber(),
            buildings,
            roads,
            hand: counts(me.hand),
            dev: std::array::from_fn(|i| me.dev[i] + me.dev_new[i]),
            players: (0..n)
                .map(|q| {
                    let p = view.player(q);
                    EnteredPlayer {
                        cards: p.hand_size,
                        dev_cards: p.dev_cards,
                        played: p.dev_played,
                    }
                })
                .collect(),
            bank: counts(view.bank()),
            longest_road: view.longest_road(),
            largest_army: view.largest_army(),
        }
    }
}

/// A hand from counts in `Resource::ALL` order.
pub fn hand_of(counts: &[u8; 5]) -> Hand {
    let mut h = Hand::EMPTY;
    for (r, &n) in Resource::ALL.into_iter().zip(counts) {
        h.add_n(r, n);
    }
    h
}

/// The others' hands: `unseen` dealt at random, `sizes[q]` cards each, the seat's left as `mine`.
pub fn deal(unseen: Hand, sizes: &[u32], seat: usize, mine: Hand, rng: &mut Rng) -> Hands {
    let mut cards: Vec<Resource> = Resource::ALL
        .iter()
        .flat_map(|&r| std::iter::repeat_n(r, unseen.get(r) as usize))
        .collect();
    rng.shuffle(&mut cards);
    let mut it = cards.into_iter();
    let mut hands: Hands = Default::default();
    for (q, &size) in sizes.iter().enumerate() {
        if q == seat {
            hands[q] = mine;
            continue;
        }
        for r in it.by_ref().take(size as usize) {
            hands[q].add_n(r, 1);
        }
    }
    hands
}

/// A typed-in position, built: the game state, with the others' cards and development cards one
/// random dealing consistent with what's known, and what a caller needs to deal again: the cards
/// the seat can't see, every hand's size, and the random stream, left where the dealing stopped.
pub struct Built {
    pub state: GameState,
    pub unseen: Hand,
    pub sizes: Vec<u32>,
    pub rng: Rng,
}

/// Builds a typed-in position, or says what makes it impossible.
pub fn build(e: &Entered, board: Arc<Board>, seed: u64) -> Result<Built, String> {
    let config = GameConfig::official(PLAYERS as u8);
    if e.players.len() != PLAYERS {
        return Err(format!("a position has {PLAYERS} players"));
    }
    if e.seat as usize >= PLAYERS || e.current as usize >= PLAYERS {
        return Err("no such seat".into());
    }
    for (award, holder) in [
        ("Longest Road", e.longest_road),
        ("Largest Army", e.largest_army),
    ] {
        if holder.is_some_and(|h| h as usize >= PLAYERS) {
            return Err(format!("{award} is held by a player who isn't in the game"));
        }
    }
    // No list is longer than the board has places for.
    let topo = &board.topology;
    if e.buildings.len() > topo.num_vertices() || e.roads.len() > topo.num_edges() {
        return Err("more buildings or roads than the board has places for".into());
    }
    let player = |q: PlayerId| {
        ["Red", "Blue", "Orange", "White"]
            .get(q as usize)
            .copied()
            .unwrap_or("?")
    };
    // After the initial placement nobody has fewer than two buildings and two roads.
    for q in 0..PLAYERS as PlayerId {
        let buildings = e.buildings.iter().filter(|b| b.owner == q).count();
        let roads = e.roads.iter().filter(|r| r.owner == q).count();
        if buildings < 2 || roads < 2 {
            return Err(format!(
                "{} has {buildings} buildings and {roads} roads; after the initial placement everyone has at least two of each",
                player(q)
            ));
        }
    }
    let mut s = GameState::empty_position(config.rules, board, seed);
    // Settlements first, so a road can stand on one placed later in the list. A city goes down
    // whole, so a player's five settlements don't get in its way.
    for b in e.buildings.iter().filter(|b| !b.city) {
        s.put_settlement(b.owner, b.vertex).map_err(|err| {
            format!(
                "{}'s settlement at spot {}: {err}",
                player(b.owner),
                b.vertex
            )
        })?;
    }
    for b in e.buildings.iter().filter(|b| b.city) {
        s.put_city(b.owner, b.vertex)
            .map_err(|err| format!("{}'s city at spot {}: {err}", player(b.owner), b.vertex))?;
    }
    for r in &e.roads {
        s.put_road(r.owner, r.edge)
            .map_err(|err| format!("{}'s road at edge {}: {err}", player(r.owner), r.edge))?;
    }

    let seat = e.seat as usize;
    let mine = hand_of(&e.hand);
    let bank = hand_of(&e.bank);
    let mut unseen = Hand::EMPTY;
    for r in Resource::ALL {
        let out = u32::from(bank.get(r)) + u32::from(mine.get(r));
        if out > u32::from(BANK_PER_RESOURCE) {
            return Err(format!(
                "the bank and your hand have {out} {r:?} cards; there are {BANK_PER_RESOURCE}"
            ));
        }
        unseen.add_n(r, BANK_PER_RESOURCE - bank.get(r) - mine.get(r));
    }
    let sizes: Vec<u32> = (0..PLAYERS)
        .map(|q| {
            if q == seat {
                mine.total()
            } else {
                e.players[q].cards
            }
        })
        .collect();
    // Added up wide: the counts are as typed.
    let held: u64 = (0..PLAYERS)
        .filter(|&q| q != seat)
        .map(|q| u64::from(sizes[q]))
        .sum();
    if held != u64::from(unseen.total()) {
        return Err(format!(
            "the others hold {held} cards, but the bank and your hand leave {}",
            unseen.total()
        ));
    }
    let mut rng = Rng::seed(seed);
    let hands = deal(unseen, &sizes, seat, mine, &mut rng);
    for (q, &hand) in hands.iter().enumerate().take(PLAYERS) {
        s.set_hand(q as PlayerId, hand)
            .map_err(|err| err.to_string())?;
    }

    for (card, &n) in DevCard::ALL.into_iter().zip(&e.dev) {
        for _ in 0..n {
            s.give_dev_card(e.seat, card, false)
                .map_err(|_| format!("there aren't that many {card:?} cards"))?;
        }
    }
    for (q, p) in e.players.iter().enumerate() {
        for (card, &n) in DevCard::ALL.into_iter().zip(&p.played) {
            for _ in 0..n {
                s.add_played_card(q as PlayerId, card)
                    .map_err(|err| match err {
                        EditError::CardNotInDeck(c) => {
                            format!("more {c:?} cards played and held than the game has")
                        }
                        other => other.to_string(),
                    })?;
            }
        }
    }
    // The others' hidden development cards, dealt at random. A dealing in which another player
    // already has the points to win through hidden victory point cards can't be the real game
    // (they would have shown them and won), so it is dealt again, as the search's own sampling
    // does (`View::determinize`); after `DEV_DEALS` tries the last dealing stands.
    let target = u32::from(s.rules.vp_to_win);
    let before = s;
    let mut attempt = 0;
    loop {
        attempt += 1;
        let mut s = before.clone();
        for (q, p) in e.players.iter().enumerate().filter(|&(q, _)| q != seat) {
            for _ in 0..p.dev_cards {
                if s.dev_deck.is_empty() {
                    return Err("more development cards held and played than the game has".into());
                }
                let card = s.dev_deck[rng.below(s.dev_deck.len() as u32) as usize];
                s.give_dev_card(q as PlayerId, card, false)
                    .map_err(|err| err.to_string())?;
            }
        }
        s.set_robber(e.robber).map_err(|err| err.to_string())?;
        s.set_turn(e.current, e.turn, e.rolled)
            .map_err(|err| err.to_string())?;
        s.set_awards(e.longest_road, e.largest_army)
            .map_err(|err| err.to_string())?;
        s.finish_edit().map_err(|err| err.to_string())?;
        let wins_on_hidden_points = (0..PLAYERS as PlayerId).filter(|&q| q != e.seat).any(|q| {
            let hidden = u32::from(s.players[q as usize].dev[DevCard::VictoryPoint.index()]);
            let total = u32::from(s.total_vp(q));
            hidden > 0 && total >= target && total - hidden.min(total) < target
        });
        if !wins_on_hidden_points || attempt >= DEV_DEALS {
            return Ok(Built {
                state: s,
                unseen,
                sizes,
                rng,
            });
        }
    }
}

/// How many times `build` deals the others' development cards before it accepts a dealing in
/// which one of them has already won on hidden victory points.
pub const DEV_DEALS: u32 = 200;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{Limits, RandomPolicy, play_out};

    /// A game of builders 40 turns in, between turns.
    fn midgame(seed: u64) -> GameState {
        let mut s = GameState::new(&GameConfig::official(4), seed);
        let limits = Limits {
            max_turns: 40,
            ..Limits::default()
        };
        play_out(
            &mut s,
            RandomPolicy::Builder,
            &limits,
            &mut Rng::seed(seed),
            &mut Vec::new(),
        );
        assert!(!s.is_over());
        s.phase = Phase::PreRoll;
        s
    }

    #[test]
    fn a_position_as_seen_builds_back_to_the_same_view() {
        for seed in [1, 2, 3] {
            let s = midgame(seed);
            let e = Entered::seen(&s, 1);
            let b = build(&e, s.board.clone(), 9).unwrap();
            let (x, y) = (s.view(1), b.state.view(1));
            let topo = &s.board.topology;
            for v in 0..topo.num_vertices() as VertexId {
                assert_eq!(x.building(v), y.building(v));
            }
            for r in 0..topo.num_edges() as EdgeId {
                assert_eq!(x.road_owner(r), y.road_owner(r));
            }
            assert_eq!(
                (x.robber(), x.bank(), x.turn(), x.current()),
                (y.robber(), y.bank(), y.turn(), y.current())
            );
            assert_eq!(
                (x.longest_road(), x.largest_army(), x.my_total_vp()),
                (y.longest_road(), y.largest_army(), y.my_total_vp())
            );
            assert_eq!(x.me_state().hand, y.me_state().hand);
            assert_eq!(x.unseen_dev_pool(), y.unseen_dev_pool());
            for q in 0..4 {
                let (p, r) = (x.player(q), y.player(q));
                assert_eq!(
                    (p.hand_size, p.dev_cards, p.dev_played, p.road_length),
                    (r.hand_size, r.dev_cards, r.dev_played, r.road_length)
                );
                assert_eq!(x.visible_vp(q), y.visible_vp(q));
                assert_eq!(b.sizes[q as usize], p.hand_size);
            }
            // The unseen cards are the others' together.
            let mut others = Hand::EMPTY;
            for q in [0, 2, 3] {
                others.add(&s.players[q].hand);
            }
            assert_eq!(b.unseen, others);
            // A fresh dealing keeps every count and the seat's own hand.
            let mut rng = Rng::seed(4);
            let hands = deal(b.unseen, &b.sizes, 1, b.state.players[1].hand, &mut rng);
            let mut dealt = Hand::EMPTY;
            for (q, hand) in hands.iter().enumerate().take(4) {
                assert_eq!(hand.total(), b.sizes[q]);
                if q != 1 {
                    dealt.add(hand);
                }
            }
            assert_eq!((dealt, hands[1]), (b.unseen, s.players[1].hand));
        }
    }

    /// What a view shows of its own seat: whether the dice are rolled, and every development card
    /// held, those bought this turn too.
    #[test]
    fn a_position_as_seen_keeps_the_roll_and_new_cards() {
        let mut s = midgame(4);
        assert!(!Entered::seen(&s, 1).rolled);
        let before = Entered::seen(&s, 1).dev;
        let card = s.dev_deck.pop().unwrap();
        s.players[1].dev_new[card as usize] += 1;
        s.phase = Phase::Main;
        let e = Entered::seen(&s, 1);
        assert!(e.rolled);
        let mut want = before;
        want[card as usize] += 1;
        assert_eq!(e.dev, want);
        let b = build(&e, s.board.clone(), 2).unwrap();
        assert_eq!(b.state.phase, Phase::Main);
    }

    #[test]
    fn impossible_positions_say_why() {
        let s = midgame(5);
        let good = Entered::seen(&s, 0);
        let refused = |e: &Entered, says: &str| match build(e, s.board.clone(), 1) {
            Err(err) => assert!(err.contains(says), "{err}"),
            Ok(_) => panic!("built a position that should say {says:?}"),
        };
        let mut e = good.clone();
        e.players[1].cards += 1;
        refused(&e, "the bank and your hand leave");
        let mut e = good.clone();
        e.bank[2] = BANK_PER_RESOURCE;
        e.hand[2] = 1;
        refused(&e, "there are 19");
        let mut e = good.clone();
        e.players[3].dev_cards += 30;
        refused(&e, "than the game has");
        let mut e = good.clone();
        e.dev[1] = 6;
        refused(&e, "VictoryPoint");
        let mut e = good.clone();
        e.players[2].played[0] = 15;
        refused(&e, "Knight cards played and held");
        let mut e = good.clone();
        let v = e.buildings[0].vertex;
        let next = s.board.topology.vertex_neighbors[v as usize].as_slice()[0];
        e.buildings.push(EnteredBuilding {
            vertex: next,
            owner: 1,
            city: false,
        });
        refused(&e, "too close");
        let mut e = good.clone();
        e.roads.retain(|r| r.owner != 2);
        refused(&e, "Orange has");
        let mut e = good.clone();
        let first = e.buildings.iter().position(|b| b.owner == 3).unwrap();
        let keep = e.buildings[first];
        e.buildings.retain(|b| b.owner != 3);
        e.buildings.push(keep);
        refused(&e, "White has 1 buildings");
        let mut e = good.clone();
        e.players.pop();
        refused(&e, "4 players");
        let mut e = good.clone();
        e.seat = 4;
        refused(&e, "no such seat");
        let mut e = good.clone();
        e.turn = 0;
        refused(&e, "turn 0");
        assert!(build(&good, s.board.clone(), 1).is_ok());
    }

    /// Twelve spots no two of which are neighbours, for pieces placed by hand.
    fn spread_spots(s: &GameState, taken: &[VertexId], n: usize) -> Vec<VertexId> {
        let topo = &s.board.topology;
        let mut spots: Vec<VertexId> = taken.to_vec();
        for v in 0..topo.num_vertices() as VertexId {
            let near = |u: &VertexId| {
                *u == v || topo.vertex_neighbors[*u as usize].as_slice().contains(&v)
            };
            if !spots.iter().any(near) {
                spots.push(v);
            }
        }
        spots[taken.len()..].iter().copied().take(n).collect()
    }

    /// A small legal position: everyone has two settlements and a road by each, nobody has
    /// cards, Red (the seat) is to roll on turn 20.
    fn bare() -> (Entered, Arc<Board>) {
        let s = GameState::new(&GameConfig::official(4), 11);
        let topo = &s.board.topology;
        let spots = spread_spots(&s, &[], 8);
        assert_eq!(spots.len(), 8);
        let mut buildings = Vec::new();
        let mut roads = Vec::new();
        for (i, &v) in spots.iter().enumerate() {
            let owner = (i % 4) as PlayerId;
            buildings.push(EnteredBuilding {
                vertex: v,
                owner,
                city: false,
            });
            roads.push(EnteredRoad {
                edge: topo.vertex_edges[v as usize].as_slice()[0],
                owner,
            });
        }
        let e = Entered {
            seat: 0,
            current: 0,
            turn: 20,
            rolled: false,
            robber: 0,
            buildings,
            roads,
            hand: [0; 5],
            dev: [0; 5],
            players: vec![EnteredPlayer::default(); 4],
            bank: [BANK_PER_RESOURCE; 5],
            longest_road: None,
            largest_army: None,
        };
        build(&e, s.board.clone(), 1).expect("the bare position builds");
        (e, s.board.clone())
    }

    /// Audit 2.5: five settlements and a city is a legal position (the city's settlement went
    /// back to the stock and was built again), but the city was placed by putting down a sixth
    /// settlement first and was refused with "no pieces of that kind left".
    #[test]
    fn five_settlements_and_a_city_can_be_entered() {
        let (mut e, board) = bare();
        let s = GameState::new(&GameConfig::official(4), 11);
        let taken: Vec<VertexId> = e.buildings.iter().map(|b| b.vertex).collect();
        let more = spread_spots(&s, &taken, 4);
        assert_eq!(more.len(), 4);
        // Red: three more settlements (five in all) and a city, the city listed first and last.
        for city_first in [true, false] {
            let mut e = e.clone();
            let city = EnteredBuilding {
                vertex: more[3],
                owner: 0,
                city: true,
            };
            if city_first {
                e.buildings.insert(0, city);
            }
            for &v in &more[..3] {
                e.buildings.push(EnteredBuilding {
                    vertex: v,
                    owner: 0,
                    city: false,
                });
            }
            if !city_first {
                e.buildings.push(city);
            }
            let b = build(&e, board.clone(), 1)
                .unwrap_or_else(|err| panic!("five settlements and a city: {err}"));
            let red = &b.state.players[0];
            assert_eq!((red.settlements_left, red.cities_left), (0, 3));
            assert_eq!(b.state.visible_vp(0), 7);
            assert!(
                b.state
                    .building(more[3])
                    .is_some_and(|x| x.city && x.owner == 0)
            );
        }
        // The stocks still bind: a sixth settlement, and a fifth city, are refused.
        let mut six = e.clone();
        for &v in &more {
            six.buildings.push(EnteredBuilding {
                vertex: v,
                owner: 0,
                city: false,
            });
        }
        assert!(
            build(&six, board.clone(), 1)
                .err()
                .unwrap_or_default()
                .contains("no pieces")
        );
        let even_more = spread_spots(&s, &[taken.clone(), more.clone()].concat(), 1);
        for b in e.buildings.iter_mut().filter(|b| b.owner == 0) {
            b.city = true;
        }
        for &v in more[..2].iter().chain(&even_more) {
            e.buildings.push(EnteredBuilding {
                vertex: v,
                owner: 0,
                city: true,
            });
        }
        assert_eq!(
            e.buildings
                .iter()
                .filter(|b| b.owner == 0 && b.city)
                .count(),
            5
        );
        assert!(
            build(&e, board.clone(), 1)
                .err()
                .unwrap_or_default()
                .contains("no pieces")
        );
        // A city beside another building is still too close.
        let (mut e, board) = bare();
        let next = s.board.topology.vertex_neighbors[taken[0] as usize].as_slice()[0];
        e.buildings.push(EnteredBuilding {
            vertex: next,
            owner: 1,
            city: true,
        });
        assert!(
            build(&e, board, 1)
                .err()
                .unwrap_or_default()
                .contains("too close")
        );
    }

    /// Audit 2.6: an award holder who isn't a player indexed past the players and panicked.
    #[test]
    fn an_award_holder_who_is_not_a_player_is_refused() {
        let (good, board) = bare();
        for holder in [4, 5, 6, 7, 8, 100, 255] {
            for road in [true, false] {
                let mut e = good.clone();
                if road {
                    e.longest_road = Some(holder);
                } else {
                    e.largest_army = Some(holder);
                }
                let err = build(&e, board.clone(), 1).err();
                assert!(
                    err.as_deref()
                        .is_some_and(|m| m.contains("isn't in the game")),
                    "holder {holder}: {err:?}"
                );
            }
        }
        // The editing call itself checks too, for 2-player positions.
        let mut s = GameState::empty_position(GameConfig::official(2).rules, board.clone(), 1);
        for holder in [2, 3, 4, 6, 255] {
            assert_eq!(s.set_awards(Some(holder), None), Err(EditError::InvalidId));
            assert_eq!(s.set_awards(None, Some(holder)), Err(EditError::InvalidId));
        }
        assert_eq!(s.set_awards(Some(1), Some(0)), Ok(()));
        // A real player named without the pieces for it is treated as before: no Longest Road,
        // and a Largest Army without knights is refused by the position's own checks.
        let mut e = good.clone();
        e.longest_road = Some(2);
        let b = build(&e, board.clone(), 1).unwrap();
        assert_eq!((b.state.longest_road, b.state.largest_army), (None, None));
        e.largest_army = Some(3);
        assert!(
            build(&e, board, 1)
                .err()
                .unwrap_or_default()
                .contains("largest army held by 3")
        );
    }

    /// Other hostile numbers in a typed-in position are errors, not panics.
    #[test]
    fn hostile_positions_are_refused_not_panicked_on() {
        let (good, board) = bare();
        type Break = fn(&mut Entered);
        let cases: [(&str, Break); 16] = [
            ("seat", |e| e.seat = 255),
            ("current", |e| e.current = 4),
            ("robber", |e| e.robber = 255),
            ("turn", |e| e.turn = 0),
            ("cards", |e| e.players[1].cards = u32::MAX),
            ("cards that wrap", |e| {
                // 2^32 in all: a sum in 32 bits wraps to 0, which is what the bank leaves.
                e.players[1].cards = u32::MAX;
                e.players[2].cards = 1;
            }),
            ("dev cards", |e| e.players[2].dev_cards = u32::MAX),
            ("played", |e| e.players[3].played = [255; 5]),
            ("hand", |e| e.hand = [255; 5]),
            ("dev", |e| e.dev = [255; 5]),
            ("bank", |e| e.bank = [255; 5]),
            ("vertex", |e| e.buildings[0].vertex = u16::MAX),
            ("owner", |e| e.buildings[0].owner = 255),
            ("edge", |e| e.roads[0].edge = u16::MAX),
            ("road owner", |e| e.roads[0].owner = 200),
            ("too many players", |e| {
                e.players = vec![EnteredPlayer::default(); 100_000]
            }),
        ];
        for (what, break_it) in cases {
            let mut e = good.clone();
            break_it(&mut e);
            assert!(build(&e, board.clone(), 1).is_err(), "{what} accepted");
        }
        // Lists far longer than the board.
        let mut e = good.clone();
        e.buildings = vec![good.buildings[0]; 100_000];
        assert!(build(&e, board.clone(), 1).is_err());
        let mut e = good.clone();
        e.roads = vec![good.roads[0]; 100_000];
        assert!(build(&e, board, 1).is_err());
    }

    /// Audit 2.7: the others' hidden development cards are dealt at random, and a dealing could
    /// give the player to move a victory point card that makes ten points, so the built position
    /// was already over (and the calculator said so, every time, the seed being the request's).
    #[test]
    fn nobody_is_dealt_hidden_points_that_already_win() {
        let (mut e, board) = bare();
        let s = GameState::new(&GameConfig::official(4), 11);
        let taken: Vec<VertexId> = e.buildings.iter().map(|b| b.vertex).collect();
        let more = spread_spots(&s, &taken, 3);
        // Blue: four cities and a settlement, 9 points, to move, holding two hidden cards.
        for b in e.buildings.iter_mut().filter(|b| b.owner == 1) {
            b.city = true;
        }
        for (i, &v) in more.iter().enumerate() {
            e.buildings.push(EnteredBuilding {
                vertex: v,
                owner: 1,
                city: i < 2,
            });
        }
        e.current = 1;
        e.rolled = true;
        e.players[1].dev_cards = 2;
        // Before the fix: of these seeds, the ones where Blue drew a victory point card built a
        // finished game (5 of the 25 cards are victory points, so about 4 in 10 dealings).
        let mut blue_cards = std::collections::HashSet::new();
        for seed in 0..200 {
            let b = build(&e, board.clone(), seed).unwrap();
            assert_eq!(b.state.visible_vp(1), 9);
            assert!(
                !b.state.is_over() && b.state.total_vp(1) < 10,
                "seed {seed}: Blue was dealt a win"
            );
            assert_eq!(b.state.players[1].dev_cards_held(), 2);
            blue_cards.insert(b.state.players[1].dev);
        }
        assert!(blue_cards.len() > 1, "the hidden cards still vary");
        // With points to spare, hidden victory points are still dealt.
        let mut far = e.clone();
        far.buildings.retain(|b| !more.contains(&b.vertex));
        let dealt_a_point = (0..200).any(|seed| {
            let b = build(&far, board.clone(), seed).unwrap();
            b.state.players[1].dev[DevCard::VictoryPoint.index()] > 0
        });
        assert!(dealt_a_point);
        // A win on the table is left alone: ten points showing on Blue's turn is a finished game.
        let mut won = e.clone();
        let taken: Vec<VertexId> = won.buildings.iter().map(|b| b.vertex).collect();
        won.buildings.push(EnteredBuilding {
            vertex: spread_spots(&s, &taken, 1)[0],
            owner: 1,
            city: false,
        });
        won.players[1].dev_cards = 0;
        let b = build(&won, board.clone(), 3).unwrap();
        assert_eq!((b.state.visible_vp(1), b.state.winner), (10, Some(1)));
        // When every unseen card is a victory point there is nothing else to deal, and the
        // dealing stands after `DEV_DEALS` tries instead of looping.
        let mut forced = e.clone();
        forced.dev = [14, 0, 2, 2, 2];
        forced.players[1].dev_cards = 1;
        let b = build(&forced, board, 3).unwrap();
        assert_eq!(b.state.winner, Some(1));
    }
}
