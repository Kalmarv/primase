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

/// Emission of the emissive mask's lane `m` at `Emissive Scale` `scale` over the exposure `exposure`.
pub fn emission(m: f64, scale: f64, exposure: f64) -> f64 {
    m * scale / exposure
}

/// Occlusion of the mask's blue `b` at the switch `on`: `1 + on (b - 1)`.
pub fn occlusion(b: f64, on: f64) -> f64 {
    1.0 + on * (b - 1.0)
}

/// A lake's water type `p` from its Params atlas texel's colour `rgb` (0-1, true RGB): `rgb^2 (40, 100, 40)` (the
/// G-buffer's square-root form; docs/material-spec.md §24.3, the scale `cb1[83]` found in the snapshot, §17.1).
pub fn water_type(rgb: [f64; 3]) -> [f64; 3] {
    let scale = [40.0, 100.0, 40.0];
    std::array::from_fn(|k| rgb[k] * rgb[k] * scale[k])
}

/// The lake shader's absorption per metre of water type `p = (a, b, c)` (pack 2185, executed: §24.4): its pure-water
/// term plus the type's linear map.
pub fn lake_sigma(p: [f64; 3]) -> [f64; 3] {
    let [a, b, c] = p;
    let base = [0.32549 + 0.000902, 0.098039 + 0.003843, 0.098039 + 0.00451];
    let ma = [0.047059, 0.039216, 0.494118];
    let mb = [0.004980 + 0.115803, 0.005333 + 0.126938, 0.005765 + 0.134245];
    let mc = [0.018627, 0.055882, 0.125490];
    std::array::from_fn(|k| base[k] + ma[k] * a + mb[k] * b + mc[k] * c)
}

/// A water column's opacity at depth `d` metres: `1 - max_c exp(-sigma_c d)` (the transmittance the shader writes,
/// `min(1, max_c exp(-sigma_c d))`, taken from 1).
pub fn water_opacity(sigma: [f64; 3], d: f64) -> f64 {
    let t = sigma.iter().map(|s| (-s * d).exp()).fold(0.0, f64::max).min(1.0);
    1.0 - t
}

/// The water body's diffuse reflectance `0.66 bb / a` of water type `p` at depth `depth` metres, as the sun-light
/// pass's water branch computes it (pack 770, read: §24.5; its absorption `a` and backscatter `bb`, with
/// `w = sat(0.05 depth)`).
pub fn water_reflectance(p: [f64; 3], depth: f64) -> [f64; 3] {
    let [pa, pb, pc] = p;
    let w = sat(0.05 * depth);
    let pure = [0.429412, 0.15098, 0.201961];
    let deep = [0.041176, 0.04902, -0.001961];
    let ca = [0.047059, 0.039216, 0.494118];
    let cb = [0.00498, 0.005333, 0.005765];
    let cc = [0.018627, 0.055882, 0.12549];
    let bb0 = [0.000147, 0.00049, 0.000427];
    let bbw = [0.000002, 0.000259, 0.000322];
    let bbb = [0.000467, 0.00064, 0.00077];
    std::array::from_fn(|k| {
        let a = pure[k] + w * deep[k] + pc * cc[k] + pa * ca[k] + pb * cb[k];
        let bb = 0.5 * (bb0[k] + w * bbw[k]) + pb * bbb[k];
        0.66 * bb / a
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(x: f64, y: f64, within: f64) -> bool {
        (x - y).abs() <= within
    }

    /// The notes' table of the lake shader's sigma and opacities (§24.4: computed from the executed shader), and the
    /// reflectance hues of §24.5, at their printed precision, for the types the notes state exactly (their custom
    /// type's `p` is printed rounded).
    #[test]
    fn the_lake_water_types_give_the_notes_values() {
        for (p, sigma, opacity) in [
            ([4.0, 0.5, 5.0], [0.668, 0.604, 2.776], [0.26, 0.45, 0.70, 0.91]),
            ([20.0, 4.0, 4.0], [1.825, 1.639, 11.05], [0.56, 0.81, 0.96, 1.00]),
            ([1.0, 8.0, 0.0], [1.340, 1.199, 1.717], [0.45, 0.70, 0.91, 0.99]),
        ] {
            let s = lake_sigma(p);
            for k in 0..3 {
                assert!(close(s[k], sigma[k], 0.006), "{p:?}: {s:?}");
            }
            for (d, o) in [0.5, 1.0, 2.0, 4.0].into_iter().zip(opacity) {
                assert!(
                    close(water_opacity(s, d), o, 0.006),
                    "{p:?} at {d}: {}",
                    water_opacity(s, d)
                );
            }
        }
        for (p, hue) in [
            ([4.0, 0.5, 5.0], [0.44, 1.00, 0.22]),
            ([20.0, 4.0, 4.0], [0.56, 1.00, 0.13]),
            ([1.0, 8.0, 0.0], [0.32, 1.00, 0.38]),
        ] {
            let r = water_reflectance(p, 1.0);
            let top = r.iter().copied().fold(0.0, f64::max);
            for k in 0..3 {
                assert!(close(r[k] / top, hue[k], 0.006), "{p:?}: {r:?}");
            }
        }
    }
}
