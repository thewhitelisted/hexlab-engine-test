//! Legal move generation and the placement predicates shared with rule checking.

use crate::actions::Action;
use crate::bits::Bits;
use crate::rules::DevTiming;
use crate::state::{GameState, Phase, settlement_code};
use crate::types::*;

/// Which player-to-player offers [`GameState::legal_actions_with`] enumerates. Rule checking
/// accepts any valid offer; this only bounds the action space for bots and search.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OfferSpace {
    None,
    /// Give 1 of a resource for 1 of another.
    OneForOne,
    /// 1-for-1 and 2-for-1 offers.
    #[default]
    Simple,
}

/// The trades `offers` lists for a player holding `hand`, as `make(give, get)`: one card of a
/// resource held for one of another, and with `Simple` two of one for one of another.
fn push_trades(
    hand: &Hand,
    offers: OfferSpace,
    out: &mut Vec<Action>,
    make: impl Fn(Hand, Hand) -> Action,
) {
    if offers == OfferSpace::None {
        return;
    }
    for give in Resource::ALL {
        let have = hand.get(give);
        for get in Resource::ALL {
            if get == give {
                continue;
            }
            if have >= 1 {
                out.push(make(Hand::of(give, 1), Hand::of(get, 1)));
            }
            if have >= 2 && offers == OfferSpace::Simple {
                out.push(make(Hand::of(give, 2), Hand::of(get, 1)));
            }
        }
    }
}

/// Bitmask over hexes (boards have at most 128 land hexes).
pub type HexMask = u128;

impl GameState {
    /// All legal actions for the acting player, with the default offer space.
    pub fn legal_actions(&self, out: &mut Vec<Action>) {
        self.legal_actions_with(OfferSpace::default(), out);
    }

    pub fn legal_actions_with(&self, offers: OfferSpace, out: &mut Vec<Action>) {
        out.clear();
        let p = self.current;
        let topo = &self.board.topology;
        match self.phase {
            Phase::GameOver => {}
            Phase::SetupSettlement => {
                for v in (topo.all_vertices & !self.blocked).ones() {
                    out.push(Action::BuildSettlement { vertex: v });
                }
            }
            Phase::SetupRoad { vertex } => {
                for &e in topo.vertex_edges[vertex as usize].as_slice() {
                    if self.edge[e as usize] == 0 {
                        out.push(Action::BuildRoad { edge: e });
                    }
                }
            }
            Phase::PreRoll => {
                out.push(Action::RollDice);
                self.push_dev_plays(out);
            }
            Phase::Discard => {
                let q = self.acting_player();
                let hand = self.players[q as usize].hand;
                for r in Resource::ALL {
                    if hand.get(r) > 0 {
                        out.push(Action::Discard { resource: r });
                    }
                }
            }
            Phase::MoveRobber { .. } => {
                let allowed = self.robber_hexes();
                let victims_by_hex = self.victims_by_hex();
                for h in 0..topo.num_hexes() as HexId {
                    if allowed & (1 << h) == 0 {
                        continue;
                    }
                    let victims = victims_by_hex[h as usize];
                    if victims == 0 {
                        out.push(Action::MoveRobber {
                            hex: h,
                            victim: None,
                        });
                    }
                    for q in 0..self.rules.num_players.get() {
                        if victims & (1 << q) != 0 {
                            out.push(Action::MoveRobber {
                                hex: h,
                                victim: Some(q),
                            });
                        }
                    }
                }
            }
            Phase::Main => {
                let ps = &self.players[p as usize];
                if ps.roads_left > 0 && ps.hand.contains(&cost::ROAD) {
                    self.push_road_spots(p, out);
                }
                if ps.settlements_left > 0 && ps.hand.contains(&cost::SETTLEMENT) {
                    for v in self.settlement_spots(p).ones() {
                        out.push(Action::BuildSettlement { vertex: v });
                    }
                }
                if ps.cities_left > 0 && ps.hand.contains(&cost::CITY) {
                    let code = settlement_code(p);
                    for v in self.buildings_of[p as usize].ones() {
                        if self.vertex[v as usize] == code {
                            out.push(Action::BuildCity { vertex: v });
                        }
                    }
                }
                if !self.dev_deck.is_empty() && ps.hand.contains(&cost::DEV_CARD) {
                    out.push(Action::BuyDevCard);
                }
                self.push_dev_plays(out);
                for give in Resource::ALL {
                    if ps.hand.get(give) >= ps.trade_ratio(give) {
                        for get in Resource::ALL {
                            if get != give && self.bank.get(get) > 0 {
                                out.push(Action::MaritimeTrade { give, get });
                            }
                        }
                    }
                }
                if self.rules.player_trading
                    && self.offers_this_turn < self.rules.max_offers_per_turn
                {
                    push_trades(&ps.hand, offers, out, |give, get| Action::OfferTrade {
                        give,
                        get,
                    });
                }
                out.push(Action::EndTurn);
            }
            Phase::RoadBuilding { .. } => self.push_road_spots(p, out),
            Phase::TradeResponse => {
                let t = self.trade.expect("trade response without a trade");
                let q = self.acting_player();
                let hand = &self.players[q as usize].hand;
                if !t.is_round() && hand.contains(&t.get) {
                    out.push(Action::AcceptTrade);
                }
                out.push(Action::RejectTrade);
                if self.rules.open_trading {
                    push_trades(hand, offers, out, |give, get| Action::ProposeTrade {
                        give,
                        get,
                    });
                }
            }
            Phase::TradeConfirm => {
                let t = self.trade.expect("trade confirm without a trade");
                let hand = &self.players[p as usize].hand;
                for q in 0..self.rules.num_players.get() {
                    if q != p && t.terms(q).is_some_and(|(give, _)| hand.contains(&give)) {
                        out.push(Action::ConfirmTrade { with: q });
                    }
                }
                out.push(Action::CancelTrade);
            }
        }
    }

