//! The per-texel formulas `lower-anvil-material` makes its textures of, each one value as the template's pixel shader
//! computes it (the Riders Republic notes, docs/material-spec.md §4, §13-15), on values as the shader samples them
//! (linear). Public so the shader oracle (worlds/anvil/tests/shader_oracle.rs) checks these very functions against
//! the game's own pixel shaders run in primase-dxbc.

/// `S(x) = x x (3 - 2 x)`.
pub fn smooth(x: f64) -> f64 {
    x * x * (3.0 - 2.0 * x)
}

pub fn sat(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

/// The HSV tint `T(c; h, s, v)` the shaders compute: to HSV (hue in 0..1), the hue offset added and kept as
/// `sign(h + dh) frc(|h + dh|)` (not wrapped: a negative sum keeps its sign, and the way back reads it so), saturation
/// and value offset (clamped), back. The Riders Republic notes found the unwrapped hue running the shader
/// (docs/material-spec.md §8); the shader oracle (worlds/anvil/tests/shader_oracle.rs) checks it with negative sums.
pub fn tint(c: [f64; 3], dh: f64, ds: f64, dv: f64) -> [f64; 3] {
    let [r, g, b] = c;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d) / 6.0
    } else if max == g {
        (2.0 + (b - r) / d) / 6.0
    } else {
        (4.0 + (r - g) / d) / 6.0
    };
    let s = if max > 0.0 { d / max } else { 0.0 };
    let x = h.rem_euclid(1.0) + dh;
    let h = x.signum() * x.abs().fract();
    let (s, v) = (sat(s + ds), sat(max + dv));
    let hue = [
        (6.0 * h - 3.0).abs() - 1.0,
        2.0 - (6.0 * h - 2.0).abs(),
        2.0 - (6.0 * h - 4.0).abs(),
    ];
    hue.map(|x| ((sat(x) - 1.0) * s + 1.0) * v)
}

/// The base colour of `c` tinted by `hsl` (hue, saturation, luminosity offsets) where the mask's red `m` is: `c` lerped
/// toward [`tint`]`(c)` by `m`.
pub fn tinted(c: [f64; 3], m: f64, hsl: [f64; 3]) -> [f64; 3] {
    let tc = tint(c, hsl[0], hsl[1], hsl[2]);
    [0, 1, 2].map(|k| c[k] + (tc[k] - c[k]) * m)
}

/// How a template makes roughness of its normal map's alpha `a`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rough {
    /// `a`.
    Raw,
    /// `S(sat(a + offset))` (Tex1, Decals1).
    Offset(f64),
    /// `S(sat((a + offset - 0.1) 1.25))` (Tex2).
    Tex2(f64),
}

/// Roughness of the normal map's alpha `a` by `rough`.
pub fn roughness(a: f64, rough: Rough) -> f64 {
    match rough {
        Rough::Raw => a,
        Rough::Offset(o) => smooth(sat(a + o)),
        Rough::Tex2(o) => smooth(sat((a + o - 0.1) * 1.25)),
    }
}

/// Metalness of the normal map's blue `b`: `b * metal`, or `b` where `None`.
pub fn metalness(b: f64, metal: Option<f64>) -> f64 {
    metal.map_or(b, |m| b * m)
}

/// Occlusion of the mask's blue `b` at the switch `on`: `1 + on (b - 1)`.
pub fn occlusion(b: f64, on: f64) -> f64 {
    1.0 + on * (b - 1.0)
}
