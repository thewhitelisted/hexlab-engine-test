//! A concrete board: terrain, number tokens, ports and robber start, plus generators.

use std::fmt;
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};

use crate::bits::Bits;
use crate::rng::Rng;
use crate::rules::PlayerCount;
use crate::topology::{Axial, MAX_HEXES, Topology, TopologyError, ring};
use crate::types::{HexId, Port, Resource, VertexId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Tile {
    /// `None` for the desert.
    pub resource: Option<Resource>,
    pub number: Option<u8>,
}

/// One land hex in a [`BoardLayout`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HexSpec {
    pub q: i8,
    pub r: i8,
    pub resource: Option<Resource>,
    pub number: Option<u8>,
}

/// A port on the coast: the sea hex holding it and the land hex whose shared edge it serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PortSpot {
    pub kind: Port,
    pub sea: Axial,
    pub land: Axial,
}

/// Serializable board description, used for records, the analysis board and tests.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BoardLayout {
    pub hexes: Vec<HexSpec>,
    pub ports: Vec<PortSpot>,
    /// Robber start; defaults to the (first) desert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub robber: Option<Axial>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardError {
    DuplicateHex(Axial),
    BadNumber(Axial, u8),
    NumberMismatch(Axial),
    PortNotOnCoast(Axial, Axial),
    NoRobberStart,
    RobberOffBoard(Axial),
    /// The hexes aren't a board: none, too many, too far out, or too many corners or edges.
    Hexes(TopologyError),
    TooManyPorts(usize),
    /// One land hex: a robber moved by a 7 or a knight has nowhere to go.
    NoRobberMove,
    /// The initial placement of this many players can reach a turn with no spot left for the
    /// player to move: some settlements, fewer than two each, leave every corner taken or next
    /// to one.
    SetupCanStall(PlayerCount),
}

impl fmt::Display for BoardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BoardError::DuplicateHex(a) => write!(f, "duplicate hex at {a:?}"),
            BoardError::BadNumber(a, n) => write!(f, "hex {a:?} has invalid number {n}"),
            BoardError::NumberMismatch(a) => {
                write!(
                    f,
                    "hex {a:?}: resource hexes need a number and the desert must not have one"
                )
            }
            BoardError::PortNotOnCoast(s, l) => {
                write!(
                    f,
                    "port at sea {s:?} facing {l:?} is not on a coast edge of the board"
                )
            }
            BoardError::NoRobberStart => write!(f, "no desert and no robber start given"),
            BoardError::RobberOffBoard(a) => write!(f, "robber start {a:?} is not a land hex"),
            BoardError::Hexes(e) => write!(f, "{e}"),
            BoardError::TooManyPorts(n) => {
                write!(f, "{n} ports: a board has at most {MAX_PORTS}")
            }
            BoardError::NoRobberMove => {
                write!(f, "one land hex: the robber needs another hex to move to")
            }
            BoardError::SetupCanStall(n) => write!(
                f,
                "too few corners for {n} players: the initial placement can leave a player no spot"
            ),
        }
    }
}

impl std::error::Error for BoardError {}

/// The most ports a layout may list (the base game has 9).
pub const MAX_PORTS: usize = 64;

#[derive(Debug)]
pub struct Board {
    pub topology: Arc<Topology>,
    /// Tiles indexed by [`HexId`].
    pub tiles: Vec<Tile>,
    pub ports: Vec<(PortSpot, [VertexId; 2])>,
    pub vertex_port: Vec<Option<Port>>,
    pub robber_start: HexId,
    /// Hexes carrying each number token (index = number, 2..=12).
    pub hexes_by_number: [Vec<HexId>; 13],
}

impl Board {
    pub fn from_layout(layout: &BoardLayout) -> Result<Board, BoardError> {
        // A layout comes from outside (a request, a record, a file): its sizes are checked
        // before anything is built from it.
        if layout.hexes.len() > MAX_HEXES {
            return Err(BoardError::Hexes(TopologyError::TooManyHexes(
                layout.hexes.len(),
            )));
        }
        if layout.ports.len() > MAX_PORTS {
            return Err(BoardError::TooManyPorts(layout.ports.len()));
        }
        let positions: Vec<Axial> = layout.hexes.iter().map(|h| Axial::new(h.q, h.r)).collect();
        let mut sorted = positions.clone();
        sorted.sort();
        if let Some(w) = sorted.windows(2).find(|w| w[0] == w[1]) {
            return Err(BoardError::DuplicateHex(w[0]));
        }
        // 19 distinct hexes within radius 2 are exactly the standard board.
        let origin = Axial::new(0, 0);
        let is_standard =
            positions.len() == 19 && positions.iter().all(|a| a.distance(origin) <= 2);
        let topology = if is_standard {
            Topology::standard()
        } else {
            Arc::new(Topology::try_from_hexes(&positions).map_err(BoardError::Hexes)?)
        };

        let mut tiles = vec![
            Tile {
                resource: None,
                number: None
            };
            topology.num_hexes()
        ];
        for spec in &layout.hexes {
            let a = Axial::new(spec.q, spec.r);
            if let Some(n) = spec.number
                && (!(2..=12).contains(&n) || n == 7)
            {
                return Err(BoardError::BadNumber(a, n));
            }
            if spec.resource.is_some() != spec.number.is_some() {
                return Err(BoardError::NumberMismatch(a));
            }
            let id = topology
                .hex_at(a)
                .ok_or(BoardError::Hexes(TopologyError::OutOfRange(a)))?;
            tiles[id as usize] = Tile {
                resource: spec.resource,
                number: spec.number,
            };
        }

        let mut ports = Vec::with_capacity(layout.ports.len());
        let mut vertex_port = vec![None; topology.num_vertices()];
        for spot in &layout.ports {
            let on_coast =
                topology.hex_at(spot.sea).is_none() && topology.hex_at(spot.land).is_some();
            let corners = topology
                .shared_corners(spot.sea, spot.land)
                .filter(|_| on_coast)
                .ok_or(BoardError::PortNotOnCoast(spot.sea, spot.land))?;
            for v in corners {
                vertex_port[v as usize] = Some(spot.kind);
            }
            ports.push((*spot, corners));
        }

        let robber_start = match layout.robber {
            Some(a) => topology.hex_at(a).ok_or(BoardError::RobberOffBoard(a))?,
            None => tiles
                .iter()
                .position(|t| t.resource.is_none())
                .ok_or(BoardError::NoRobberStart)? as HexId,
        };

        let mut hexes_by_number: [Vec<HexId>; 13] = Default::default();
        for (h, t) in tiles.iter().enumerate() {
            if let Some(n) = t.number {
                hexes_by_number[n as usize].push(h as HexId);
            }
        }

        Ok(Board {
            topology,
            tiles,
            ports,
            vertex_port,
            robber_start,
            hexes_by_number,
        })
    }

