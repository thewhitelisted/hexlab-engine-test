//! Consistency checks over a game state, used by tests and the fuzzer.

use crate::bits::Bits;
use crate::state::{
    CITIES_PER_PLAYER, GameState, LARGEST_ARMY_MIN, LONGEST_ROAD_MIN, Phase, ROADS_PER_PLAYER,
    SETTLEMENTS_PER_PLAYER,
};
use crate::types::*;

impl GameState {
    /// Returns a description of the first broken invariant, if any.
    pub fn check_invariants(&self) -> Result<(), String> {
        let n = self.num_players();
        let topo = &self.board.topology;

        // Resource and development card conservation.
        for r in Resource::ALL {
            let held: u32 = self.players[..n].iter().map(|p| p.hand.get(r) as u32).sum();
            let total = held + self.bank.get(r) as u32;
            if total != BANK_PER_RESOURCE as u32 {
                return Err(format!(
                    "{r:?}: {total} cards in play, expected {BANK_PER_RESOURCE}"
                ));
            }
        }
        for c in DevCard::ALL {
            let i = c.index();
            let mut total = self.dev_deck.iter().filter(|&&d| d == c).count() as u32;
            for p in &self.players[..n] {
                total += (p.dev[i] + p.dev_new[i] + p.dev_played[i]) as u32;
            }
            if total != BASE_DEV_DECK[i] as u32 {
                return Err(format!(
                    "{c:?}: {total} cards, expected {}",
                    BASE_DEV_DECK[i]
                ));
            }
        }
        if self.players[..n]
            .iter()
            .any(|p| p.dev_played[DevCard::VictoryPoint.index()] > 0)
        {
            return Err("a victory point card was played".into());
        }

        // Pieces on the board match pieces used, and the distance rule holds.
        let mut settlements = [0u8; MAX_PLAYERS];
        let mut cities = [0u8; MAX_PLAYERS];
        let mut roads = [0u8; MAX_PLAYERS];
        for v in 0..topo.num_vertices() as VertexId {
            if let Some(b) = self.building(v) {
                if b.city {
                    cities[b.owner as usize] += 1;
                } else {
                    settlements[b.owner as usize] += 1;
                }
                for &w in topo.vertex_neighbors[v as usize].as_slice() {
                    if self.building(w).is_some() {
                        return Err(format!("distance rule broken at {v}-{w}"));
                    }
                }
            }
        }
        for e in 0..topo.num_edges() as EdgeId {
            if let Some(q) = self.road_owner(e) {
                roads[q as usize] += 1;
            }
        }
        for (q, p) in self.players[..n].iter().enumerate() {
            if settlements[q] + p.settlements_left != SETTLEMENTS_PER_PLAYER
                || cities[q] + p.cities_left != CITIES_PER_PLAYER
                || roads[q] + p.roads_left != ROADS_PER_PLAYER
            {
                return Err(format!("player {q}: piece counts don't match the board"));
            }
            let reference = self.compute_road_length_reference(q as PlayerId);
            if p.road_length != reference || self.compute_road_length(q as PlayerId) != reference {
                return Err(format!(
                    "player {q}: road length {} but brute force gives {reference}",
                    p.road_length
                ));
            }
        }

        // Bitset mirrors match the vertex and edge arrays, and the fast spot generators agree
        // with the reference predicates.
        let mut occupied = Bits::default();
        let mut blocked = Bits::default();
        let mut road_edges = Bits::default();
        for q in 0..n as PlayerId {
            let mut buildings = Bits::default();
            let mut roads_of = Bits::default();
            for v in 0..topo.num_vertices() as VertexId {
                if self.building(v).is_some_and(|b| b.owner == q) {
                    buildings.set(v);
                    occupied.set(v);
                    blocked |= topo.vertex_block_bits[v as usize];
                }
            }
            for e in 0..topo.num_edges() as EdgeId {
                if self.road_owner(e) == Some(q) {
                    roads_of.set(e);
                    road_edges.set(e);
                }
            }
            let mut ends = Bits::default();
            for e in roads_of.ones() {
                for &v in &topo.edge_vertices[e as usize] {
                    ends.set(v);
                }
            }
            if buildings != self.buildings_of[q as usize]
                || roads_of != self.roads_of[q as usize]
                || ends != self.road_ends_of[q as usize]
            {
                return Err(format!("player {q}: bitsets out of sync"));
            }
            let road_spots = self.road_spots(q);
            for e in 0..topo.num_edges() as EdgeId {
                if road_spots.has(e) != self.can_road(q, e) {
                    return Err(format!("player {q}: road spot {e} disagrees with can_road"));
                }
            }
            if self.turn > 0 {
                let settle_spots = self.settlement_spots(q);
                for v in 0..topo.num_vertices() as VertexId {
                    if settle_spots.has(v) != self.can_settle(q, v) {
                        return Err(format!("player {q}: settlement spot {v} disagrees"));
                    }
                }
            }
        }
        if occupied != self.occupied || blocked != self.blocked || road_edges != self.road_edges {
            return Err("shared bitsets out of sync".into());
        }

        // Every road network is anchored to one of its owner's buildings.
        for q in 0..n as PlayerId {
            if let Some(e) = self.unanchored_road(q) {
                return Err(format!(
                    "player {q}: road {e} is not connected to a building"
                ));
            }
        }

        // Awards.
        let max_len = self.players[..n]
            .iter()
            .map(|p| p.road_length)
            .max()
            .unwrap_or(0);
        match self.longest_road {
            Some(h) => {
                let len = self.players[h as usize].road_length;
                if len < LONGEST_ROAD_MIN || len != max_len {
                    return Err(format!("longest road held by {h} with length {len}"));
                }
            }
            None => {
                let leaders = self.players[..n]
                    .iter()
                    .filter(|p| p.road_length == max_len)
                    .count();
                if max_len >= LONGEST_ROAD_MIN && leaders == 1 {
                    return Err("unique longest road is unclaimed".into());
                }
            }
        }
        let max_knights = self.players[..n]
            .iter()
            .map(|p| p.knights_played())
            .max()
            .unwrap_or(0);
        match self.largest_army {
            Some(h) => {
                let k = self.players[h as usize].knights_played();
                if k < LARGEST_ARMY_MIN || k != max_knights {
                    return Err(format!("largest army held by {h} with {k} knights"));
                }
            }
            None if max_knights >= LARGEST_ARMY_MIN => {
                return Err("largest army is unclaimed".into());
            }
            None => {}
        }

        // Phase bookkeeping.
        let discarding = self.discard_pending.iter().any(|&d| d > 0);
        if discarding != (self.phase == Phase::Discard) {
            return Err("discard bookkeeping out of sync with phase".into());
        }
        let trading = matches!(self.phase, Phase::TradeResponse | Phase::TradeConfirm);
        if trading != self.trade.is_some() {
            return Err("pending trade out of sync with phase".into());
        }
        if (self.robber as usize) >= topo.num_hexes() {
            return Err("robber off the board".into());
        }
        match self.winner {
            Some(w) => {
                if self.phase != Phase::GameOver || w != self.current {
                    return Err("winner set outside game over".into());
                }
                if self.total_vp(w) < self.rules.vp_to_win {
                    return Err("winner below the target".into());
                }
            }
            None => {
                if self.phase == Phase::GameOver {
                    return Err("game over without a winner".into());
                }
                if self.turn > 0 && self.total_vp(self.current) >= self.rules.vp_to_win {
                    return Err("current player has enough points but hasn't won".into());
                }
            }
        }
        Ok(())
    }

