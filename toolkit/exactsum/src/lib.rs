//! A sum of `f64`s that does not depend on the order of its terms: the exact sum kept as non-overlapping partials
//! (Shewchuk, "Adaptive Precision Floating-Point Arithmetic and Fast Robust Geometric Predicates", 1997), rounded once
//! to the nearest `f64` when read, ties to even (as Python's `math.fsum`). A decision made over the parts of a map
//! (a texture's weight summed over every placement) is then the same whatever order the parts arrive in, and the same
//! as over the whole map at once. Finite terms only.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

/// See the module documentation.
#[derive(Clone, Debug, Default)]
pub struct ExactSum {
    /// Non-overlapping, increasing in magnitude; their exact sum is the sum.
    partials: Vec<f64>,
}

impl ExactSum {
    /// Add `x` (finite) exactly.
    pub fn add(&mut self, mut x: f64) {
        let mut i = 0;
        for j in 0..self.partials.len() {
            let mut y = self.partials[j];
            if x.abs() < y.abs() {
                std::mem::swap(&mut x, &mut y);
            }
            let hi = x + y;
            let lo = y - (hi - x);
            if lo != 0.0 {
                self.partials[i] = lo;
                i += 1;
            }
            x = hi;
        }
        self.partials.truncate(i);
        self.partials.push(x);
    }

    /// Add `other`'s sum exactly.
    pub fn merge(&mut self, other: &ExactSum) {
        for &p in &other.partials {
            self.add(p);
        }
    }

    /// The bytes it holds.
    pub fn bytes(&self) -> usize {
        8 * self.partials.len()
    }

    /// The exact sum rounded to the nearest `f64`, ties to even.
    pub fn value(&self) -> f64 {
        let p = &self.partials;
        let Some(&top) = p.last() else {
            return 0.0;
        };
        let (mut hi, mut lo, mut n) = (top, 0.0, p.len() - 1);
        while n > 0 {
            let x = hi;
            let y = p[n - 1];
            n -= 1;
            hi = x + y;
            lo = y - (hi - x);
            if lo != 0.0 {
                break;
            }
        }
        // a half-way case rounded the wrong way when the partials below push past it
        if n > 0 && ((lo < 0.0 && p[n - 1] < 0.0) || (lo > 0.0 && p[n - 1] > 0.0)) {
            let y = lo * 2.0;
            let x = hi + y;
            if y == x - hi {
                hi = x;
            }
        }
        hi
    }
}

impl PartialEq for ExactSum {
    /// Equal when their rounded sums are (the partials of one sum differ with the order it was made in).
    fn eq(&self, other: &ExactSum) -> bool {
        self.value().to_bits() == other.value().to_bits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sum(xs: &[f64]) -> f64 {
        let mut s = ExactSum::default();
        for &x in xs {
            s.add(x);
        }
        s.value()
    }

    #[test]
    fn the_sum_is_exact_and_the_order_does_not_matter() {
        // a naive sum loses the 1 in either order
        assert_eq!(sum(&[1e16, 1.0, -1e16]), 1.0);
        assert_eq!(sum(&[1e100, 1.0, -1e100, 1e-100]), 1.0);
        assert_eq!(sum(&[0.1; 10]), 1.0);
        let xs = [0.1, 0.2, 0.3, 1e-17, 7.25, -3.125, 1e15, -1e15];
        let mut rev = xs;
        rev.reverse();
        assert_eq!(sum(&xs).to_bits(), sum(&rev).to_bits());
        // merging sums made in parts is the sum of all
        let (mut a, mut b) = (ExactSum::default(), ExactSum::default());
        for &x in &xs[..3] {
            a.add(x);
        }
        for &x in &xs[3..] {
            b.add(x);
        }
        let mut ab = a.clone();
        ab.merge(&b);
        let mut ba = b;
        ba.merge(&a);
        assert_eq!(ab, ba);
        assert_eq!(ab.value().to_bits(), sum(&xs).to_bits());
        assert_eq!(ExactSum::default().value(), 0.0);
    }

    /// The rounding is to nearest, ties to even, of the exact sum (not of a partial sum).
    #[test]
    fn half_way_cases_round_to_even_of_the_exact_sum() {
        // 1 + 2^-53 is half-way between 1 and the next f64: ties to even, 1
        assert_eq!(sum(&[1.0, 2f64.powi(-53)]), 1.0);
        // a little more than half-way: up
        assert_eq!(sum(&[1.0, 2f64.powi(-53), 2f64.powi(-80)]), 1.0 + f64::EPSILON);
        // a little less: down
        assert_eq!(sum(&[1.0, 2f64.powi(-53), -2f64.powi(-80)]), 1.0);
    }
}