    pub fn to_layout(&self) -> BoardLayout {
        let hexes = self
            .topology
            .hexes
            .iter()
            .zip(&self.tiles)
            .map(|(a, t)| HexSpec {
                q: a.q,
                r: a.r,
                resource: t.resource,
                number: t.number,
            })
            .collect();
        BoardLayout {
            hexes,
            ports: self.ports.iter().map(|(spot, _)| *spot).collect(),
            robber: Some(self.topology.hexes[self.robber_start as usize]),
        }
    }

    /// Whether the robber, moved by a 7 or a knight, always has a hex to go to: a second land hex.
    pub fn robber_can_move(&self) -> Result<(), BoardError> {
        if self.topology.num_hexes() < 2 {
            return Err(BoardError::NoRobberMove);
        }
        Ok(())
    }

    /// Whether every game of `players` on this board, from the initial placement on, has a move
    /// at every decision. Most decisions always have one (rolling, ending the turn, declining a
    /// trade); two depend on the board:
    ///
    /// - moving the robber ([`Board::robber_can_move`]);
    /// - a settlement in the initial placement, which needs a corner that isn't taken or next to
    ///   one. The settlements placed so far are corners no two of which are neighbours; the
    ///   placement stalls when fewer than `2 * players` of them cover every corner (each corner
    ///   taken or next to one taken). [`covers_within`] looks for such a set.
    ///
    /// Roads in the initial placement never stall: a road there touches its own settlement, so a
    /// corner next to someone else's road is next to a settlement too.
    pub fn hosts(&self, players: PlayerCount) -> Result<(), BoardError> {
        self.robber_can_move()?;
        let fewer = 2 * usize::from(players.get()) - 1;
        // A settlement covers itself and at most three neighbours, so `fewer` settlements cover
        // at most `4 * fewer` corners: on a board with more, nothing stalls.
        if self.topology.num_vertices() > 4 * fewer {
            return Ok(());
        }
        if covers_within(&self.topology, Bits::default(), fewer) {
            return Err(BoardError::SetupCanStall(players));
        }
        Ok(())
    }

    /// Generates a standard 19-hex board.
    pub fn generate(spec: &BoardGen, rng: &mut Rng) -> Board {
        let layout = generate_standard_layout(spec, rng);
        Board::from_layout(&layout).expect("generated layout is valid")
    }

