//! What one player is allowed to see.

use serde::{Deserialize, Serialize};

use crate::state::{Building, GameState, PendingTrade, Phase};
use crate::types::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicPlayer {
    pub hand_size: u32,
    /// Unplayed development cards, including ones bought this turn.
    pub dev_cards: u32,
    pub dev_played: DevCounts,
    pub roads_left: u8,
    pub settlements_left: u8,
    pub cities_left: u8,
    pub road_length: u8,
    pub visible_vp: u8,
    pub ports: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub observer: PlayerId,
    pub phase: Phase,
    pub current: PlayerId,
    pub acting: PlayerId,
    pub turn: u32,
    pub buildings: Vec<Option<Building>>,
    pub roads: Vec<Option<PlayerId>>,
    pub robber: HexId,
    pub bank: Hand,
    pub dev_deck_size: u32,
    pub players: Vec<PublicPlayer>,
    pub longest_road: Option<PlayerId>,
    pub largest_army: Option<PlayerId>,
    pub winner: Option<PlayerId>,
    pub last_roll: Option<(u8, u8)>,
    pub discard_pending: Vec<u8>,
    pub trade: Option<PendingTrade>,
    pub hand: Hand,
    pub dev: DevCounts,
    pub dev_new: DevCounts,
}

impl GameState {
    pub fn observe(&self, observer: PlayerId) -> Observation {
        let n = self.num_players();
        let topo = &self.board.topology;
        let me = &self.players[observer as usize];
        Observation {
            observer,
            phase: self.phase,
            current: self.current,
            acting: if self.is_over() {
                self.current
            } else {
                self.acting_player()
            },
            turn: self.turn,
            buildings: (0..topo.num_vertices() as VertexId)
                .map(|v| self.building(v))
                .collect(),
            roads: (0..topo.num_edges() as EdgeId)
                .map(|e| self.road_owner(e))
                .collect(),
            robber: self.robber,
            bank: self.bank,
            dev_deck_size: self.dev_deck.len() as u32,
            players: (0..n as PlayerId)
                .map(|q| {
                    let p = &self.players[q as usize];
                    PublicPlayer {
                        hand_size: p.hand.total(),
                        dev_cards: p.dev_cards_held(),
                        dev_played: p.dev_played,
                        roads_left: p.roads_left,
                        settlements_left: p.settlements_left,
                        cities_left: p.cities_left,
                        road_length: p.road_length,
                        visible_vp: self.visible_vp(q),
                        ports: p.ports,
                    }
                })
                .collect(),
            longest_road: self.longest_road,
            largest_army: self.largest_army,
            winner: self.winner,
            last_roll: self.last_roll,
            discard_pending: self.discard_pending[..n].to_vec(),
            trade: self.trade,
            hand: me.hand,
            dev: me.dev,
            dev_new: me.dev_new,
        }
    }
}

impl Observation {
    /// Development cards whose identity the observer can't see: the deck plus opponents' unplayed
    /// cards. Each unseen card is equally likely to be any card in this pool.
    pub fn unseen_dev_pool(&self) -> DevCounts {
        let mut pool = BASE_DEV_DECK;
        for (i, count) in pool.iter_mut().enumerate() {
            *count -= self.dev[i] + self.dev_new[i];
            for p in &self.players {
                *count -= p.dev_played[i];
            }
        }
        pool
    }

    /// Probability that a given unseen development card is of each type.
    pub fn unseen_dev_probabilities(&self) -> [f64; NUM_DEV_TYPES] {
        let pool = self.unseen_dev_pool();
        let total: u32 = pool.iter().map(|&c| c as u32).sum();
        let mut out = [0.0; NUM_DEV_TYPES];
        if total > 0 {
            for i in 0..NUM_DEV_TYPES {
                out[i] = pool[i] as f64 / total as f64;
            }
        }
        out
    }
}