    /// A road of `q`'s whose network (walking `q`'s roads, ignoring blocks) reaches no building of `q`.
    fn unanchored_road(&self, q: PlayerId) -> Option<EdgeId> {
        let topo = &self.board.topology;
        let mut reached = vec![false; topo.num_vertices()];
        let mut stack: Vec<VertexId> = (0..topo.num_vertices() as VertexId)
            .filter(|&v| self.building(v).is_some_and(|b| b.owner == q))
            .collect();
        for &v in &stack {
            reached[v as usize] = true;
        }
        while let Some(v) = stack.pop() {
            for &e in topo.vertex_edges[v as usize].as_slice() {
                if self.road_owner(e) == Some(q) {
                    let w = topo.other_end(e, v);
                    if !reached[w as usize] {
                        reached[w as usize] = true;
                        stack.push(w);
                    }
                }
            }
        }
        (0..topo.num_edges() as EdgeId).find(|&e| {
            self.road_owner(e) == Some(q)
                && !topo.edge_vertices[e as usize]
                    .iter()
                    .any(|&v| reached[v as usize])
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::PlayerCount;
    use std::sync::Arc;

    use crate::rng::Rng;
    use crate::rules::GameConfig;
    use crate::sim::{Limits, RandomPolicy, TurnGuard, play_out};
    use crate::state::{GameState, Phase, settlement_code};
    use crate::types::*;

    /// Applies `corrupt` to a copy of `s` and checks that the checker names the problem.
    fn caught(s: &GameState, want: &str, corrupt: impl FnOnce(&mut GameState)) {
        let mut c = s.clone();
        corrupt(&mut c);
        match c.check_invariants() {
            Ok(()) => panic!("not caught: {want}"),
            Err(m) => assert!(m.contains(want), "expected {want:?}, got {m:?}"),
        }
    }

    fn after_setup() -> GameState {
        let mut s = GameState::new(&GameConfig::official(PlayerCount::FOUR), 7);
        let (mut rng, mut guard, limits) = (Rng::seed(7), TurnGuard::default(), Limits::default());
        let mut legal = Vec::new();
        while s.turn == 0 {
            s.legal_actions(&mut legal);
            let a = guard.choose(RandomPolicy::Builder, &limits, &s, &legal, &mut rng);
            s.apply_legal(a);
        }
        s.check_invariants().unwrap();
        s
    }

    /// An edge, and one of its ends, where no building stands or blocks and no road touches
    /// either end.
    fn far_edge(s: &GameState) -> (EdgeId, VertexId) {
        let topo = &s.board.topology;
        let empty = |v: VertexId| {
            !s.blocked.has(v)
                && topo.vertex_edges[v as usize]
                    .as_slice()
                    .iter()
                    .all(|&e| s.road_owner(e).is_none())
        };
        let e = (0..topo.num_edges() as EdgeId)
            .find(|&e| topo.edge_vertices[e as usize].iter().all(|&v| empty(v)))
            .expect("room on the board");
        (e, topo.edge_vertices[e as usize][0])
    }

    #[test]
    fn every_check_catches_the_corruption_it_is_for() {
        let s = after_setup();
        let topo = s.board.topology.clone();
        let home = (0..topo.num_vertices() as VertexId)
            .find(|&v| s.building(v).is_some_and(|b| b.owner == 0))
            .unwrap();
        let road = *topo.vertex_edges[home as usize]
            .as_slice()
            .iter()
            .find(|&&e| s.road_owner(e) == Some(0))
            .unwrap();
        let end = topo.other_end(road, home);
        let beyond = *topo.vertex_edges[end as usize]
            .as_slice()
            .iter()
            .find(|&&e| s.road_owner(e).is_none())
            .unwrap();
        let (far, far_v) = far_edge(&s);

        caught(&s, "Wood: 20 cards", |c| c.bank.add_n(Resource::Wood, 1));
        caught(&s, "Knight: 15 cards", |c| c.dev_deck.push(DevCard::Knight));
        caught(&s, "victory point card was played", |c| {
            let i = c.dev_deck.iter().position(|&d| d == DevCard::VictoryPoint);
            c.dev_deck.remove(i.unwrap());
            c.players[0].dev_played[DevCard::VictoryPoint.index()] += 1;
        });
        caught(&s, "distance rule", |c| {
            c.vertex[end as usize] = settlement_code(1)
        });
        caught(&s, "piece counts", |c| c.players[0].settlements_left += 1);
        caught(&s, "piece counts", |c| c.players[0].cities_left += 1);
        caught(&s, "piece counts", |c| c.players[0].roads_left += 1);
        caught(&s, "brute force", |c| c.players[0].road_length += 1);
        caught(&s, "brute force", |c| c.roads_of[0].set(beyond));
        caught(&s, "bitsets out of sync", |c| c.buildings_of[0].clear(home));
        caught(&s, "bitsets out of sync", |c| c.roads_of[1].set(far));
        caught(&s, "bitsets out of sync", |c| c.road_ends_of[0].set(far_v));
        caught(&s, "road spot", |c| c.road_edges.set(beyond));
        caught(&s, "settlement spot", |c| c.blocked.clear(end));
        caught(&s, "shared bitsets", |c| c.occupied.set(far_v));
        caught(&s, "shared bitsets", |c| c.blocked.set(far_v));
        caught(&s, "shared bitsets", |c| c.road_edges.set(far));
        caught(&s, "not connected to a building", |c| c.place_road(0, far));
        caught(&s, "discard bookkeeping", |c| c.discard_pending[0] = 1);
        caught(&s, "pending trade", |c| c.phase = Phase::TradeResponse);
        caught(&s, "robber off the board", |c| {
            c.robber = topo.num_hexes() as HexId
        });
        caught(&s, "winner set outside game over", |c| c.winner = Some(0));
        caught(&s, "game over without a winner", |c| {
            c.phase = Phase::GameOver
        });
        caught(&s, "enough points but hasn't won", |c| {
            c.rules.vp_to_win = 2
        });

        let mut done = GameState::new(&GameConfig::official(PlayerCount::FOUR), 8);
        let limits = Limits::default();
        let mut rng = Rng::seed(8);
        play_out(
            &mut done,
            RandomPolicy::Builder,
            &limits,
            &mut rng,
            &mut Vec::new(),
        )
        .unwrap();
        done.check_invariants().unwrap();
        caught(&done, "winner set outside game over", |c| {
            c.current = (c.current + 1) % 4
        });
        caught(&done, "winner below the target", |c| c.rules.vp_to_win = 20);
    }

    #[test]
    fn award_checks_catch_a_wrong_holder() {
        // Player 1 has the longer road (6 to 5) and the larger army (4 knights to 3).
        let board = Arc::new(crate::board::Board::generate(
            &crate::board::BoardGen::OFFICIAL,
            &mut Rng::seed(1),
        ));
        let mut s =
            GameState::empty_position(GameConfig::official(PlayerCount::THREE).rules, board, 9)
                .unwrap();
        let topo = s.board.topology.clone();
        let mut used = std::collections::HashSet::new();
        for (p, len) in [(0u8, 5usize), (1, 6)] {
            let path = path_avoiding(&s, len, &used);
            s.put_settlement(p, path[0]).unwrap();
            for w in path.windows(2) {
                s.put_road(p, topo.edge_between(w[0], w[1]).unwrap())
                    .unwrap();
            }
            for &v in &path {
                used.insert(v);
                used.extend(topo.vertex_neighbors[v as usize].as_slice());
            }
        }
        for (p, k) in [(0u8, 3), (1, 4)] {
            for _ in 0..k {
                s.add_played_card(p, DevCard::Knight).unwrap();
            }
        }
        s.finish_edit().unwrap();
        assert_eq!((s.longest_road, s.largest_army), (Some(1), Some(1)));

        caught(&s, "longest road held by 0", |c| c.longest_road = Some(0));
        caught(&s, "longest road held by 2", |c| c.longest_road = Some(2));
        caught(&s, "unique longest road is unclaimed", |c| {
            c.longest_road = None
        });
        caught(&s, "largest army held by 0", |c| c.largest_army = Some(0));
        caught(&s, "largest army held by 2", |c| c.largest_army = Some(2));
        caught(&s, "largest army is unclaimed", |c| c.largest_army = None);
    }

    /// A simple path of `len` edges whose vertices avoid `avoid`.
    fn path_avoiding(
        s: &GameState,
        len: usize,
        avoid: &std::collections::HashSet<VertexId>,
    ) -> Vec<VertexId> {
        let topo = &s.board.topology;
        fn extend(
            topo: &crate::topology::Topology,
            path: &mut Vec<VertexId>,
            len: usize,
            avoid: &std::collections::HashSet<VertexId>,
        ) -> bool {
            if path.len() == len + 1 {
                return true;
            }
            let last = *path.last().unwrap();
            for &n in topo.vertex_neighbors[last as usize].as_slice() {
                if !avoid.contains(&n) && !path.contains(&n) {
                    path.push(n);
                    if extend(topo, path, len, avoid) {
                        return true;
                    }
                    path.pop();
                }
            }
            false
        }
        for start in 0..topo.num_vertices() as VertexId {
            let mut path = vec![start];
            if !avoid.contains(&start) && extend(topo, &mut path, len, avoid) {
                return path;
            }
        }
        panic!("no path of length {len}");
    }
}