    fn push_road_spots(&self, p: PlayerId, out: &mut Vec<Action>) {
        for e in self.road_spots(p).ones() {
            out.push(Action::BuildRoad { edge: e });
        }
    }

    /// Vertices touched by `p`'s roads.
    #[inline]
    pub fn road_ends(&self, p: PlayerId) -> Bits {
        self.road_ends_of[p as usize]
    }

    /// Edges where `p` may place a road (ignoring cost and pieces left). Same rule as
    /// [`GameState::can_road`], computed from bitsets.
    pub fn road_spots(&self, p: PlayerId) -> Bits {
        let topo = &self.board.topology;
        let own = self.buildings_of[p as usize];
        let opponents = self.occupied & !own;
        let connect = own | (self.road_ends(p) & !opponents);
        let mut spots = Bits::default();
        for v in connect.ones() {
            spots |= topo.vertex_edge_bits[v as usize];
        }
        spots & !self.road_edges
    }

    /// Vertices where `p` may build a settlement after initial placement (ignoring cost). Same
    /// rule as [`GameState::can_settle`], computed from bitsets.
    pub fn settlement_spots(&self, p: PlayerId) -> Bits {
        self.road_ends(p) & !self.blocked
    }

    fn push_dev_plays(&self, out: &mut Vec<Action>) {
        if self.dev_played_this_turn {
            return;
        }
        let ps = &self.players[self.current as usize];
        let held = |c: DevCard| ps.dev[c.index()] > 0;
        if held(DevCard::Knight) {
            out.push(Action::PlayKnight);
        }
        if self.phase == Phase::PreRoll && self.rules.dev_timing == DevTiming::KnightBeforeRoll {
            return;
        }
        if held(DevCard::RoadBuilding) && ps.roads_left > 0 && self.any_road_spot(self.current) {
            out.push(Action::PlayRoadBuilding);
        }
        if held(DevCard::YearOfPlenty) {
            for (i, a) in Resource::ALL.into_iter().enumerate() {
                for b in Resource::ALL.into_iter().skip(i) {
                    if self.bank_can_pay_pair(a, b) {
                        out.push(Action::PlayYearOfPlenty { a, b });
                    }
                }
            }
        }
        if held(DevCard::Monopoly) {
            for r in Resource::ALL {
                out.push(Action::PlayMonopoly { resource: r });
            }
        }
    }

