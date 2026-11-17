//! Full game state (the referee's view, including hidden information).

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::bits::Bits;
use crate::board::Board;
use crate::dice::Dice;
use crate::rng::Rng;
use crate::rules::{GameConfig, Rules};
use crate::types::*;

pub const ROADS_PER_PLAYER: u8 = 15;
pub const SETTLEMENTS_PER_PLAYER: u8 = 5;
pub const CITIES_PER_PLAYER: u8 = 4;

/// Longest Road needs at least this many connected segments.
pub const LONGEST_ROAD_MIN: u8 = 5;
/// Largest Army needs at least this many knights played.
pub const LARGEST_ARMY_MIN: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlayerState {
    pub hand: Hand,
    /// Development cards held from earlier turns (playable).
    pub dev: DevCounts,
    /// Development cards bought this turn (not playable until next turn).
    pub dev_new: DevCounts,
    pub dev_played: DevCounts,
    pub roads_left: u8,
    pub settlements_left: u8,
    pub cities_left: u8,
    /// Length of this player's longest road.
    pub road_length: u8,
    /// Port bits, see [`Port::bit`].
    pub ports: u8,
}

impl Default for PlayerState {
    fn default() -> Self {
        PlayerState {
            hand: Hand::EMPTY,
            dev: [0; NUM_DEV_TYPES],
            dev_new: [0; NUM_DEV_TYPES],
            dev_played: [0; NUM_DEV_TYPES],
            roads_left: ROADS_PER_PLAYER,
            settlements_left: SETTLEMENTS_PER_PLAYER,
            cities_left: CITIES_PER_PLAYER,
            road_length: 0,
            ports: 0,
        }
    }
}

impl PlayerState {
    #[inline]
    pub fn knights_played(&self) -> u8 {
        self.dev_played[DevCard::Knight.index()]
    }

    #[inline]
    pub fn settlements_on_board(&self) -> u8 {
        SETTLEMENTS_PER_PLAYER - self.settlements_left
    }

    #[inline]
    pub fn cities_on_board(&self) -> u8 {
        CITIES_PER_PLAYER - self.cities_left
    }

    #[inline]
    pub fn roads_on_board(&self) -> u8 {
        ROADS_PER_PLAYER - self.roads_left
    }

    /// Unplayed development cards, including ones bought this turn.
    #[inline]
    pub fn dev_cards_held(&self) -> u32 {
        self.dev
            .iter()
            .chain(self.dev_new.iter())
            .map(|&c| c as u32)
            .sum()
    }

    #[inline]
    pub fn vp_cards(&self) -> u8 {
        let i = DevCard::VictoryPoint.index();
        self.dev[i] + self.dev_new[i]
    }

    /// Bank trade ratio for giving `r`.
    #[inline]
    pub fn trade_ratio(&self, r: Resource) -> u8 {
        if self.ports & (1 << r as u8) != 0 {
            2
        } else if self.ports & GENERIC_PORT_BIT != 0 {
            3
        } else {
            4
        }
    }
}

/// Where play continues after the robber or Road Building interrupts a turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resume {
    PreRoll,
    Main,
    /// The main phase, after the round in which the others may propose trades (`open_trading`):
    /// the robber after a 7 resumes here.
    AfterRoll,
}

