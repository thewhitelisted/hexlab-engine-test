//! Basic game vocabulary: ids, resources, hands, development cards and ports.

use serde::{Deserialize, Serialize};

pub type PlayerId = u8;
pub type HexId = u8;
pub type VertexId = u16;
pub type EdgeId = u16;

pub const MAX_PLAYERS: usize = 6;
pub const NUM_RESOURCES: usize = 5;
pub const NUM_DEV_TYPES: usize = 5;

/// Cards of each resource in the bank at the start of a base game.
pub const BANK_PER_RESOURCE: u8 = 19;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum Resource {
    Wood = 0,
    Brick = 1,
    Sheep = 2,
    Wheat = 3,
    Ore = 4,
}

impl Resource {
    pub const ALL: [Resource; NUM_RESOURCES] = [
        Resource::Wood,
        Resource::Brick,
        Resource::Sheep,
        Resource::Wheat,
        Resource::Ore,
    ];

    #[inline]
    pub const fn index(self) -> usize {
        self as usize
    }

    #[inline]
    pub const fn from_index(i: usize) -> Resource {
        Self::ALL[i]
    }
}

/// A multiset of resource cards, indexed by [`Resource`].
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct Hand(pub [u8; NUM_RESOURCES]);

impl Hand {
    pub const EMPTY: Hand = Hand([0; NUM_RESOURCES]);

    pub const fn new(wood: u8, brick: u8, sheep: u8, wheat: u8, ore: u8) -> Hand {
        Hand([wood, brick, sheep, wheat, ore])
    }

    pub const fn of(r: Resource, n: u8) -> Hand {
        let mut h = [0; NUM_RESOURCES];
        h[r as usize] = n;
        Hand(h)
    }

    #[inline]
    pub fn get(&self, r: Resource) -> u8 {
        self.0[r.index()]
    }

    #[inline]
    pub fn total(&self) -> u32 {
        self.0.iter().map(|&c| c as u32).sum()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0 == [0; NUM_RESOURCES]
    }

    #[inline]
    pub fn contains(&self, other: &Hand) -> bool {
        self.0.iter().zip(other.0.iter()).all(|(a, b)| a >= b)
    }

    #[inline]
    pub fn add(&mut self, other: &Hand) {
        for i in 0..NUM_RESOURCES {
            self.0[i] += other.0[i];
        }
    }

    #[inline]
    /// Takes `other` out of the hand. The hand must hold it: a debug build stops on a hand that
    /// doesn't, and a release build leaves no cards of that kind rather than wrapping the count
    /// round to about 255 (as it did when a search played a trade out in a sampled world whose
    /// partner lacked the cards).
    pub fn sub(&mut self, other: &Hand) {
        for i in 0..NUM_RESOURCES {
            debug_assert!(self.0[i] >= other.0[i], "hand underflow");
            self.0[i] = self.0[i].saturating_sub(other.0[i]);
        }
    }

    #[inline]
    pub fn add_n(&mut self, r: Resource, n: u8) {
        self.0[r.index()] += n;
    }

    #[inline]
    pub fn sub_n(&mut self, r: Resource, n: u8) {
        debug_assert!(self.0[r.index()] >= n, "hand underflow");
        self.0[r.index()] -= n;
    }

    /// True when no resource type appears in both hands (a trade can't swap like for like).
    #[inline]
    pub fn disjoint_types(&self, other: &Hand) -> bool {
        self.0
            .iter()
            .zip(other.0.iter())
            .all(|(&a, &b)| a == 0 || b == 0)
    }
}

/// Building and purchase costs.
pub mod cost {
    use super::Hand;
    pub const ROAD: Hand = Hand::new(1, 1, 0, 0, 0);
    pub const SETTLEMENT: Hand = Hand::new(1, 1, 1, 1, 0);
    pub const CITY: Hand = Hand::new(0, 0, 0, 2, 3);
    pub const DEV_CARD: Hand = Hand::new(0, 0, 1, 1, 1);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum DevCard {
    Knight = 0,
    VictoryPoint = 1,
    RoadBuilding = 2,
    YearOfPlenty = 3,
    Monopoly = 4,
}

impl DevCard {
    pub const ALL: [DevCard; NUM_DEV_TYPES] = [
        DevCard::Knight,
        DevCard::VictoryPoint,
        DevCard::RoadBuilding,
        DevCard::YearOfPlenty,
        DevCard::Monopoly,
    ];

