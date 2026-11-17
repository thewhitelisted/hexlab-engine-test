//! Road length and the Longest Road award.

use crate::bits::Bits;
use crate::state::{GameState, LONGEST_ROAD_MIN};
use crate::types::{EdgeId, PlayerId, VertexId};

impl GameState {
    /// Longest trail through `p`'s roads (no edge used twice) that doesn't pass through a vertex
    /// holding an opponent's building.
    ///
    /// Search starts only at road ends, branch points and opponent-blocked vertices: a longest
    /// trail starting anywhere else could be extended backwards, or is a closed loop that can be
    /// restarted at a branch point. Loop components with no such vertex are searched from any
    /// vertex. [`GameState::compute_road_length_reference`] is the brute-force check.
    pub fn compute_road_length(&self, p: PlayerId) -> u8 {
        let mut remaining = self.roads_of[p as usize];
        let mut best = 0;
        while let Some(e) = remaining.ones().next() {
            let component = self.road_network(p, e);
            remaining = remaining & !component;
            best = best.max(self.network_longest(p, component, best));
        }
        best
    }

    /// Road length after `p` adds road `e`: only the network containing `e` can have changed.
    pub(crate) fn road_length_after_adding(&self, p: PlayerId, e: EdgeId) -> u8 {
        let old = self.players[p as usize].road_length;
        let network = self.road_network(p, e);
        old.max(self.network_longest(p, network, old))
    }

    /// `p`'s roads connected to road `e` (through any vertex, blocked or not).
    fn road_network(&self, p: PlayerId, e: EdgeId) -> Bits {
        let topo = &self.board.topology;
        let roads = self.roads_of[p as usize];
        let mut network = Bits::default();
        network.set(e);
        let mut frontier = network;
        while !frontier.is_empty() {
            let mut next = Bits::default();
            for f in frontier.ones() {
                for &v in &topo.edge_vertices[f as usize] {
                    next |= topo.vertex_edge_bits[v as usize];
                }
            }
            frontier = next & roads & !network;
            network |= frontier;
        }
        network
    }

    /// Longest trail within one road network, or `floor` if it can't beat `floor`.
    fn network_longest(&self, p: PlayerId, network: Bits, floor: u8) -> u8 {
        let size = network.count() as u8;
        if size <= floor {
            return floor;
        }
        let topo = &self.board.topology;
        let (mut once, mut twice, mut thrice) = (Bits::default(), Bits::default(), Bits::default());
        for e in network.ones() {
            for &v in &topo.edge_vertices[e as usize] {
                if twice.has(v) {
                    thrice.set(v);
                } else if once.has(v) {
                    twice.set(v);
                } else {
                    once.set(v);
                }
            }
        }
        let opponents = self.occupied & !self.buildings_of[p as usize];
        let starts = (once & !twice) | thrice | (once & opponents);
        if starts.is_empty() {
            // An unbroken loop: the trail goes all the way round.
            return size;
        }
        if size as u32 + 1 == once.count() && (once & opponents).is_empty() {
            // A tree nobody has cut: the longest trail is its diameter.
            let any = starts.ones().next().expect("non-empty");
            let (far, _) = self.farthest_in_tree(network, any);
            let (_, diameter) = self.farthest_in_tree(network, far);
            return floor.max(diameter);
        }
        let mut best = floor;
        let mut used = Bits::default();
        for v in starts.ones() {
            self.road_dfs(p, network, v, 0, size, &mut used, &mut best);
            if best == size {
                break;
            }
        }
        best
    }

    /// Farthest vertex from `from` along an acyclic road network, and its distance in roads.
    fn farthest_in_tree(&self, network: Bits, from: VertexId) -> (VertexId, u8) {
        let topo = &self.board.topology;
        let mut stack = [(0 as VertexId, EdgeId::MAX, 0u8); 32];
        stack[0] = (from, EdgeId::MAX, 0);
        let mut top = 1;
        let mut best = (from, 0);
        while top > 0 {
            top -= 1;
            let (v, via, dist) = stack[top];
            if dist > best.1 {
                best = (v, dist);
            }
            for e in (topo.vertex_edge_bits[v as usize] & network).ones() {
                if e != via {
                    stack[top] = (topo.other_end(e, v), e, dist + 1);
                    top += 1;
                }
            }
        }
        best
    }

    /// Brute-force road length: a full search from every vertex on `p`'s roads.
    pub fn compute_road_length_reference(&self, p: PlayerId) -> u8 {
        let topo = &self.board.topology;
        let roads = self.roads_of[p as usize];
        let mut best = 0;
        let mut used = Bits::default();
        for e in roads.ones() {
            for &v in &topo.edge_vertices[e as usize] {
                self.road_dfs(p, roads, v, 0, u8::MAX, &mut used, &mut best);
            }
        }
        best
    }

    /// Extends trails from `v` over unused edges of `network`, stopping once `best` hits `limit`.
    #[allow(clippy::too_many_arguments)]
    fn road_dfs(
        &self,
        p: PlayerId,
        network: Bits,
        v: VertexId,
        len: u8,
        limit: u8,
        used: &mut Bits,
        best: &mut u8,
    ) {
        *best = (*best).max(len);
        if *best >= limit {
            return;
        }
        if len > 0
            && let Some(b) = self.building(v)
            && b.owner != p
        {
            return;
        }
        let topo = &self.board.topology;
        let next = topo.vertex_edge_bits[v as usize] & network & !*used;
        for e in next.ones() {
            used.set(e);
            self.road_dfs(p, network, topo.other_end(e, v), len + 1, limit, used, best);
            used.clear(e);
        }
    }

    /// Recomputes road lengths of opponents whose roads meet at `v`, where the current player
    /// just built (a building cuts other players' roads, never its owner's).
    pub(crate) fn recompute_roads_through(&mut self, v: VertexId) {
        let mut touched = 0u8;
        for &e in self.board.topology.vertex_edges[v as usize].as_slice() {
            if let Some(owner) = self.road_owner(e)
                && owner != self.current
            {
                touched |= 1 << owner;
            }
        }
        for q in 0..self.rules.num_players {
            if touched & (1 << q) != 0 {
                self.players[q as usize].road_length = self.compute_road_length(q);
            }
        }
    }

    /// Reassigns Longest Road from current road lengths. The holder keeps it while tied for the
    /// longest; otherwise it goes to the unique longest road of at least 5, and is set aside when
    /// the longest is shared or too short.
    pub(crate) fn update_longest_road(&mut self) {
        let n = self.num_players();
        let lengths = |q: usize| self.players[q].road_length;
        let max = (0..n).map(lengths).max().unwrap_or(0);
        if max < LONGEST_ROAD_MIN {
            self.longest_road = None;
            return;
        }
        if let Some(h) = self.longest_road
            && lengths(h as usize) == max
        {
            return;
        }
        let mut leaders = (0..n).filter(|&q| lengths(q) == max);
        let first = leaders.next();
        self.longest_road = match (first, leaders.next()) {
            (Some(q), None) => Some(q as PlayerId),
            _ => None,
        };
    }
}