impl Resume {
    /// The phase play resumes in; the round of proposals (`AfterRoll`) is opened by the caller.
    pub fn phase(self) -> Phase {
        match self {
            Resume::PreRoll => Phase::PreRoll,
            Resume::Main | Resume::AfterRoll => Phase::Main,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    SetupSettlement,
    SetupRoad {
        vertex: VertexId,
    },
    PreRoll,
    /// Players over the discard limit discard one card at a time, in seat order from the roller.
    Discard,
    MoveRobber {
        resume: Resume,
    },
    Main,
    RoadBuilding {
        remaining: u8,
        resume: Resume,
    },
    /// Other players answer the current offer one at a time, in seat order from the player on
    /// turn: accept or decline, or (`open_trading`) propose other terms. Without `open_trading`,
    /// those who can't pay are skipped; with it, those with no cards. With an empty offer this is
    /// the round after a roll in which each other player may propose a trade or pass.
    TradeResponse,
    /// The player on turn takes one accepted offer or proposal, or none.
    TradeConfirm,
    GameOver,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PendingTrade {
    /// What the player on turn offers to give; empty in a round of proposals.
    pub give: Hand,
    /// What the player on turn asks for; empty in a round of proposals.
    pub get: Hand,
    /// Seat offset (1..n) from the player on turn of the player now responding.
    pub responder_offset: u8,
    /// Bitmask of players who accepted.
    pub accepted: u8,
    /// Each player's proposal to the player on turn (`open_trading`), as (what that player gives,
    /// what it asks for); both empty when it made none.
    pub proposals: [(Hand, Hand); MAX_PLAYERS],
}

impl PendingTrade {
    /// An offer from the player on turn (a round of proposals when both are empty).
    pub fn offer(give: Hand, get: Hand) -> PendingTrade {
        PendingTrade {
            give,
            get,
            responder_offset: 0,
            accepted: 0,
            proposals: [(Hand::EMPTY, Hand::EMPTY); MAX_PLAYERS],
        }
    }

    /// Whether this is the round after a roll, with no offer to answer.
    pub fn is_round(&self) -> bool {
        self.give.is_empty() && self.get.is_empty()
    }

    /// A trade with `q` that the player on turn could take, as (what the player on turn gives,
    /// what it gets): the offer if `q` accepted it, `q`'s proposal if it made one.
    pub fn terms(&self, q: PlayerId) -> Option<(Hand, Hand)> {
        let (give, get) = self.proposals[q as usize];
        if self.accepted & (1 << q) != 0 {
            Some((self.give, self.get))
        } else if !give.is_empty() {
            Some((get, give))
        } else {
            None
        }
    }
}

/// Contents of a vertex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Building {
    pub owner: PlayerId,
    pub city: bool,
}

const CITY_FLAG: u8 = 0x80;

#[inline]
pub(crate) fn settlement_code(p: PlayerId) -> u8 {
    p + 1
}

#[inline]
pub(crate) fn city_code(p: PlayerId) -> u8 {
    (p + 1) | CITY_FLAG
}

/// Owner of an encoded vertex (non-zero code).
#[inline]
pub(crate) fn code_owner(code: u8) -> PlayerId {
    (code & !CITY_FLAG) - 1
}

#[inline]
pub(crate) fn code_is_city(code: u8) -> bool {
    code & CITY_FLAG != 0
}

/// `Clone` is written out below, so `clone_from` can reuse the buffers.
#[derive(Debug)]
pub struct GameState {
    pub rules: Rules,
    pub board: Arc<Board>,
    pub players: [PlayerState; MAX_PLAYERS],
    /// Per vertex: 0 if empty, otherwise an encoded building (see [`GameState::building`]).
    pub(crate) vertex: Vec<u8>,
    /// Per edge: 0 if empty, otherwise owner + 1.
    pub(crate) edge: Vec<u8>,
    // Bitset mirrors of `vertex` and `edge` for fast move generation.
    pub(crate) roads_of: [Bits; MAX_PLAYERS],
    /// Vertices touched by each player's roads.
    pub(crate) road_ends_of: [Bits; MAX_PLAYERS],
    pub(crate) buildings_of: [Bits; MAX_PLAYERS],
    /// Vertices holding any building.
    pub(crate) occupied: Bits,
    /// Vertices where the distance rule forbids building.
    pub(crate) blocked: Bits,
    pub(crate) road_edges: Bits,
    pub robber: HexId,
    pub bank: Hand,
    /// Remaining development cards; the next card drawn is the last.
    pub dev_deck: Vec<DevCard>,
    pub phase: Phase,
    pub current: PlayerId,
    /// 0 during initial placement, then 1 for the first player's first turn, +1 per turn.
    pub turn: u32,
    pub setup_step: u8,
    pub dev_played_this_turn: bool,
    pub offers_this_turn: u8,
    pub discard_pending: [u8; MAX_PLAYERS],
    pub trade: Option<PendingTrade>,
    pub longest_road: Option<PlayerId>,
    pub largest_army: Option<PlayerId>,
    pub winner: Option<PlayerId>,
    pub last_roll: Option<(u8, u8)>,
    /// Robber steals so far. Until the first one, every card movement is public.
    pub steals: u32,
    pub rng: Rng,
    pub dice: Dice,
}

// Every field is named in both methods, so a new field doesn't compile until it's copied.
impl Clone for GameState {
    fn clone(&self) -> Self {
        let GameState {
            rules,
            board,
            players,
            vertex,
            edge,
            roads_of,
            road_ends_of,
            buildings_of,
            occupied,
            blocked,
            road_edges,
            robber,
            bank,
            dev_deck,
            phase,
            current,
            turn,
            setup_step,
            dev_played_this_turn,
            offers_this_turn,
            discard_pending,
            trade,
            longest_road,
            largest_army,
            winner,
            last_roll,
            steals,
            rng,
            dice,
        } = self;
        GameState {
            rules: *rules,
            board: Arc::clone(board),
            players: *players,
            vertex: vertex.clone(),
            edge: edge.clone(),
            roads_of: *roads_of,
            road_ends_of: *road_ends_of,
            buildings_of: *buildings_of,
            occupied: *occupied,
            blocked: *blocked,
            road_edges: *road_edges,
            robber: *robber,
            bank: *bank,
            dev_deck: dev_deck.clone(),
            phase: *phase,
            current: *current,
            turn: *turn,
            setup_step: *setup_step,
            dev_played_this_turn: *dev_played_this_turn,
            offers_this_turn: *offers_this_turn,
            discard_pending: *discard_pending,
            trade: *trade,
            longest_road: *longest_road,
            largest_army: *largest_army,
            winner: *winner,
            last_roll: *last_roll,
            steals: *steals,
            rng: rng.clone(),
            dice: dice.clone(),
        }
    }

    /// Copies `source` into `self` reusing `self`'s buffers: weighing moves copies a state per
    /// move, and allocating each copy took about a twentieth of a search's time.
    fn clone_from(&mut self, source: &Self) {
        let GameState {
            rules,
            board,
            players,
            vertex,
            edge,
            roads_of,
            road_ends_of,
            buildings_of,
            occupied,
            blocked,
            road_edges,
            robber,
            bank,
            dev_deck,
            phase,
            current,
            turn,
            setup_step,
            dev_played_this_turn,
            offers_this_turn,
            discard_pending,
            trade,
            longest_road,
            largest_army,
            winner,
            last_roll,
            steals,
            rng,
            dice,
        } = source;
        self.rules = *rules;
        if !Arc::ptr_eq(&self.board, board) {
            self.board = Arc::clone(board);
        }
        self.players = *players;
        self.vertex.clone_from(vertex);
        self.edge.clone_from(edge);
        self.roads_of = *roads_of;
        self.road_ends_of = *road_ends_of;
        self.buildings_of = *buildings_of;
        self.occupied = *occupied;
        self.blocked = *blocked;
        self.road_edges = *road_edges;
        self.robber = *robber;
        self.bank = *bank;
        self.dev_deck.clone_from(dev_deck);
        self.phase = *phase;
        self.current = *current;
        self.turn = *turn;
        self.setup_step = *setup_step;
        self.dev_played_this_turn = *dev_played_this_turn;
        self.offers_this_turn = *offers_this_turn;
        self.discard_pending = *discard_pending;
        self.trade = *trade;
        self.longest_road = *longest_road;
        self.largest_army = *largest_army;
        self.winner = *winner;
        self.last_roll = *last_roll;
        self.steals = *steals;
        self.rng.clone_from(rng);
        self.dice.clone_from(dice);
    }
}

impl GameState {
    /// Starts a game on a board generated from the config, everything seeded by `seed`.
    pub fn new(config: &GameConfig, seed: u64) -> GameState {
        let mut rng = Rng::seed(seed);
        let board = Arc::new(Board::generate(&config.board, &mut rng));
        GameState::with_board(config.rules, board, rng)
    }

    /// Starts a game on a given board. The RNG shuffles the deck; after that it is never advanced,
    /// and dice and steals derive their own streams from it, keyed by turn (see `Rng::derive`).
    pub fn with_board(rules: Rules, board: Arc<Board>, mut rng: Rng) -> GameState {
        assert!(
            (2..=MAX_PLAYERS as u8).contains(&rules.num_players),
            "unsupported player count"
        );
        let mut dev_deck = Vec::with_capacity(25);
        for card in DevCard::ALL {
            dev_deck.extend(std::iter::repeat_n(
                card,
                BASE_DEV_DECK[card.index()] as usize,
            ));
        }
        rng.shuffle(&mut dev_deck);
        let nv = board.topology.num_vertices();
        let ne = board.topology.num_edges();
        GameState {
            rules,
            robber: board.robber_start,
            board,
            players: [PlayerState::default(); MAX_PLAYERS],
            vertex: vec![0; nv],
            edge: vec![0; ne],
            roads_of: [Bits::default(); MAX_PLAYERS],
            road_ends_of: [Bits::default(); MAX_PLAYERS],
            buildings_of: [Bits::default(); MAX_PLAYERS],
            occupied: Bits::default(),
            blocked: Bits::default(),
            road_edges: Bits::default(),
            bank: Hand([BANK_PER_RESOURCE; NUM_RESOURCES]),
            dev_deck,
            phase: Phase::SetupSettlement,
            current: 0,
            turn: 0,
            setup_step: 0,
            dev_played_this_turn: false,
            offers_this_turn: 0,
            discard_pending: [0; MAX_PLAYERS],
            trade: None,
            longest_road: None,
            largest_army: None,
            winner: None,
            last_roll: None,
            steals: 0,
            rng,
            dice: Dice::default(),
        }
    }

    #[inline]
    pub fn num_players(&self) -> usize {
        self.rules.num_players as usize
    }

    #[inline]
    pub fn is_over(&self) -> bool {
        self.phase == Phase::GameOver
    }

    #[inline]
    pub fn player(&self, p: PlayerId) -> &PlayerState {
        &self.players[p as usize]
    }

    /// The player who must make the next decision.
    pub fn acting_player(&self) -> PlayerId {
        let n = self.rules.num_players;
        match self.phase {
            Phase::Discard => (0..n)
                .map(|i| (self.current + i) % n)
                .find(|&p| self.discard_pending[p as usize] > 0)
                .expect("discard phase with nobody discarding"),
            Phase::TradeResponse => {
                let t = self.trade.expect("trade response without a trade");
                (self.current + t.responder_offset) % n
            }
            _ => self.current,
        }
    }

    #[inline]
    pub fn building(&self, v: VertexId) -> Option<Building> {
        match self.vertex[v as usize] {
            0 => None,
            code => Some(Building {
                owner: code_owner(code),
                city: code_is_city(code),
            }),
        }
    }

    #[inline]
    pub fn road_owner(&self, e: EdgeId) -> Option<PlayerId> {
        match self.edge[e as usize] {
            0 => None,
            code => Some(code - 1),
        }
    }

    /// Points everyone can see: buildings, Longest Road and Largest Army.
    pub fn visible_vp(&self, p: PlayerId) -> u8 {
        let ps = &self.players[p as usize];
        let mut vp = ps.settlements_on_board() + 2 * ps.cities_on_board();
        if self.longest_road == Some(p) {
            vp += 2;
        }
        if self.largest_army == Some(p) {
            vp += 2;
        }
        vp
    }

    /// Visible points plus hidden victory point cards.
    pub fn total_vp(&self, p: PlayerId) -> u8 {
        self.visible_vp(p) + self.players[p as usize].vp_cards()
    }

    pub(crate) fn place_settlement(&mut self, p: PlayerId, v: VertexId) {
        self.vertex[v as usize] = settlement_code(p);
        self.players[p as usize].settlements_left -= 1;
        self.buildings_of[p as usize].set(v);
        self.occupied.set(v);
        self.blocked |= self.board.topology.vertex_block_bits[v as usize];
    }

    pub(crate) fn place_road(&mut self, p: PlayerId, e: EdgeId) {
        self.edge[e as usize] = p + 1;
        self.players[p as usize].roads_left -= 1;
        self.roads_of[p as usize].set(e);
        let [a, b] = self.board.topology.edge_vertices[e as usize];
        self.road_ends_of[p as usize].set(a);
        self.road_ends_of[p as usize].set(b);
        self.road_edges.set(e);
        self.players[p as usize].road_length = self.road_length_after_adding(p, e);
    }

    /// Player placing during initial placement step `step` (snake order).
    pub(crate) fn setup_player(&self, step: u8) -> PlayerId {
        let n = self.rules.num_players;
        if step < n { step } else { 2 * n - 1 - step }
    }

    /// Deterministic hash of the game position (excluding RNG state and deck order), used to
    /// check that replays reproduce a game exactly.
    pub fn position_hash(&self) -> u64 {
        let mut h = Fnv64::default();
        self.board.to_layout().hash(&mut h);
        self.players[..self.num_players()].hash(&mut h);
        self.vertex.hash(&mut h);
        self.edge.hash(&mut h);
        self.robber.hash(&mut h);
        self.bank.hash(&mut h);
        self.dev_deck.len().hash(&mut h);
        self.phase.hash(&mut h);
        self.current.hash(&mut h);
        self.turn.hash(&mut h);
        self.setup_step.hash(&mut h);
        self.dev_played_this_turn.hash(&mut h);
        self.offers_this_turn.hash(&mut h);
        self.discard_pending.hash(&mut h);
        self.trade.hash(&mut h);
        self.longest_road.hash(&mut h);
        self.largest_army.hash(&mut h);
        self.winner.hash(&mut h);
        self.last_roll.hash(&mut h);
        h.finish()
    }
}

/// FNV-1a: stable across runs and platforms, unlike the std default hasher.
struct Fnv64(u64);

impl Default for Fnv64 {
    fn default() -> Self {
        Fnv64(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for Fnv64 {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x100_0000_01b3);
        }
    }

    fn write_usize(&mut self, i: usize) {
        self.write(&(i as u64).to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_hashes_use_64_bit_fnv_1a() {
        // Published FNV-1a test vectors.
        let vectors: [(&[u8], u64); 3] = [
            (b"", 0xcbf2_9ce4_8422_2325),
            (b"a", 0xaf63_dc4c_8601_ec8c),
            (b"foobar", 0x8594_4171_f739_67e8),
        ];
        for (input, want) in vectors {
            let mut h = Fnv64::default();
            h.write(input);
            assert_eq!(h.finish(), want, "{input:?}");
        }
        // A usize hashes as its 8 little-endian bytes on every platform.
        let (mut a, mut b) = (Fnv64::default(), Fnv64::default());
        a.write_usize(0x0102);
        b.write(&0x0102u64.to_le_bytes());
        assert_eq!(a.finish(), b.finish());
        assert_ne!(a.finish(), Fnv64::default().finish());
    }

    /// A game `steps` random legal moves in, on the board of `seed`.
    fn played(seed: u64, steps: usize) -> GameState {
        let mut s = GameState::new(&crate::GameConfig::official(4), seed);
        let (mut rng, mut legal) = (Rng::seed(seed), Vec::new());
        for _ in 0..steps {
            if s.is_over() {
                break;
            }
            s.legal_actions(&mut legal);
            let a = legal[rng.below(legal.len() as u32) as usize];
            s.apply_legal(a);
        }
        s
    }

    #[test]
    fn clone_from_copies_every_field() {
        // Random moves spend many steps on trade proposals, hence the long game.
        let late = played(1, 3000);
        let early = played(2, 12);
        assert!(late.dev_deck.len() < early.dev_deck.len() && late.turn > early.turn);
        let shown = |s: &GameState| format!("{s:?}");
        assert_eq!(shown(&late.clone()), shown(&late));
        // Into a state on another board, with longer and shorter buffers, both ways.
        for (from, into) in [(&late, &early), (&early, &late)] {
            let mut c = into.clone();
            c.clone_from(from);
            assert_eq!(shown(&c), shown(from));
            assert!(Arc::ptr_eq(&c.board, &from.board));
            // Copies, not views: changing the copy leaves the source as it was.
            let before = shown(from);
            let mut legal = Vec::new();
            c.legal_actions(&mut legal);
            if let Some(&a) = legal.first() {
                c.apply_legal(a);
            }
            assert_eq!(shown(from), before);
        }
    }
}

/// Kani proof (docs/TRACEABILITY.md, "Kani").
#[cfg(kani)]
mod proofs {
    use super::*;
    use crate::types::Port;

    /// RULES.md: "4:1, 3:1 with a generic harbour, or 2:1 for that resource with a matching
    /// harbour. You always get the best rate you have." For every set of harbours held and every
    /// resource given.
    #[kani::proof]
    fn bank_trades_get_the_best_rate_held() {
        let held: [bool; 6] = kani::any();
        let ports = [
            Port::Special(Resource::Wood),
            Port::Special(Resource::Brick),
            Port::Special(Resource::Sheep),
            Port::Special(Resource::Wheat),
            Port::Special(Resource::Ore),
            Port::Generic,
        ];
        let mut p = PlayerState::default();
        for (port, h) in ports.iter().zip(held) {
            if h {
                p.ports |= port.bit();
            }
        }
        let i: usize = kani::any();
        kani::assume(i < NUM_RESOURCES);
        let r = Resource::from_index(i);
        let mut best = 4;
        for (port, h) in ports.iter().zip(held) {
            let rate = match port {
                Port::Generic => 3,
                Port::Special(s) if *s == r => 2,
                Port::Special(_) => 4,
            };
            if h && rate < best {
                best = rate;
            }
        }
        assert_eq!(p.trade_ratio(r), best);
    }
}
