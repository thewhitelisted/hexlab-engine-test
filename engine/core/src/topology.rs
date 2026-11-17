//! Board geometry: hexes, the vertices (intersections) and edges (paths) between them.
//!
//! Hexes are pointy-top and addressed by axial coordinates. Vertices are identified by an integer
//! lattice position, so a topology can be built from any set of land hexes (base board, 5-6
//! player board, sea scenarios) and ids are stable for a given hex list.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};

use crate::bits::Bits;
use crate::types::{EdgeId, HexId, VertexId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Axial {
    pub q: i8,
    pub r: i8,
}

/// Axial neighbour offsets, in order around the hex.
pub const DIRECTIONS: [(i8, i8); 6] = [(1, 0), (1, -1), (0, -1), (-1, 0), (-1, 1), (0, 1)];

/// Corner offsets on the vertex lattice, clockwise from the top corner. A hex at (q, r) has its
/// centre at lattice (2q + r, 3r); x is scaled by 2/sqrt(3) and y by 2 so corners are integers.
const CORNER_OFFSETS: [(i16, i16); 6] = [(0, -2), (1, -1), (1, 1), (0, 2), (-1, 1), (-1, -1)];

impl Axial {
    pub const fn new(q: i8, r: i8) -> Axial {
        Axial { q, r }
    }

    /// The third cube coordinate. Only for hexes within [`MAX_COORD`] of the origin (every hex
    /// of a checked board): further out it doesn't fit.
    pub const fn s(self) -> i8 {
        -self.q - self.r
    }

    /// Steps between two hexes. Worked in wider integers, so it is right for any two
    /// coordinates, including ones read from a layout that hasn't been checked yet.
    pub fn distance(self, other: Axial) -> i32 {
        let dq = self.q as i32 - other.q as i32;
        let dr = self.r as i32 - other.r as i32;
        (dq.abs() + dr.abs() + (dq + dr).abs()) / 2
    }

    /// Whether both coordinates are within [`MAX_COORD`] of the origin.
    pub fn in_range(self) -> bool {
        self.q.unsigned_abs() <= MAX_COORD as u8 && self.r.unsigned_abs() <= MAX_COORD as u8
    }

    /// The hex next to this one in direction `dir` (0 to 5). Only for hexes `in_range`.
    pub fn neighbor(self, dir: usize) -> Axial {
        let (dq, dr) = DIRECTIONS[dir];
        Axial::new(self.q + dq, self.r + dr)
    }

    /// Lattice position of corner `k` (0 = top, clockwise).
    pub fn corner(self, k: usize) -> (i16, i16) {
        let (dx, dy) = CORNER_OFFSETS[k];
        (
            2 * self.q as i16 + self.r as i16 + dx,
            3 * self.r as i16 + dy,
        )
    }
}

/// Hexes at exactly `radius` from the origin, walking once around the ring.
pub fn ring(radius: i8) -> Vec<Axial> {
    if radius == 0 {
        return vec![Axial::new(0, 0)];
    }
    let (dq, dr) = DIRECTIONS[4];
    let mut h = Axial::new(dq * radius, dr * radius);
    let mut out = Vec::with_capacity(6 * radius as usize);
    for dir in 0..6 {
        for _ in 0..radius {
            out.push(h);
            h = h.neighbor(dir);
        }
    }
    out
}

/// Fixed-capacity adjacency list (a vertex touches at most 3 hexes, vertices and edges).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Adj<T: Copy + Default, const N: usize> {
    items: [T; N],
    len: u8,
}

impl<T: Copy + Default, const N: usize> Default for Adj<T, N> {
    fn default() -> Self {
        Adj {
            items: [T::default(); N],
            len: 0,
        }
    }
}

impl<T: Copy + Default, const N: usize> Adj<T, N> {
    fn push(&mut self, item: T) {
        self.items[self.len as usize] = item;
        self.len += 1;
    }

    #[inline]
    pub fn as_slice(&self) -> &[T] {
        &self.items[..self.len as usize]
    }
}

/// Largest vertex and edge counts supported by the fixed-size bitsets.
pub const MAX_VERTICES: usize = 256;
pub const MAX_EDGES: usize = 256;
/// The most land hexes a board may have (the base game has 19, the 5-6 player board 30).
pub const MAX_HEXES: usize = 64;
/// The furthest a hex coordinate may be from the origin, so that no arithmetic on coordinates
/// can leave an `i8`.
pub const MAX_COORD: i8 = 30;

/// Why a set of hexes isn't a board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopologyError {
    NoHexes,
    TooManyHexes(usize),
    OutOfRange(Axial),
    DuplicateHex(Axial),
    TooManyVertices,
    TooManyEdges,
}