    pub(crate) fn bank_can_pay_pair(&self, a: Resource, b: Resource) -> bool {
        if a == b {
            self.bank.get(a) >= 2
        } else {
            self.bank.get(a) >= 1 && self.bank.get(b) >= 1
        }
    }

    /// Empty vertex with no building on any neighbouring vertex (the distance rule).
    #[inline]
    pub fn vertex_open(&self, v: VertexId) -> bool {
        self.vertex[v as usize] == 0
            && self.board.topology.vertex_neighbors[v as usize]
                .as_slice()
                .iter()
                .all(|&n| self.vertex[n as usize] == 0)
    }

    /// Whether `p` may place a road on `e` (ignoring cost and pieces left): the edge is empty and
    /// connects to `p`'s building, or to `p`'s road through a vertex no opponent occupies.
    #[inline]
    pub fn can_road(&self, p: PlayerId, e: EdgeId) -> bool {
        if self.edge[e as usize] != 0 {
            return false;
        }
        let topo = &self.board.topology;
        let road = p + 1;
        topo.edge_vertices[e as usize].iter().any(|&v| {
            match self.building(v) {
                Some(b) if b.owner == p => return true,
                Some(_) => return false,
                None => {}
            }
            topo.vertex_edges[v as usize]
                .as_slice()
                .iter()
                .any(|&o| o != e && self.edge[o as usize] == road)
        })
    }

    /// Whether `p` may build a settlement on `v` after initial placement (ignoring cost).
    #[inline]
    pub fn can_settle(&self, p: PlayerId, v: VertexId) -> bool {
        self.vertex_open(v)
            && self.board.topology.vertex_edges[v as usize]
                .as_slice()
                .iter()
                .any(|&e| self.edge[e as usize] == p + 1)
    }

    pub(crate) fn any_road_spot(&self, p: PlayerId) -> bool {
        !self.road_spots(p).is_empty()
    }

    /// Hexes the robber may move to.
    pub fn robber_hexes(&self) -> HexMask {
        let nh = self.board.topology.num_hexes();
        let all: HexMask = if nh == 128 { !0 } else { (1 << nh) - 1 };
        let any = all & !(1 << self.robber);
        let Some(max_vp) = self.rules.friendly_robber_max_vp else {
            return any;
        };
        let mut allowed = 0;
        for h in 0..nh as HexId {
            if h == self.robber {
                continue;
            }
            let protected = self.board.topology.hex_vertices[h as usize]
                .iter()
                .filter_map(|&v| self.building(v))
                .any(|b| b.owner != self.current && self.visible_vp(b.owner) <= max_vp);
            if !protected {
                allowed |= 1 << h;
            }
        }
        // If every hex is protected the robber may go anywhere else.
        if allowed == 0 { any } else { allowed }
    }

    /// [`GameState::robber_victims`] for every hex at once.
    fn victims_by_hex(&self) -> [u8; 128] {
        let mut out = [0u8; 128];
        let topo = &self.board.topology;
        for q in 0..self.rules.num_players.get() {
            if q == self.current || self.players[q as usize].hand.total() == 0 {
                continue;
            }
            for v in self.buildings_of[q as usize].ones() {
                for &h in topo.vertex_hexes[v as usize].as_slice() {
                    out[h as usize] |= 1 << q;
                }
            }
        }
        out
    }

    /// Bitmask of opponents with a building on `h` and at least one card.
    pub fn robber_victims(&self, h: HexId) -> u8 {
        let mut mask = 0u8;
        for &v in &self.board.topology.hex_vertices[h as usize] {
            if let Some(b) = self.building(v)
                && b.owner != self.current
                && self.players[b.owner as usize].hand.total() > 0
            {
                mask |= 1 << b.owner;
            }
        }
        mask
    }
}
