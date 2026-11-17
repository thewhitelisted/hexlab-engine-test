//! Deterministic PRNG (xoshiro256** seeded by splitmix64).
//!
//! Implemented here rather than taken from a crate so that game replays stay bit-identical
//! across dependency upgrades and platforms.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    pub fn seed(seed: u64) -> Rng {
        let mut sm = seed;
        let mut next = || {
            sm = sm.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = sm;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        Rng {
            s: [next(), next(), next(), next()],
        }
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Uniform integer in `0..n` (Lemire's method, unbiased). `n` must be non-zero.
    #[inline]
    pub fn below(&mut self, n: u32) -> u32 {
        debug_assert!(n > 0);
        let mut m = self.next_u32() as u64 * n as u64;
        let mut low = m as u32;
        if low < n {
            let threshold = n.wrapping_neg() % n;
            while low < threshold {
                m = self.next_u32() as u64 * n as u64;
                low = m as u32;
            }
        }
        (m >> 32) as u32
    }

    /// Uniform float in `[0, 1)`.
    #[inline]
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    #[inline]
    pub fn die(&mut self) -> u8 {
        self.below(6) as u8 + 1
    }

    /// An independent generator for one chance event, from this generator's current state, a
    /// purpose and a key (e.g. the turn), without advancing this one. Games draw their dice and
    /// steals this way, so a given turn's dice don't depend on how many random draws came before:
    /// two games that diverge still roll the same dice on the same turn (common random numbers).
    pub fn derive(&self, purpose: u64, key: u64) -> Rng {
        let mut h = purpose.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ key;
        for w in self.s {
            h = (h ^ w).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            h ^= h >> 31;
        }
        Rng::seed(h)
    }

    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i as u32 + 1) as usize;
            items.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_for_seed() {
        let mut a = Rng::seed(42);
        let mut b = Rng::seed(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn below_stays_in_range_and_covers_it() {
        let mut rng = Rng::seed(7);
        let mut seen = [0u32; 6];
        for _ in 0..60_000 {
            let x = rng.below(6);
            seen[x as usize] += 1;
        }
        for count in seen {
            assert!((9_000..11_000).contains(&count), "{seen:?}");
        }
    }

    #[test]
    fn large_ranges_reject_the_draws_that_would_bias_them() {
        // With n = 3 * 2^30, a plain multiply-and-shift maps two 32-bit draws onto every
        // multiple of 3 and one onto every other value, so half the results would be multiples
        // of 3. The rejection step drops one draw from each pair.
        let n = 3u32 << 30;
        let mut rng = Rng::seed(4);
        let mut multiples = 0;
        for _ in 0..30_000 {
            let x = rng.below(n);
            assert!(x < n);
            if x.is_multiple_of(3) {
                multiples += 1;
            }
        }
        // A third of 30,000, with a standard deviation of about 82.
        assert!((9_600..10_400).contains(&multiples), "{multiples}");
    }
}
