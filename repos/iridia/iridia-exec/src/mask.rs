//! Sets of lanes: which invocations of a run are active. A pixel run is a quad or a few invocations in lockstep, its
//! set an integer's bits; a thread group of up to 1024 invocations needs [`Wide`].

/// A set of the lanes of a run.
pub trait Mask: Copy + PartialEq + std::fmt::Debug {
    /// The empty set.
    fn none() -> Self;
    /// Lanes `0..n`.
    fn first(n: usize) -> Self;
    fn has(self, lane: usize) -> bool;
    fn insert(&mut self, lane: usize);
    fn remove(&mut self, lane: usize);
    fn and(self, other: Self) -> Self;
    fn or(self, other: Self) -> Self;
    /// The lanes of `self` that are not in `other`.
    fn minus(self, other: Self) -> Self;
    fn is_empty(self) -> bool;
}

/// A set of up to as many lanes as the integer has bits.
macro_rules! bits {
    ($($t:ty),*) => {$(
        impl Mask for $t {
            fn none() -> Self {
                0
            }
            fn first(n: usize) -> Self {
                if n >= <$t>::BITS as usize { <$t>::MAX } else { (1 << n) - 1 }
            }
            fn has(self, lane: usize) -> bool {
                self >> lane & 1 != 0
            }
            fn insert(&mut self, lane: usize) {
                *self |= 1 << lane;
            }
            fn remove(&mut self, lane: usize) {
                *self &= !(1 << lane);
            }
            fn and(self, other: Self) -> Self {
                self & other
            }
            fn or(self, other: Self) -> Self {
                self | other
            }
            fn minus(self, other: Self) -> Self {
                self & !other
            }
            fn is_empty(self) -> bool {
                self == 0
            }
        }
    )*};
}
bits!(u8, u16, u32);

/// Up to 1024 lanes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wide([u64; 16]);

impl Mask for Wide {
    fn none() -> Self {
        Wide([0; 16])
    }
    fn first(n: usize) -> Self {
        let mut w = [0u64; 16];
        for (i, word) in w.iter_mut().enumerate() {
            let lo = i * 64;
            if n >= lo + 64 {
                *word = u64::MAX;
            } else if n > lo {
                *word = (1u64 << (n - lo)) - 1;
            }
        }
        Wide(w)
    }
    fn has(self, lane: usize) -> bool {
        self.0[lane / 64] >> (lane % 64) & 1 != 0
    }
    fn insert(&mut self, lane: usize) {
        self.0[lane / 64] |= 1 << (lane % 64);
    }
    fn remove(&mut self, lane: usize) {
        self.0[lane / 64] &= !(1 << (lane % 64));
    }
    fn and(self, other: Self) -> Self {
        Wide(std::array::from_fn(|i| self.0[i] & other.0[i]))
    }
    fn or(self, other: Self) -> Self {
        Wide(std::array::from_fn(|i| self.0[i] | other.0[i]))
    }
    fn minus(self, other: Self) -> Self {
        Wide(std::array::from_fn(|i| self.0[i] & !other.0[i]))
    }
    fn is_empty(self) -> bool {
        self.0.iter().all(|w| *w == 0)
    }
}
