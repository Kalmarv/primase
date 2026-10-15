//! Plain vector arithmetic. Basic operations round in f32; length and
//! normalization accumulate squares in f64 to avoid f32 underflow/overflow.
//! Normalization divides each component by the f64 length and rounds once to
//! f32. Zero normalizes to zero; callers choose any geometric fallback explicitly.
use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
impl Vec3 {
    pub const ZERO: Self = Self::splat(0.);
    pub const UNIT_X: Self = Self::new(1., 0., 0.);
    pub const UNIT_Y: Self = Self::new(0., 1., 0.);
    pub const UNIT_Z: Self = Self::new(0., 0., 1.);
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    pub const fn splat(v: f32) -> Self {
        Self::new(v, v, v)
    }
    pub fn at(v: &[f32], i: usize) -> Self {
        Self::new(v[i], v[i + 1], v[i + 2])
    }
    pub fn dot(self, b: Self) -> f32 {
        self.x * b.x + self.y * b.y + self.z * b.z
    }
    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }
    fn length64(self) -> f64 {
        let [x, y, z] = <[f32; 3]>::from(self).map(f64::from);
        (x * x + y * y + z * z).sqrt()
    }
    pub fn length(self) -> f32 {
        self.length64() as f32
    }
    pub fn normalize(self) -> Self {
        let len = self.length64();
        if len == 0. {
            Self::ZERO
        } else {
            Self::from(<[f32; 3]>::from(self).map(|x| (x as f64 / len) as f32))
        }
    }
    /// Normalize finite nonzero input; return the caller's fallback otherwise.
    pub fn normalize_or(self, fallback: Self) -> Self {
        let len = self.length64();
        if len == 0. || !len.is_finite() {
            fallback
        } else {
            self.normalize()
        }
    }
    pub fn cross(self, b: Self) -> Self {
        Self::new(
            self.y * b.z - self.z * b.y,
            self.z * b.x - self.x * b.z,
            self.x * b.y - self.y * b.x,
        )
    }
    pub fn min(self, b: Self) -> Self {
        Self::new(self.x.min(b.x), self.y.min(b.y), self.z.min(b.z))
    }
    pub fn max(self, b: Self) -> Self {
        Self::new(self.x.max(b.x), self.y.max(b.y), self.z.max(b.z))
    }
    pub fn lerp(a: Self, b: Self, t: f32) -> Self {
        a * (1. - t) + b * t
    }
}
impl From<[f32; 3]> for Vec3 {
    fn from(v: [f32; 3]) -> Self {
        Self::new(v[0], v[1], v[2])
    }
}
impl From<Vec3> for [f32; 3] {
    fn from(v: Vec3) -> Self {
        [v.x, v.y, v.z]
    }
}
impl Add for Vec3 {
    type Output = Self;
    fn add(self, b: Self) -> Self {
        Self::new(self.x + b.x, self.y + b.y, self.z + b.z)
    }
}
impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, b: Self) -> Self {
        Self::new(self.x - b.x, self.y - b.y, self.z - b.z)
    }
}
impl Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, b: f32) -> Self {
        Self::new(self.x * b, self.y * b, self.z * b)
    }
}
impl Div<f32> for Vec3 {
    type Output = Self;
    fn div(self, b: f32) -> Self {
        Self::new(self.x / b, self.y / b, self.z / b)
    }
}
impl Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}
impl AddAssign for Vec3 {
    fn add_assign(&mut self, b: Self) {
        *self = *self + b;
    }
}
impl SubAssign for Vec3 {
    fn sub_assign(&mut self, b: Self) {
        *self = *self - b;
    }
}
