//! The two 3-vector types of the reference writer, each with its own rounding: SharpDX's (`Sdx`: length through a
//! double square root, normalise by multiplying with the inverse, nothing done under 1e-6) and System.Numerics'
//! (`Num`: single-precision square root, normalise by dividing). Every operation is evaluated in the same order as
//! there, so results match bit for bit (checked against a recorded fixture).

use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

macro_rules! vec3 {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Default)]
        pub struct $name {
            pub x: f32,
            pub y: f32,
            pub z: f32,
        }

        impl $name {
            pub const ZERO: $name = $name {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            };
            pub const UNIT_X: $name = $name {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            };
            pub const UNIT_Y: $name = $name {
                x: 0.0,
                y: 1.0,
                z: 0.0,
            };
            pub const UNIT_Z: $name = $name {
                x: 0.0,
                y: 0.0,
                z: 1.0,
            };

            pub const fn new(x: f32, y: f32, z: f32) -> Self {
                $name { x, y, z }
            }
            pub const fn splat(v: f32) -> Self {
                $name { x: v, y: v, z: v }
            }
            pub fn at(v: &[f32], i: usize) -> Self {
                $name {
                    x: v[i],
                    y: v[i + 1],
                    z: v[i + 2],
                }
            }
            pub fn length_squared(self) -> f32 {
                self.x * self.x + self.y * self.y + self.z * self.z
            }
            pub fn dot(self, o: Self) -> f32 {
                self.x * o.x + self.y * o.y + self.z * o.z
            }
            pub fn cross(self, o: Self) -> Self {
                $name {
                    x: self.y * o.z - self.z * o.y,
                    y: self.z * o.x - self.x * o.z,
                    z: self.x * o.y - self.y * o.x,
                }
            }
            pub fn min(self, o: Self) -> Self {
                $name {
                    x: min(self.x, o.x),
                    y: min(self.y, o.y),
                    z: min(self.z, o.z),
                }
            }
            pub fn max(self, o: Self) -> Self {
                $name {
                    x: max(self.x, o.x),
                    y: max(self.y, o.y),
                    z: max(self.z, o.z),
                }
            }
        }

        impl Add for $name {
            type Output = $name;
            fn add(self, o: Self) -> Self {
                $name {
                    x: self.x + o.x,
                    y: self.y + o.y,
                    z: self.z + o.z,
                }
            }
        }
        impl AddAssign for $name {
            fn add_assign(&mut self, o: Self) {
                *self = *self + o;
            }
        }
        impl Sub for $name {
            type Output = $name;
            fn sub(self, o: Self) -> Self {
                $name {
                    x: self.x - o.x,
                    y: self.y - o.y,
                    z: self.z - o.z,
                }
            }
        }
        impl SubAssign for $name {
            fn sub_assign(&mut self, o: Self) {
                *self = *self - o;
            }
        }
        impl Neg for $name {
            type Output = $name;
            fn neg(self) -> Self {
                $name {
                    x: -self.x,
                    y: -self.y,
                    z: -self.z,
                }
            }
        }
        /// Component-wise division by a scalar.
        impl Div<f32> for $name {
            type Output = $name;
            fn div(self, s: f32) -> Self {
                $name {
                    x: self.x / s,
                    y: self.y / s,
                    z: self.z / s,
                }
            }
        }
        impl Mul<f32> for $name {
            type Output = $name;
            fn mul(self, s: f32) -> Self {
                $name {
                    x: self.x * s,
                    y: self.y * s,
                    z: self.z * s,
                }
            }
        }
    };
}

/// `Vector3.Min` / `Max` per component: `a < b ? a : b`.
fn min(a: f32, b: f32) -> f32 {
    if a < b { a } else { b }
}

fn max(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

vec3!(Sdx);
vec3!(Num);

impl Sdx {
    /// `(float)Math.Sqrt(x * x + y * y + z * z)`: the f32 sum, a double square root, rounded to f32.
    pub fn length(self) -> f32 {
        f64::from(self.length_squared()).sqrt() as f32
    }
    /// `Vector3.Normalize`: multiplied by `1 / length`, unchanged when the length is under 1e-6.
    pub fn normalize(self) -> Self {
        let len = self.length();
        if len.abs() < 1e-6 {
            return self;
        }
        let inv = 1.0 / len;
        self * inv
    }
}

impl Num {
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }
    /// `Vector3.Normalize`: divided by the length.
    pub fn normalize(self) -> Self {
        self / self.length()
    }
    /// `Vector3.Lerp`.
    pub fn lerp(a: Self, b: Self, t: f32) -> Self {
        a * (1.0 - t) + b * t
    }
}

impl From<Sdx> for Num {
    fn from(v: Sdx) -> Num {
        Num::new(v.x, v.y, v.z)
    }
}
impl From<Num> for Sdx {
    fn from(v: Num) -> Sdx {
        Sdx::new(v.x, v.y, v.z)
    }
}