impl std::fmt::Display for TopologyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TopologyError::NoHexes => write!(f, "a board needs at least one hex"),
            TopologyError::TooManyHexes(n) => {
                write!(f, "{n} hexes: a board has at most {MAX_HEXES}")
            }
            TopologyError::OutOfRange(a) => write!(
                f,
                "hex {a:?} is too far out: coordinates go from -{MAX_COORD} to {MAX_COORD}"
            ),
            TopologyError::DuplicateHex(a) => write!(f, "duplicate hex at {a:?}"),
            TopologyError::TooManyVertices => {
                write!(f, "the board has more than {MAX_VERTICES} corners")
            }
            TopologyError::TooManyEdges => write!(f, "the board has more than {MAX_EDGES} edges"),
        }
    }
}

impl std::error::Error for TopologyError {}

#[derive(Debug)]
pub struct Topology {
    /// Land hexes, indexed by [`HexId`].
    pub hexes: Vec<Axial>,
    /// Corner vertices of each hex, clockwise from the top.
    pub hex_vertices: Vec<[VertexId; 6]>,
    pub vertex_pos: Vec<(i16, i16)>,
    pub vertex_hexes: Vec<Adj<HexId, 3>>,
    pub vertex_neighbors: Vec<Adj<VertexId, 3>>,
    pub vertex_edges: Vec<Adj<EdgeId, 3>>,
    /// Endpoints of each edge, lower id first.
    pub edge_vertices: Vec<[VertexId; 2]>,
    /// Edges meeting at each vertex, as a bitset.
    pub vertex_edge_bits: Vec<Bits>,
    /// Each vertex together with its neighbours (the spots a building there blocks).
    pub vertex_block_bits: Vec<Bits>,
    pub all_vertices: Bits,
    pub all_edges: Bits,
    hex_index: HashMap<Axial, HexId>,
    vertex_index: HashMap<(i16, i16), VertexId>,
}

impl Topology {
    /// Builds the topology for a set of land hexes known to be a board (the standard one).
    /// Panics on anything else: hexes from outside go through [`Topology::try_from_hexes`].
    pub fn from_hexes(hexes: &[Axial]) -> Topology {
        Topology::try_from_hexes(hexes).expect("a valid set of hexes")
    }

    /// Builds the topology for a set of land hexes. Ids follow the order of `hexes`. The hexes
    /// are checked first: at least one and at most [`MAX_HEXES`], each once, within
    /// [`MAX_COORD`] of the origin, and few enough corners and edges for the bitsets.
    pub fn try_from_hexes(hexes: &[Axial]) -> Result<Topology, TopologyError> {
        if hexes.is_empty() {
            return Err(TopologyError::NoHexes);
        }
        if hexes.len() > MAX_HEXES {
            return Err(TopologyError::TooManyHexes(hexes.len()));
        }
        let mut hex_index = HashMap::new();
        for (i, &h) in hexes.iter().enumerate() {
            if !h.in_range() {
                return Err(TopologyError::OutOfRange(h));
            }
            // At most MAX_HEXES hexes, so the index fits a HexId.
            if hex_index.insert(h, i as HexId).is_some() {
                return Err(TopologyError::DuplicateHex(h));
            }
        }

        let mut vertex_index: HashMap<(i16, i16), VertexId> = HashMap::new();
        let mut vertex_pos = Vec::new();
        let mut hex_vertices = Vec::with_capacity(hexes.len());
        for &h in hexes {
            let mut corners = [0; 6];
            for (k, corner) in corners.iter_mut().enumerate() {
                let pos = h.corner(k);
                *corner = *vertex_index.entry(pos).or_insert_with(|| {
                    vertex_pos.push(pos);
                    (vertex_pos.len() - 1) as VertexId
                });
            }
            hex_vertices.push(corners);
        }
        // At most 6 * MAX_HEXES corners were numbered, which fits a VertexId; more than the
        // bitsets hold is refused before anything is indexed by them.
        if vertex_pos.len() > MAX_VERTICES {
            return Err(TopologyError::TooManyVertices);
        }

        let nv = vertex_pos.len();
        let mut vertex_hexes = vec![Adj::default(); nv];
        for (h, corners) in hex_vertices.iter().enumerate() {
            for &v in corners {
                vertex_hexes[v as usize].push(h as HexId);
            }
        }

        let mut edge_index: HashMap<(VertexId, VertexId), EdgeId> = HashMap::new();
        let mut edge_vertices = Vec::new();
        for corners in &hex_vertices {
            for k in 0..6 {
                let (a, b) = (corners[k], corners[(k + 1) % 6]);
                let key = (a.min(b), a.max(b));
                edge_index.entry(key).or_insert_with(|| {
                    edge_vertices.push([key.0, key.1]);
                    (edge_vertices.len() - 1) as EdgeId
                });
            }
        }
        if edge_vertices.len() > MAX_EDGES {
            return Err(TopologyError::TooManyEdges);
        }

        let mut vertex_neighbors = vec![Adj::default(); nv];
        let mut vertex_edges = vec![Adj::default(); nv];
        for (e, &[a, b]) in edge_vertices.iter().enumerate() {
            vertex_neighbors[a as usize].push(b);
            vertex_neighbors[b as usize].push(a);
            vertex_edges[a as usize].push(e as EdgeId);
            vertex_edges[b as usize].push(e as EdgeId);
        }

        let vertex_edge_bits = (0..nv)
            .map(|v| {
                let mut b = Bits::default();
                for &e in vertex_edges[v].as_slice() {
                    b.set(e);
                }
                b
            })
            .collect();
        let vertex_block_bits = (0..nv)
            .map(|v| {
                let mut b = Bits::default();
                b.set(v as VertexId);
                for &n in vertex_neighbors[v].as_slice() {
                    b.set(n);
                }
                b
            })
            .collect();

        Ok(Topology {
            hexes: hexes.to_vec(),
            hex_vertices,
            vertex_pos,
            vertex_hexes,
            vertex_neighbors,
            vertex_edges,
            all_vertices: Bits::first(nv),
            all_edges: Bits::first(edge_vertices.len()),
            edge_vertices,
            vertex_edge_bits,
            vertex_block_bits,
            hex_index,
            vertex_index,
        })
    }