    /// Dots on a number token: the number of 36 dice outcomes that roll it.
    pub fn pips(number: u8) -> u8 {
        match number {
            2..=6 => number - 1,
            8..=12 => 13 - number,
            _ => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NumberPlacement {
    /// Tokens shuffled onto the non-desert hexes.
    Random,
    /// Official variable setup: tokens A-R laid in a spiral from a random corner, skipping the desert.
    OfficialSpiral,
}

/// How to generate a random standard board.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BoardGen {
    pub numbers: NumberPlacement,
    /// Reject boards where two 6/8 tokens are on adjacent hexes.
    pub separate_red_numbers: bool,
    pub shuffle_ports: bool,
}

impl BoardGen {
    pub const OFFICIAL: BoardGen = BoardGen {
        numbers: NumberPlacement::OfficialSpiral,
        separate_red_numbers: false,
        shuffle_ports: true,
    };

    pub const RANDOM_BALANCED: BoardGen = BoardGen {
        numbers: NumberPlacement::Random,
        separate_red_numbers: true,
        shuffle_ports: true,
    };
}

/// Terrain of the base game: 4 forest, 3 hills, 4 pasture, 4 fields, 3 mountains, 1 desert.
const BASE_TERRAIN: [Option<Resource>; 19] = {
    use Resource::*;
    [
        Some(Wood),
        Some(Wood),
        Some(Wood),
        Some(Wood),
        Some(Brick),
        Some(Brick),
        Some(Brick),
        Some(Sheep),
        Some(Sheep),
        Some(Sheep),
        Some(Sheep),
        Some(Wheat),
        Some(Wheat),
        Some(Wheat),
        Some(Wheat),
        Some(Ore),
        Some(Ore),
        Some(Ore),
        None,
    ]
};

const BASE_NUMBERS: [u8; 18] = [2, 3, 3, 4, 4, 5, 5, 6, 6, 8, 8, 9, 9, 10, 10, 11, 11, 12];

/// Official variable-setup token order A..R.
const OFFICIAL_TOKEN_ORDER: [u8; 18] = [5, 2, 6, 3, 8, 10, 9, 12, 11, 4, 8, 10, 9, 4, 5, 6, 3, 11];

const BASE_PORTS: [Port; 9] = [
    Port::Generic,
    Port::Generic,
    Port::Generic,
    Port::Generic,
    Port::Special(Resource::Wood),
    Port::Special(Resource::Brick),
    Port::Special(Resource::Sheep),
    Port::Special(Resource::Wheat),
    Port::Special(Resource::Ore),
];

/// Port locations of the standard board as on the physical frame: every other hex of the
/// surrounding sea ring, as (sea hex, land hex whose shared edge the port serves). This is also
/// the layout common open-source engines use, so boards map one to one.
pub fn standard_port_spots() -> &'static [(Axial, Axial)] {
    const SPOTS: [((i8, i8), (i8, i8)); 9] = [
        ((-3, 3), (-2, 2)),
        ((-1, 3), (-1, 2)),
        ((1, 2), (1, 1)),
        ((3, 0), (2, 0)),
        ((3, -2), (2, -1)),
        ((2, -3), (1, -2)),
        ((0, -3), (0, -2)),
        ((-2, -1), (-1, -1)),
        ((-3, 1), (-2, 1)),
    ];
    static AXIAL: OnceLock<Vec<(Axial, Axial)>> = OnceLock::new();
    AXIAL.get_or_init(|| {
        SPOTS
            .iter()
            .map(|&((sq, sr), (lq, lr))| (Axial::new(sq, sr), Axial::new(lq, lr)))
            .collect()
    })
}

/// Standard-board hex indices in official spiral order, starting from each of the 6 corners.
fn spiral_orders() -> &'static [[usize; 19]; 6] {
    static ORDERS: OnceLock<[[usize; 19]; 6]> = OnceLock::new();
    ORDERS.get_or_init(|| {
        let positions = Topology::standard_hexes();
        std::array::from_fn(|corner| {
            let mut outer = ring(2);
            outer.rotate_left(2 * corner);
            let mut inner = ring(1);
            inner.rotate_left(corner);
            let spiral: Vec<usize> = outer
                .into_iter()
                .chain(inner)
                .chain([Axial::new(0, 0)])
                .map(|a| {
                    positions
                        .iter()
                        .position(|&p| p == a)
                        .expect("spiral on board")
                })
                .collect();
            spiral.try_into().expect("19 hexes")
        })
    })
}

/// Pairs of adjacent hexes on the standard board.
fn standard_adjacent_pairs() -> &'static [(usize, usize)] {
    static PAIRS: OnceLock<Vec<(usize, usize)>> = OnceLock::new();
    PAIRS.get_or_init(|| {
        let positions = Topology::standard_hexes();
        let mut out = Vec::new();
        for i in 0..positions.len() {
            for j in (i + 1)..positions.len() {
                if positions[i].distance(positions[j]) == 1 {
                    out.push((i, j));
                }
            }
        }
        out
    })
}

fn generate_standard_layout(spec: &BoardGen, rng: &mut Rng) -> BoardLayout {
    let positions = Topology::standard_hexes();
    loop {
        let mut terrain = BASE_TERRAIN;
        rng.shuffle(&mut terrain);
        let mut numbers = [None; 19];
        match spec.numbers {
            NumberPlacement::Random => {
                let mut tokens = BASE_NUMBERS;
                rng.shuffle(&mut tokens);
                let mut it = tokens.iter();
                for (i, t) in terrain.iter().enumerate() {
                    if t.is_some() {
                        numbers[i] = it.next().copied();
                    }
                }
            }
            NumberPlacement::OfficialSpiral => {
                let order = &spiral_orders()[rng.below(6) as usize];
                let mut tokens = OFFICIAL_TOKEN_ORDER.iter();
                for &i in order {
                    if terrain[i].is_some() {
                        numbers[i] = tokens.next().copied();
                    }
                }
            }
        }
        if spec.separate_red_numbers && red_numbers_touch(&numbers) {
            continue;
        }

        let mut kinds = BASE_PORTS;
        if spec.shuffle_ports {
            rng.shuffle(&mut kinds);
        }
        let ports = standard_port_spots()
            .iter()
            .zip(kinds)
            .map(|(&(sea, land), kind)| PortSpot { kind, sea, land })
            .collect();

        let hexes = positions
            .iter()
            .enumerate()
            .map(|(i, a)| HexSpec {
                q: a.q,
                r: a.r,
                resource: terrain[i],
                number: numbers[i],
            })
            .collect();
        return BoardLayout {
            hexes,
            ports,
            robber: None,
        };
    }
}

/// Whether `left` more settlements or fewer, each on a corner `covered` doesn't hold (taken or
/// next to one taken), can leave every corner covered. The first uncovered corner must end up
/// covered by a settlement on it or on a neighbour, so trying each of those (at most four, all
/// uncovered) tries every such set, `left` deep.
fn covers_within(topo: &Topology, covered: Bits, left: usize) -> bool {
    let Some(v) = (topo.all_vertices & !covered).ones().next() else {
        return true;
    };
    left > 0
        && (topo.vertex_block_bits[usize::from(v)] & !covered)
            .ones()
            .any(|w| {
                covers_within(
                    topo,
                    covered | topo.vertex_block_bits[usize::from(w)],
                    left - 1,
                )
            })
}

