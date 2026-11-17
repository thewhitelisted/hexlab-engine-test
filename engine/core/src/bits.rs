//! Fixed 256-bit sets over vertex or edge ids.

use std::ops::{BitAnd, BitOr, BitOrAssign, Not};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Bits([u64; 4]);

impl Bits {
    #[inline]
    pub fn has(&self, i: u16) -> bool {
        self.0[i as usize >> 6] & (1 << (i & 63)) != 0
    }

    #[inline]
    pub fn set(&mut self, i: u16) {
        self.0[i as usize >> 6] |= 1 << (i & 63);
    }

    #[inline]
    pub fn clear(&mut self, i: u16) {
        self.0[i as usize >> 6] &= !(1 << (i & 63));
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0 == [0; 4]
    }

    #[inline]
    pub fn count(&self) -> u32 {
        self.0.iter().map(|w| w.count_ones()).sum()
    }

    /// Set bits in increasing order.
    #[inline]
    pub fn ones(&self) -> Ones {
        Ones {
            words: self.0,
            word: 0,
        }
    }

    /// Bits `0..n` set.
    pub fn first(n: usize) -> Bits {
        let mut b = Bits::default();
        for i in 0..n {
            b.set(i as u16);
        }
        b
    }
}

pub struct Ones {
    words: [u64; 4],
    word: usize,
}

impl Iterator for Ones {
    type Item = u16;

    #[inline]
    fn next(&mut self) -> Option<u16> {
        while self.word < 4 {
            let w = self.words[self.word];
            if w != 0 {
                self.words[self.word] = w & (w - 1);
                return Some((self.word * 64) as u16 + w.trailing_zeros() as u16);
            }
            self.word += 1;
        }
        None
    }
}

impl BitOr for Bits {
    type Output = Bits;
    #[inline]
    fn bitor(self, o: Bits) -> Bits {
        Bits(std::array::from_fn(|i| self.0[i] | o.0[i]))
    }
}

impl BitOrAssign for Bits {
    #[inline]
    fn bitor_assign(&mut self, o: Bits) {
        for i in 0..4 {
            self.0[i] |= o.0[i];
        }
    }
}

impl BitAnd for Bits {
    type Output = Bits;
    #[inline]
    fn bitand(self, o: Bits) -> Bits {
        Bits(std::array::from_fn(|i| self.0[i] & o.0[i]))
    }
}

impl Not for Bits {
    type Output = Bits;
    #[inline]
    fn not(self) -> Bits {
        Bits(self.0.map(|w| !w))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_ops_and_iteration() {
        let mut a = Bits::default();
        for i in [0, 5, 63, 64, 130, 255] {
            a.set(i);
        }
        assert_eq!(a.ones().collect::<Vec<_>>(), vec![0, 5, 63, 64, 130, 255]);
        assert_eq!(a.count(), 6);
        let b = Bits::first(64);
        assert_eq!((a & b).ones().collect::<Vec<_>>(), vec![0, 5, 63]);
        assert_eq!((a & !b).count(), 3);
        assert!((a | b).has(10));
    }
}

/// Kani proofs, for every set and index (docs/TRACEABILITY.md, "Kani").
#[cfg(kani)]
mod proofs {
    use super::*;

    fn index() -> u16 {
        let i: u16 = kani::any();
        kani::assume(i < 256);
        i
    }

    #[kani::proof]
    fn setting_and_clearing_touch_only_that_bit() {
        let before = Bits(kani::any());
        let (i, j) = (index(), index());
        let mut b = before;
        b.set(i);
        assert!(b.has(i));
        assert_eq!(b.count(), before.count() + !before.has(i) as u32);
        if j != i {
            assert_eq!(b.has(j), before.has(j));
        }
        b.clear(i);
        assert!(!b.has(i));
        assert_eq!(b.count(), before.count() - before.has(i) as u32);
        if j != i {
            assert_eq!(b.has(j), before.has(j));
        }
    }

    #[kani::proof]
    fn set_operations_work_bit_by_bit() {
        let (a, b) = (Bits(kani::any()), Bits(kani::any()));
        let i = index();
        assert_eq!((a | b).has(i), a.has(i) || b.has(i));
        assert_eq!((a & b).has(i), a.has(i) && b.has(i));
        assert_eq!((!a).has(i), !a.has(i));
        let mut c = a;
        c |= b;
        assert_eq!(c, a | b);
        assert_eq!(a.is_empty(), a.count() == 0);
    }

    /// The iterator's step on one word, for every word: `w & (w - 1)` drops the lowest set bit,
    /// which `trailing_zeros` locates.
    #[kani::proof]
    fn each_step_takes_the_lowest_set_bit() {
        let w: u64 = kani::any();
        kani::assume(w != 0);
        let low = w.trailing_zeros();
        assert!(w & (1 << low) != 0 && w & ((1u64 << low) - 1) == 0);
        assert_eq!(w & (w - 1), w & !(1 << low));
    }
}