    /// Hex positions of the standard 19-hex board, in row order (top row first).
    pub fn standard_hexes() -> &'static [Axial] {
        static HEXES: OnceLock<Vec<Axial>> = OnceLock::new();
        HEXES.get_or_init(|| {
            let mut out = Vec::with_capacity(19);
            for r in -2i8..=2 {
                for q in (-2 - r).max(-2)..=(2 - r).min(2) {
                    out.push(Axial::new(q, r));
                }
            }
            out
        })
    }

    /// The shared topology of the standard 19-hex board.
    pub fn standard() -> Arc<Topology> {
        static STANDARD: OnceLock<Arc<Topology>> = OnceLock::new();
        STANDARD
            .get_or_init(|| Arc::new(Topology::from_hexes(Topology::standard_hexes())))
            .clone()
    }

    #[inline]
    pub fn num_hexes(&self) -> usize {
        self.hexes.len()
    }

    #[inline]
    pub fn num_vertices(&self) -> usize {
        self.vertex_pos.len()
    }

    #[inline]
    pub fn num_edges(&self) -> usize {
        self.edge_vertices.len()
    }

    pub fn hex_at(&self, a: Axial) -> Option<HexId> {
        self.hex_index.get(&a).copied()
    }

    pub fn vertex_at(&self, pos: (i16, i16)) -> Option<VertexId> {
        self.vertex_index.get(&pos).copied()
    }

    pub fn edge_between(&self, a: VertexId, b: VertexId) -> Option<EdgeId> {
        self.vertex_edges[a as usize]
            .as_slice()
            .iter()
            .copied()
            .find(|&e| self.other_end(e, a) == b)
    }

    #[inline]
    pub fn other_end(&self, e: EdgeId, v: VertexId) -> VertexId {
        let [a, b] = self.edge_vertices[e as usize];
        if a == v { b } else { a }
    }

    /// The two board vertices shared by two adjacent hex positions (either may be off-board,
    /// e.g. a sea hex holding a port). `None` if the hexes aren't adjacent or the corners aren't
    /// on the board.
    pub fn shared_corners(&self, a: Axial, b: Axial) -> Option<[VertexId; 2]> {
        if a.distance(b) != 1 {
            return None;
        }
        let mut found = Vec::with_capacity(2);
        for k in 0..6 {
            let pos = a.corner(k);
            if (0..6).any(|j| b.corner(j) == pos) {
                found.push(self.vertex_at(pos)?);
            }
        }
        (found.len() == 2).then(|| [found[0], found[1]])
    }

    /// True for vertices that touch fewer than three land hexes.
    pub fn is_coastal(&self, v: VertexId) -> bool {
        self.vertex_hexes[v as usize].as_slice().len() < 3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_board_counts() {
        let t = Topology::standard();
        assert_eq!(t.num_hexes(), 19);
        assert_eq!(t.num_vertices(), 54);
        assert_eq!(t.num_edges(), 72);
        let coastal = (0..54).filter(|&v| t.is_coastal(v)).count();
        assert_eq!(coastal, 30);
        for v in 0..t.num_vertices() {
            let deg = t.vertex_neighbors[v].as_slice().len();
            assert!(deg == 2 || deg == 3);
            assert_eq!(deg, t.vertex_edges[v].as_slice().len());
        }
    }

    /// Audit 2.4: hexes from a layout sent to the server reached `assert!`s here (too many
    /// edges, too many corners, a duplicate), and the distance arithmetic overflowed an `i8`.
    #[test]
    fn hostile_hex_sets_are_refused_not_panicked_on() {
        let line = |n: usize| -> Vec<Axial> {
            // Hexes spaced apart share no corner: 6 corners and 6 edges each.
            (0..n)
                .map(|i| Axial::new((i % 16) as i8 * 3 - 24, (i / 16) as i8 * 3 - 6))
                .collect()
        };
        let cases: Vec<(Vec<Axial>, TopologyError)> = vec![
            (vec![], TopologyError::NoHexes),
            (line(MAX_HEXES + 1), TopologyError::TooManyHexes(65)),
            (
                (0..300)
                    .map(|i| Axial::new((i % 20) as i8, (i / 20) as i8))
                    .collect(),
                TopologyError::TooManyHexes(300),
            ),
            (
                vec![Axial::new(i8::MIN, i8::MIN)],
                TopologyError::OutOfRange(Axial::new(i8::MIN, i8::MIN)),
            ),
            (
                vec![Axial::new(0, 0), Axial::new(i8::MAX, 0)],
                TopologyError::OutOfRange(Axial::new(i8::MAX, 0)),
            ),
            (
                vec![Axial::new(0, MAX_COORD + 1)],
                TopologyError::OutOfRange(Axial::new(0, MAX_COORD + 1)),
            ),
            (
                vec![Axial::new(-MAX_COORD - 1, 0)],
                TopologyError::OutOfRange(Axial::new(-MAX_COORD - 1, 0)),
            ),
            (
                vec![Axial::new(1, 1), Axial::new(0, 0), Axial::new(1, 1)],
                TopologyError::DuplicateHex(Axial::new(1, 1)),
            ),
            // 64 separate hexes have 384 corners.
            (line(MAX_HEXES), TopologyError::TooManyVertices),
            // 43 separate hexes: 258 corners.
            (line(43), TopologyError::TooManyVertices),
        ];
        for (hexes, want) in cases {
            assert_eq!(
                Topology::try_from_hexes(&hexes).err(),
                Some(want),
                "{} hexes starting {:?}",
                hexes.len(),
                hexes.first()
            );
        }
        // A solid block of 64 hexes fits: 160 corners and 223 edges.
        let block: Vec<Axial> = (0..64).map(|i| Axial::new(i % 8 - 4, i / 8 - 4)).collect();
        let t = Topology::try_from_hexes(&block).unwrap();
        assert_eq!((t.num_vertices(), t.num_edges()), (160, 223));
        // The furthest hexes allowed, and the widest distances, don't overflow.
        let far = [
            Axial::new(MAX_COORD, MAX_COORD),
            Axial::new(-MAX_COORD, -MAX_COORD),
        ];
        let t = Topology::try_from_hexes(&far).unwrap();
        assert_eq!(
            (t.num_hexes(), t.num_vertices(), t.num_edges()),
            (2, 12, 12)
        );
        assert_eq!(far[0].distance(far[1]), 4 * MAX_COORD as i32);
        assert_eq!(
            Axial::new(i8::MIN, i8::MIN).distance(Axial::new(i8::MAX, i8::MAX)),
            510
        );
        assert_eq!(Axial::new(i8::MIN, i8::MAX).distance(Axial::new(0, 0)), 128);
        for k in 0..6 {
            let n = far[0].neighbor(k);
            assert_eq!(n.distance(far[0]), 1);
            assert!(t.shared_corners(n, far[0]).is_some(), "{k}");
        }
        assert_eq!(
            t.shared_corners(Axial::new(i8::MIN, i8::MIN), Axial::new(i8::MAX, i8::MAX)),
            None
        );
    }

    #[test]
    fn rings_have_expected_sizes() {
        assert_eq!(ring(0), vec![Axial::new(0, 0)]);
        assert_eq!(ring(1).len(), 6);
        assert_eq!(ring(2).len(), 12);
        assert_eq!(ring(3).len(), 18);
        for h in ring(3) {
            assert_eq!(h.distance(Axial::new(0, 0)), 3);
        }
    }

    #[test]
    fn adjacent_hexes_share_two_corners() {
        let t = Topology::standard();
        let shared = t
            .shared_corners(Axial::new(0, 0), Axial::new(1, 0))
            .unwrap();
        let center = t.hex_vertices[t.hex_at(Axial::new(0, 0)).unwrap() as usize];
        assert!(center.contains(&shared[0]) && center.contains(&shared[1]));
        assert!(t.edge_between(shared[0], shared[1]).is_some());
    }
}