/// Whether two 6/8 tokens sit on adjacent hexes (numbers indexed like the standard hexes).
fn red_numbers_touch(numbers: &[Option<u8>]) -> bool {
    let red = |n: Option<u8>| matches!(n, Some(6) | Some(8));
    standard_adjacent_pairs()
        .iter()
        .any(|&(i, j)| red(numbers[i]) && red(numbers[j]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hexes of an official board in the order its tokens were laid, found from the board
    /// alone: walk the outer ring from a corner, then the inner ring from the hex touching both
    /// the first and last outer hexes, then the centre. Returns the walk that reads the official
    /// token order (skipping the desert), and whether it turns counterclockwise on screen.
    fn official_spiral(layout: &BoardLayout) -> Option<(Vec<usize>, bool)> {
        let at = |a: Axial| layout.hexes.iter().position(|h| Axial::new(h.q, h.r) == a);
        let centre = Axial::new(0, 0);
        let ring = |d: i32| -> Vec<Axial> {
            layout
                .hexes
                .iter()
                .map(|h| Axial::new(h.q, h.r))
                .filter(|a| a.distance(centre) == d)
                .collect()
        };
        // Walk a ring from `start` through `next`, always to the unvisited ring neighbour.
        let walk = |ring: &[Axial], start: Axial, next: Axial| -> Vec<Axial> {
            let mut out = vec![start, next];
            while out.len() < ring.len() {
                let last = *out.last().unwrap();
                let step = ring
                    .iter()
                    .copied()
                    .find(|&a| a.distance(last) == 1 && !out.contains(&a))
                    .expect("a ring is a cycle");
                out.push(step);
            }
            out
        };
        let (outer, inner) = (ring(2), ring(1));
        // Corners of the outer ring: one coordinate is 0.
        let corners = outer
            .iter()
            .copied()
            .filter(|a| a.q == 0 || a.r == 0 || a.s() == 0);
        for c in corners {
            for next in outer.iter().copied().filter(|a| a.distance(c) == 1) {
                let o = walk(&outer, c, next);
                let last = *o.last().unwrap();
                let Some(i0) = inner
                    .iter()
                    .copied()
                    .find(|a| a.distance(c) == 1 && a.distance(last) == 1)
                else {
                    continue;
                };
                // The inner ring continues the same way round as the outer one.
                for i1 in inner.iter().copied().filter(|a| a.distance(i0) == 1) {
                    let i = walk(&inner, i0, i1);
                    let order: Vec<usize> = o
                        .iter()
                        .chain(&i)
                        .chain([&centre])
                        .map(|&a| at(a).unwrap())
                        .collect();
                    let tokens: Vec<u8> = order
                        .iter()
                        .filter_map(|&k| layout.hexes[k].number)
                        .collect();
                    if tokens == OFFICIAL_TOKEN_ORDER {
                        // Screen y grows downward, so a counterclockwise turn has a negative cross
                        // product in lattice coordinates (x = 2q + r, y = 3r).
                        let xy = |a: Axial| ((2 * a.q + a.r) as i32, (3 * a.r) as i32);
                        let ((x0, y0), (x1, y1)) = (xy(o[0]), xy(o[1]));
                        let same_way =
                            |(ax, ay): (i32, i32), (bx, by): (i32, i32)| ax * by - bx * ay;
                        let outer_turn = same_way((x0, y0), (x1, y1));
                        let inner_turn = same_way(xy(i[0]), xy(i[1]));
                        if outer_turn.signum() == inner_turn.signum() {
                            return Some((order, outer_turn < 0));
                        }
                    }
                }
            }
        }
        None
    }

    #[test]
    fn official_tokens_spiral_inward_counterclockwise_from_a_corner() {
        let mut rng = Rng::seed(12);
        let mut starts = std::collections::HashSet::new();
        for _ in 0..200 {
            let layout = generate_standard_layout(&BoardGen::OFFICIAL, &mut rng);
            let (order, counterclockwise) =
                official_spiral(&layout).expect("tokens A-R along a spiral from a corner");
            assert!(
                counterclockwise,
                "the rulebook lays tokens counterclockwise"
            );
            starts.insert(order[0]);
        }
        assert_eq!(starts.len(), 6, "the spiral starts from every corner");
    }

    #[test]
    fn harbours_are_four_generic_and_one_of_each_resource_on_alternate_sea_hexes() {
        let mut generic = 0;
        let mut special = Vec::new();
        for p in BASE_PORTS {
            match p {
                Port::Generic => generic += 1,
                Port::Special(r) => special.push(r),
            }
        }
        special.sort_by_key(|r| r.index());
        assert_eq!(generic, 4);
        assert_eq!(special, Resource::ALL.to_vec());
        let centre = Axial::new(0, 0);
        let spots = standard_port_spots();
        for (k, &(sea, land)) in spots.iter().enumerate() {
            assert_eq!(sea.distance(centre), 3, "harbour {k} is in the sea ring");
            assert_eq!(
                land.distance(centre),
                2,
                "harbour {k} faces an outer land hex"
            );
            assert_eq!(sea.distance(land), 1, "harbour {k} touches its land hex");
            for &(other, _) in &spots[k + 1..] {
                // Nine of the ring's eighteen, none next to another: every other sea hex.
                assert!(
                    sea.distance(other) >= 2,
                    "harbours {sea:?} and {other:?} touch"
                );
            }
        }
    }

    #[test]
    fn generated_boards_have_base_contents() {
        for (seed, spec) in [(1, BoardGen::OFFICIAL), (2, BoardGen::RANDOM_BALANCED)] {
            let mut rng = Rng::seed(seed);
            for _ in 0..50 {
                let b = Board::generate(&spec, &mut rng);
                let mut counts = [0; 5];
                let mut deserts = 0;
                let mut nums: Vec<u8> = Vec::new();
                for t in &b.tiles {
                    match t.resource {
                        Some(r) => counts[r.index()] += 1,
                        None => deserts += 1,
                    }
                    nums.extend(t.number);
                }
                nums.sort();
                assert_eq!(counts, [4, 3, 4, 4, 3]);
                assert_eq!(deserts, 1);
                assert_eq!(nums, BASE_NUMBERS.to_vec());
                assert_eq!(b.ports.len(), 9);
                assert!(b.tiles[b.robber_start as usize].resource.is_none());
            }
        }
    }

    #[test]
    fn port_vertices_are_distinct_and_coastal() {
        let mut rng = Rng::seed(3);
        let b = Board::generate(&BoardGen::OFFICIAL, &mut rng);
        let mut vs: Vec<VertexId> = b.ports.iter().flat_map(|(_, v)| *v).collect();
        assert!(vs.iter().all(|&v| b.topology.is_coastal(v)));
        vs.sort();
        vs.dedup();
        assert_eq!(vs.len(), 18);
    }

    /// Whether two 6/8 tokens sit on hexes sharing an edge, read from the board's own topology
    /// rather than the generator's table.
    fn reds_touch(b: &Board) -> bool {
        let t = &b.topology;
        let red = |h: usize| matches!(b.tiles[h].number, Some(6) | Some(8));
        let share_edge = |i: usize, j: usize| {
            t.hex_vertices[i]
                .iter()
                .filter(|v| t.hex_vertices[j].contains(v))
                .count()
                == 2
        };
        let n = t.num_hexes();
        (0..n).any(|i| (i + 1..n).any(|j| red(i) && red(j) && share_edge(i, j)))
    }

    #[test]
    fn separated_reds_never_touch() {
        let mut rng = Rng::seed(4);
        for _ in 0..200 {
            assert!(!reds_touch(&Board::generate(
                &BoardGen::RANDOM_BALANCED,
                &mut rng
            )));
        }
        // Without the separation, random tokens often touch: the check can fail.
        let loose = BoardGen {
            separate_red_numbers: false,
            ..BoardGen::RANDOM_BALANCED
        };
        assert!((0..50).any(|_| reds_touch(&Board::generate(&loose, &mut rng))));
    }

    #[test]
    fn layouts_are_checked_and_get_their_own_topology() {
        let mut rng = Rng::seed(6);
        let layout = Board::generate(&BoardGen::OFFICIAL, &mut rng).to_layout();
        let standard = Board::from_layout(&layout).unwrap();
        assert!(
            Arc::ptr_eq(&standard.topology, &Topology::standard()),
            "standard boards share one topology"
        );

        // The same 19 hexes moved off centre are not the standard board.
        let mut moved = layout.clone();
        for h in &mut moved.hexes {
            h.q += 3;
        }
        moved.ports.clear();
        moved.robber = None;
        let b = Board::from_layout(&moved).unwrap();
        assert!(!Arc::ptr_eq(&b.topology, &Topology::standard()));
        for h in &moved.hexes {
            let id = b
                .topology
                .hex_at(Axial::new(h.q, h.r))
                .expect("each hex is on its board");
            assert_eq!(b.tiles[id as usize].number, h.number);
        }
        // Fewer hexes, all within the standard radius.
        let mut small = layout.clone();
        small
            .hexes
            .retain(|h| Axial::new(h.q, h.r).distance(Axial::new(0, 0)) <= 1);
        small.ports.clear();
        small.robber = None;
        if !small.hexes.iter().any(|h| h.resource.is_none()) {
            small.hexes[0] = HexSpec {
                resource: None,
                number: None,
                ..small.hexes[0]
            };
        }
        assert_eq!(Board::from_layout(&small).unwrap().topology.num_hexes(), 7);

        // Each hex appears once.
        let mut l = layout.clone();
        l.hexes[1].q = l.hexes[0].q;
        l.hexes[1].r = l.hexes[0].r;
        assert!(matches!(
            Board::from_layout(&l),
            Err(BoardError::DuplicateHex(_))
        ));

        // Numbers: 2 to 12 without 7, and only on resource hexes.
        let land = layout
            .hexes
            .iter()
            .position(|h| h.resource.is_some())
            .unwrap();
        for bad in [0, 1, 7, 13] {
            let mut l = layout.clone();
            l.hexes[land].number = Some(bad);
            assert!(
                matches!(Board::from_layout(&l), Err(BoardError::BadNumber(_, n)) if n == bad),
                "{bad}"
            );
        }
        let mut l = layout.clone();
        l.hexes[land].number = None;
        assert!(matches!(
            Board::from_layout(&l),
            Err(BoardError::NumberMismatch(_))
        ));

        // A harbour needs a sea hex beside a land hex.
        let mut l = layout.clone();
        l.ports[0].sea = l.ports[0].land;
        assert!(matches!(
            Board::from_layout(&l),
            Err(BoardError::PortNotOnCoast(..))
        ));
        let mut l = layout.clone();
        l.ports[0].land = l.ports[0].sea;
        assert!(matches!(
            Board::from_layout(&l),
            Err(BoardError::PortNotOnCoast(..))
        ));
        // Two land hexes, or two sea hexes, that share an edge aren't a coast either.
        let port = layout.ports[0];
        let is_land = |a: Axial| layout.hexes.iter().any(|h| Axial::new(h.q, h.r) == a);
        let near = |a: Axial| {
            [(1, 0), (-1, 0), (0, 1), (0, -1), (1, -1), (-1, 1)]
                .map(|(dq, dr)| Axial::new(a.q + dq, a.r + dr))
        };
        let mut l = layout.clone();
        l.ports[0].sea = near(port.land).into_iter().find(|&a| is_land(a)).unwrap();
        assert!(matches!(
            Board::from_layout(&l),
            Err(BoardError::PortNotOnCoast(..))
        ));
        let mut l = layout.clone();
        l.ports[0].land = near(port.sea).into_iter().find(|&a| !is_land(a)).unwrap();
        assert!(matches!(
            Board::from_layout(&l),
            Err(BoardError::PortNotOnCoast(..))
        ));
    }

    /// Audit 2.4: a crafted layout sent to `/api/placement/analyze` panicked here (coordinates
    /// whose distance overflowed an `i8`, and boards with more corners or edges than the bitsets
    /// hold). Every hostile layout is an `Err`.
    #[test]
    fn hostile_layouts_are_refused_not_panicked_on() {
        use crate::topology::MAX_COORD;
        let layout = Board::generate(&BoardGen::OFFICIAL, &mut Rng::seed(6)).to_layout();
        let hex = |q: i8, r: i8| HexSpec {
            q,
            r,
            resource: None,
            number: None,
        };
        let with_hexes = |hexes: Vec<HexSpec>| BoardLayout {
            hexes,
            ports: vec![],
            robber: None,
        };
        let mut cases: Vec<(&str, BoardLayout)> = Vec::new();
        // Huge coordinates: one hex, and one of the standard 19 moved to each extreme.
        for (q, r) in [
            (i8::MIN, i8::MIN),
            (i8::MAX, i8::MAX),
            (i8::MIN, i8::MAX),
            (i8::MAX, i8::MIN),
            (i8::MIN, 0),
            (0, i8::MAX),
            (100, 100),
            (-100, 27),
            (31, 0),
            (0, -31),
        ] {
            cases.push(("far hex alone", with_hexes(vec![hex(q, r)])));
            let mut l = layout.clone();
            l.hexes[0].q = q;
            l.hexes[0].r = r;
            cases.push(("far hex among 19", l));
        }
        // No hexes; with and without a robber or a port.
        cases.push(("zero hexes", with_hexes(vec![])));
        let mut l = with_hexes(vec![]);
        l.robber = Some(Axial::new(0, 0));
        cases.push(("zero hexes with a robber", l));
        let mut l = layout.clone();
        l.hexes.clear();
        cases.push(("zero hexes with ports", l));
        // Duplicates.
        cases.push(("the same hex twice", with_hexes(vec![hex(0, 0), hex(0, 0)])));
        let mut l = layout.clone();
        l.hexes.push(l.hexes[3]);
        cases.push(("20 hexes, one twice", l));
        // Far too many hexes: every position an i8 pair has, and a few sizes above the limit.
        let all: Vec<HexSpec> = (i8::MIN..=i8::MAX)
            .flat_map(|q| (i8::MIN..=i8::MAX).map(move |r| hex(q, r)))
            .collect();
        assert_eq!(all.len(), 65536);
        cases.push(("65536 hexes", with_hexes(all)));
        for n in [65usize, 257, 400] {
            let many = (0..n)
                .map(|i| hex((i % 20) as i8 - 10, (i / 20) as i8 - 10))
                .collect();
            cases.push(("too many hexes", with_hexes(many)));
        }
        // Few enough hexes, too many corners: 60 hexes none of which touch.
        let apart = (0..60)
            .map(|i| hex((i % 10) * 3 - 15, (i / 10) * 3 - 9))
            .collect();
        cases.push(("too many corners", with_hexes(apart)));
        // Ports: far out at sea, and far too many.
        for sea in [
            Axial::new(i8::MIN, i8::MIN),
            Axial::new(i8::MAX, i8::MAX),
            Axial::new(i8::MAX, i8::MIN),
        ] {
            let mut l = layout.clone();
            l.ports[0].sea = sea;
            cases.push(("port far out at sea", l.clone()));
            l.ports[0].land = sea;
            cases.push(("port wholly far out", l));
        }
        let mut l = layout.clone();
        l.ports = vec![layout.ports[0]; MAX_PORTS + 1];
        cases.push(("too many ports", l));
        let mut l = layout.clone();
        l.ports = vec![layout.ports[0]; 100_000];
        cases.push(("far too many ports", l));
        // The robber far off the board.
        let mut l = layout.clone();
        l.robber = Some(Axial::new(i8::MIN, i8::MAX));
        cases.push(("robber far away", l));

        for (what, l) in &cases {
            let got = Board::from_layout(l);
            assert!(got.is_err(), "{what}: {} hexes accepted", l.hexes.len());
            // And the error can be shown.
            assert!(!got.unwrap_err().to_string().is_empty(), "{what}");
        }
        // The kinds of error, by case.
        assert_eq!(
            Board::from_layout(&with_hexes(vec![])).unwrap_err(),
            BoardError::Hexes(TopologyError::NoHexes)
        );
        assert_eq!(
            Board::from_layout(&with_hexes(vec![hex(i8::MIN, i8::MIN)])).unwrap_err(),
            BoardError::Hexes(TopologyError::OutOfRange(Axial::new(i8::MIN, i8::MIN)))
        );
        // Valid layouts are read as before: the standard board, and one desert on its own.
        assert!(Board::from_layout(&layout).is_ok());
        let one = Board::from_layout(&with_hexes(vec![hex(MAX_COORD, -MAX_COORD)]));
        assert_eq!(one.unwrap().topology.num_vertices(), 6);
    }

    #[test]
    fn ports_keep_the_base_order_unless_shuffled() {
        let kinds = |spec: &BoardGen, seed: u64| -> Vec<Port> {
            let b = Board::generate(spec, &mut Rng::seed(seed));
            b.to_layout().ports.iter().map(|p| p.kind).collect()
        };
        let fixed = BoardGen {
            shuffle_ports: false,
            ..BoardGen::OFFICIAL
        };
        for seed in 0..4 {
            assert_eq!(kinds(&fixed, seed), BASE_PORTS);
        }
        assert!((0..4).any(|seed| kinds(&BoardGen::OFFICIAL, seed) != BASE_PORTS));
    }

    #[test]
    fn layout_round_trips() {
        let mut rng = Rng::seed(5);
        let b = Board::generate(&BoardGen::OFFICIAL, &mut rng);
        let layout = b.to_layout();
        let json = serde_json::to_string(&layout).unwrap();
        let back: BoardLayout = serde_json::from_str(&json).unwrap();
        let b2 = Board::from_layout(&back).unwrap();
        assert_eq!(b2.to_layout(), layout);
    }

    #[test]
    fn pips_match_dice_odds() {
        let total: u32 = (2..=12).map(|n| Board::pips(n) as u32).sum();
        assert_eq!(total, 30); // 36 outcomes minus the six 7s
        assert_eq!(Board::pips(6), 5);
        assert_eq!(Board::pips(12), 1);
    }

    /// Every board of deserts on some of the hexes of the 7-hex flower (each non-empty subset,
    /// 127 boards, joined or not) and on a line of 2 to 8 hexes.
    fn small_boards() -> Vec<(String, Board)> {
        let flower: Vec<Axial> = std::iter::once(Axial::new(0, 0)).chain(ring(1)).collect();
        let mut sets: Vec<Vec<Axial>> = (1..1u32 << flower.len())
            .map(|mask| {
                (0..flower.len())
                    .filter(|&i| mask & (1 << i) != 0)
                    .map(|i| flower[i])
                    .collect()
            })
            .collect();
        sets.extend((2..=8).map(|n| (0..n).map(|q| Axial::new(q, 0)).collect()));
        sets.into_iter()
            .map(|hexes| {
                let layout = BoardLayout {
                    hexes: hexes
                        .iter()
                        .map(|a| HexSpec {
                            q: a.q,
                            r: a.r,
                            resource: None,
                            number: None,
                        })
                        .collect(),
                    ports: vec![],
                    robber: None,
                };
                (format!("{hexes:?}"), Board::from_layout(&layout).unwrap())
            })
            .collect()
    }

    /// The reference for the initial placement's stall, by brute force: every set of corners no
    /// two of which are neighbours, smallest first, and whether one of fewer than `settlements`
    /// leaves no corner open.
    fn stall_by_brute_force(topo: &Topology, settlements: usize) -> bool {
        fn grow(topo: &Topology, chosen: &mut Vec<usize>, from: usize, left: usize) -> bool {
            let nv = topo.num_vertices();
            let open = |v: usize, chosen: &[usize]| {
                chosen.iter().all(|&c| {
                    c != v
                        && !topo.vertex_neighbors[c]
                            .as_slice()
                            .contains(&(v as VertexId))
                })
            };
            if (0..nv).all(|v| !open(v, chosen)) {
                return true;
            }
            if left == 0 {
                return false;
            }
            for v in from..nv {
                if open(v, chosen) {
                    chosen.push(v);
                    let found = grow(topo, chosen, v + 1, left - 1);
                    chosen.pop();
                    if found {
                        return true;
                    }
                }
            }
            false
        }
        grow(topo, &mut Vec::new(), 0, settlements - 1)
    }

    /// Issue #23: a board with no possible game was read, and a game on it panicked when the
    /// first decision had no move. `hosts` agrees with the brute-force reference on every small
    /// board and player count, refuses some of them for each count and accepts some, and some
    /// of those it accepts are small enough that it searched them (rather than counting corners).
    #[test]
    fn a_board_hosts_a_game_exactly_when_no_decision_can_run_out_of_moves() {
        let mut verdicts = [[0; 2]; 3];
        let mut searched_and_hosted = 0;
        for (what, board) in small_boards() {
            for (i, n) in [PlayerCount::TWO, PlayerCount::THREE, PlayerCount::FOUR]
                .into_iter()
                .enumerate()
            {
                let topo = &board.topology;
                let want = if topo.num_hexes() < 2 {
                    Err(BoardError::NoRobberMove)
                } else if stall_by_brute_force(topo, 2 * usize::from(n.get())) {
                    Err(BoardError::SetupCanStall(n))
                } else {
                    Ok(())
                };
                assert_eq!(board.hosts(n), want, "{what}, {n} players");
                verdicts[i][usize::from(want.is_ok())] += 1;
                let fewer = 2 * usize::from(n.get()) - 1;
                if want.is_ok() && topo.num_vertices() <= 4 * fewer {
                    searched_and_hosted += 1;
                }
            }
        }
        assert!(searched_and_hosted > 0);
        for (n, [refused, hosted]) in (2..).zip(verdicts) {
            assert!(
                refused > 0 && hosted > 0,
                "{n} players: {refused} refused, {hosted} hosted"
            );
        }
        for seed in 0..20 {
            let board = Board::generate(&BoardGen::OFFICIAL, &mut Rng::seed(seed));
            for n in [PlayerCount::TWO, PlayerCount::THREE, PlayerCount::FOUR] {
                assert_eq!(board.hosts(n), Ok(()));
            }
        }
    }

    /// `hosts` skips the search on a board of more than `4(2n-1)` corners: 12, 20 or 28. At
    /// exactly that many, a stall would take `2n-1` settlements covering every corner, each its
    /// own and three neighbours' and no corner twice: a perfect cover. A board's perfect cover is
    /// one of each of its pieces (its joined groups of hexes), and adding a hex never takes a
    /// corner away. So every joined board of up to 28 corners is grown here, one hex at a time
    /// from a single hex, and the corner counts of those with a perfect cover are collected. No
    /// sum of those counts is 12, 20 or 28, so no board at the bound has a stall: skipping the
    /// search there too would give the same answers (the shortcut's two surviving mutants, `>=`
    /// and `==` for `>`, are equivalent). The 7-hex flower, of 24 corners, has one; 24 is the
    /// bound of no player count.
    #[test]
    fn no_board_at_the_corner_bound_has_a_perfect_cover() {
        use crate::topology::DIRECTIONS;
        use std::collections::{BTreeSet, HashSet};
        type Shape = BTreeSet<(i8, i8)>;
        let at_origin = |s: Shape| -> Shape {
            let q = s.iter().map(|h| h.0).min().unwrap_or(0);
            let r = s.iter().map(|h| h.1).min().unwrap_or(0);
            s.into_iter().map(|(a, b)| (a - q, b - r)).collect()
        };
        let mut seen: HashSet<Shape> = HashSet::new();
        let mut to_grow = vec![Shape::from([(0, 0)])];
        let mut covered: BTreeSet<usize> = BTreeSet::new();
        let (mut shapes, mut checked) = (0, 0);
        while let Some(shape) = to_grow.pop() {
            let hexes: Vec<Axial> = shape.iter().map(|&(q, r)| Axial::new(q, r)).collect();
            let topo = Topology::try_from_hexes(&hexes).unwrap();
            let corners = topo.num_vertices();
            if corners > 28 {
                continue;
            }
            shapes += 1;
            if corners.is_multiple_of(4) {
                checked += 1;
                if covers_within(&topo, Bits::default(), corners / 4) {
                    covered.insert(corners);
                }
            }
            for &(q, r) in &shape {
                for (dq, dr) in DIRECTIONS {
                    let mut grown = shape.clone();
                    if grown.insert((q + dq, r + dr)) {
                        let grown = at_origin(grown);
                        if seen.insert(grown.clone()) {
                            to_grow.push(grown);
                        }
                    }
                }
            }
        }
        // The totals that pieces with a perfect cover make, any number of each, up to 28.
        let mut reachable = [false; 29];
        reachable[0] = true;
        for total in 1..=28 {
            reachable[total] = covered.iter().any(|&c| c <= total && reachable[total - c]);
        }
        for bound in [12, 20, 28] {
            assert!(!reachable[bound], "{bound}: pieces of {covered:?} corners");
        }
        assert_eq!(covered, BTreeSet::from([24]), "only the 7-hex flower");
        assert!(
            shapes > 1000 && checked > 100,
            "{shapes} shapes, {checked} checked"
        );
    }

    /// Every small board that hosts a game of `n` holds random games to the end or the turn
    /// limit, for both random policies: no decision without a move. So does every board with
    /// a hex for the robber to move to, from an empty position after the initial placement.
    #[test]
    fn every_hosted_board_plays_out() {
        use crate::sim::{Limits, RandomPolicy, play_out};
        use crate::{GameConfig, GameState};
        let limits = Limits {
            max_turns: 40,
            ..Limits::default()
        };
        let mut games = 0;
        for (what, board) in small_boards() {
            let board = Arc::new(board);
            for n in [PlayerCount::TWO, PlayerCount::THREE, PlayerCount::FOUR] {
                let rules = GameConfig::official(n).rules;
                let from_start = GameState::with_board(rules, board.clone(), Rng::seed(1));
                assert_eq!(from_start.is_ok(), board.hosts(n).is_ok(), "{what}");
                let after = GameState::empty_position(rules, board.clone(), 1);
                assert_eq!(after.is_ok(), board.robber_can_move().is_ok(), "{what}");
                let starts = [from_start, after].into_iter().flatten();
                for (start, (seed, policy)) in starts.flat_map(|s| {
                    [(1, RandomPolicy::Uniform), (2, RandomPolicy::Builder)].map(|p| (s.clone(), p))
                }) {
                    let mut s = start;
                    play_out(
                        &mut s,
                        policy,
                        &limits,
                        &mut Rng::seed(seed),
                        &mut Vec::new(),
                    );
                    assert!(s.is_over() || s.turn > limits.max_turns, "{what}, {n}");
                    games += 1;
                }
            }
        }
        assert!(games > 1000, "{games} games");
    }
}

/// Kani proof (docs/TRACEABILITY.md, "Kani").
#[cfg(kani)]
mod proofs {
    use super::*;

    #[kani::proof]
    #[kani::unwind(8)]
    fn pips_are_the_ways_two_dice_roll_the_number() {
        let n: u8 = kani::any();
        let mut ways = 0u8;
        for a in 1..=6u8 {
            for b in 1..=6u8 {
                ways += (a + b == n) as u8;
            }
        }
        // A 7 has no token.
        assert_eq!(Board::pips(n), if n == 7 { 0 } else { ways });
    }
}