    #[inline]
    pub const fn index(self) -> usize {
        self as usize
    }

    #[inline]
    pub const fn from_index(i: usize) -> DevCard {
        Self::ALL[i]
    }
}

/// Development card counts, indexed by [`DevCard`].
pub type DevCounts = [u8; NUM_DEV_TYPES];

/// The base-game development deck: 14 knights, 5 victory points, 2 each of the progress cards.
pub const BASE_DEV_DECK: DevCounts = [14, 5, 2, 2, 2];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Port {
    /// 3:1 any resource.
    Generic,
    /// 2:1 for one resource.
    Special(Resource),
}

impl Port {
    /// Bit in [`crate::state::PlayerState::ports`] that this port sets.
    #[inline]
    pub const fn bit(self) -> u8 {
        match self {
            Port::Generic => GENERIC_PORT_BIT,
            Port::Special(r) => 1 << (r as u8),
        }
    }
}

pub const GENERIC_PORT_BIT: u8 = 1 << 5;

/// Kani proofs, for every input (docs/TRACEABILITY.md, "Kani"): `cargo kani -p engine-core`.
#[cfg(kani)]
mod proofs {
    use super::*;

    /// A hand the bank could have dealt: at most 19 of each resource.
    fn bank_hand() -> Hand {
        let h: [u8; NUM_RESOURCES] = kani::any();
        for c in h {
            kani::assume(c <= 19);
        }
        Hand(h)
    }

    #[kani::proof]
    fn adding_and_taking_back_a_hand_restores_it() {
        let (a, b) = (bank_hand(), bank_hand());
        let mut c = a;
        c.add(&b); // no overflow: debug builds would panic
        assert!(c.contains(&a) && c.contains(&b));
        assert_eq!(c.total(), a.total() + b.total());
        c.sub(&b);
        assert_eq!(c, a);
    }

    #[kani::proof]
    fn contains_means_every_count_is_covered() {
        let (a, b) = (Hand(kani::any()), Hand(kani::any()));
        let covered = (0..NUM_RESOURCES).all(|i| a.0[i] >= b.0[i]);
        assert_eq!(a.contains(&b), covered);
        if covered {
            let mut c = a;
            c.sub(&b); // no underflow
            c.add(&b);
            assert_eq!(c, a);
        }
    }

    #[kani::proof]
    fn disjoint_types_means_no_resource_on_both_sides() {
        let (a, b) = (Hand(kani::any()), Hand(kani::any()));
        let shared = (0..NUM_RESOURCES).any(|i| a.0[i] > 0 && b.0[i] > 0);
        assert_eq!(a.disjoint_types(&b), !shared);
    }

    #[kani::proof]
    fn single_resource_helpers_agree_with_the_counts() {
        let h = Hand(kani::any());
        let i: usize = kani::any();
        kani::assume(i < NUM_RESOURCES);
        let r = Resource::from_index(i);
        assert_eq!(r.index(), i);
        assert_eq!(h.get(r), h.0[i]);
        assert_eq!(h.is_empty(), h.total() == 0);
        let n: u8 = kani::any();
        assert_eq!(Hand::of(r, n).get(r), n);
        assert_eq!(Hand::of(r, n).total(), n as u32);
        let mut g = h;
        kani::assume(h.0[i] as u32 + n as u32 <= 255);
        g.add_n(r, n);
        g.sub_n(r, n);
        assert_eq!(g, h);
    }

    #[kani::proof]
    fn development_card_indices_round_trip() {
        let i: usize = kani::any();
        kani::assume(i < DevCard::ALL.len());
        assert_eq!(DevCard::from_index(i).index(), i);
    }
}
