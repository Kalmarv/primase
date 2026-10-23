//! Sampling a decoded texture: each policy the caller's, stated. Bilinear filtering between texel centres (a texel
//! `i` covering `[i, i + 1)`, `uv` scaled by the level's size, minus a half), each axis's address mode, the colour's
//! transfer before filtering, at a level the caller picks. Two precisions, each a fixed order of operations: f64
//! ([`Levels::bilinear`]) and f32 over texels decoded once ([`Decoded::bilinear`]). A level of detail from a sample's
//! footprint ([`footprint_lod`]) and the two levels around it mixed ([`Levels::trilinear`]).

use primase_core::{ColorSpace, TexRole, Texture};

use crate::{Error, Result, TextureMetadata, map_rgba8_levels, srgb8_to_linear};

/// Where a coordinate outside 0..1 reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Address {
    /// Repeats the texture.
    Wrap,
    /// The edge texel.
    Clamp,
    /// Repeats the texture, every other copy mirrored.
    Mirror,
    /// Mirrored once about the origin, then the edge texel.
    MirrorOnce,
}

impl Address {
    fn index(self, i: i64, size: u32) -> usize {
        let n = i64::from(size.max(1));
        match self {
            Address::Wrap => i.rem_euclid(n) as usize,
            Address::Clamp => i.clamp(0, n - 1) as usize,
            // texel -1 is texel 0 mirrored, -2 is 1, ...: the pattern 0 1 .. n-1 n-1 .. 0 repeats every 2n texels
            Address::Mirror => {
                let m = i.rem_euclid(2 * n);
                (if m < n { m } else { 2 * n - 1 - m }) as usize
            }
            // mirrored once about the origin (-1 reads 0, -2 reads 1, ...), then the edge texel
            Address::MirrorOnce => (if i < 0 { -1 - i } else { i }).clamp(0, n - 1) as usize,
        }
    }
}

/// A texel channel's value before filtering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transfer {
    /// `x / 255`.
    Stored,
    /// Red, green and blue decoded from sRGB to linear (as a GPU's sRGB view), alpha `x / 255`.
    SrgbColour,
}

/// An RGBA8 texture's stored levels, decoded once, row by row.
#[derive(Clone, Debug)]
pub struct Levels {
    pub width: u32,
    pub height: u32,
    pub levels: Vec<Vec<u8>>,
}

impl Levels {
    /// Every stored level of `t`, decoded to RGBA8 as stored (no transfer applied).
    pub fn of(t: &Texture) -> Result<Levels> {
        let meta = TextureMetadata {
            color_space: ColorSpace::Linear,
            role: TexRole::Other,
            normal: None,
        };
        let d = map_rgba8_levels(t, 0..t.levels.len(), meta, |x| Ok(x.rgba))?;
        Ok(Levels {
            width: d.width,
            height: d.height,
            levels: d.levels,
        })
    }

    /// One level of RGBA8 texels.
    pub fn one(width: u32, height: u32, rgba: Vec<u8>) -> Result<Levels> {
        let count = (width as usize)
            .checked_mul(height as usize)
            .filter(|&n| n > 0)
            .ok_or_else(|| Error::Invalid("sampled texture: invalid dimensions".into()))?;
        if count.checked_mul(4) != Some(rgba.len()) {
            return Err(Error::Invalid("sampled texture: RGBA length mismatch".into()));
        }
        Ok(Levels {
            width,
            height,
            levels: vec![rgba],
        })
    }

    /// Level `level`'s size: the base halved per level, at least 1.
    pub fn size(&self, level: usize) -> (u32, u32) {
        let s = u32::try_from(level).unwrap_or(31);
        (
            (self.width >> s.min(31)).max(1),
            (self.height >> s.min(31)).max(1),
        )
    }

    /// The texel at (x, y) of level `level`, addressed by `address`.
    pub fn texel(&self, level: usize, x: i64, y: i64, address: [Address; 2]) -> [u8; 4] {
        let (w, h) = self.size(level);
        let (x, y) = (address[0].index(x, w), address[1].index(y, h));
        let i = (y * w as usize + x) * 4;
        let l = &self.levels[level];
        [l[i], l[i + 1], l[i + 2], l[i + 3]]
    }

    /// Bilinear at `uv` of level `level`, in f64: weights `(1 - tx)(1 - ty)`, `tx (1 - ty)`, `(1 - tx) ty`, `tx ty`
    /// summed in that order.
    pub fn bilinear(
        &self,
        level: usize,
        uv: [f64; 2],
        address: [Address; 2],
        transfer: Transfer,
    ) -> [f64; 4] {
        let (w, h) = self.size(level);
        let (x, y) = (uv[0] * f64::from(w) - 0.5, uv[1] * f64::from(h) - 0.5);
        let (fx, fy) = (x.floor(), y.floor());
        let (tx, ty) = (x - fx, y - fy);
        let (ix, iy) = (fx as i64, fy as i64);
        let mut out = [0.0; 4];
        for (dx, dy, wt) in [
            (0, 0, (1.0 - tx) * (1.0 - ty)),
            (1, 0, tx * (1.0 - ty)),
            (0, 1, (1.0 - tx) * ty),
            (1, 1, tx * ty),
        ] {
            let t = self.texel(level, ix + dx, iy + dy, address);
            for c in 0..4 {
                out[c] += wt
                    * match transfer {
                        Transfer::SrgbColour if c < 3 => srgb8_to_linear(t[c]),
                        _ => f64::from(t[c]) / 255.0,
                    };
            }
        }
        out
    }

    /// Level `level` decoded once to f32 by `transfer`, for [`Decoded::bilinear`].
    pub fn decoded(&self, level: usize, transfer: Transfer) -> Decoded {
        let (w, h) = self.size(level);
        let texels = self.levels[level]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                std::array::from_fn(|c| match transfer {
                    Transfer::SrgbColour if c < 3 => srgb8_to_linear(p[c]) as f32,
                    _ => p[c] as f32 / 255.0,
                })
            })
            .collect();
        Decoded {
            width: w as usize,
            height: h as usize,
            texels,
        }
    }
}

/// The level of detail of a footprint: the UV's screen derivatives `gradients` (d/dx, then d/dy) in texels of a
/// `width` x `height` base level, the longer one's base-2 logarithm (Direct3D's isotropic rule), plus `bias`. A
/// footprint of no extent gives negative infinity (the finest level, whatever the bias).
pub fn footprint_lod(width: u32, height: u32, gradients: [[f64; 2]; 2], bias: f64) -> f64 {
    let texels = |d: [f64; 2]| (d[0] * f64::from(width)).hypot(d[1] * f64::from(height));
    texels(gradients[0]).max(texels(gradients[1])).log2() + bias
}

impl Levels {
    /// Trilinear at `uv`: the level of detail `lod` clamped to the stored levels, the bilinear values
    /// ([`Levels::bilinear`]) of the level below it and the next mixed by its fraction (`a + (b - a) f`); a level
    /// of detail that isn't a number is refused.
    pub fn trilinear(
        &self,
        uv: [f64; 2],
        lod: f64,
        address: [Address; 2],
        transfer: Transfer,
    ) -> Result<[f64; 4]> {
        if lod.is_nan() {
            return Err(Error::Invalid(
                "sampled texture: a level of detail that isn't a number".into(),
            ));
        }
        let last = self.levels.len().saturating_sub(1);
        let lod = lod.clamp(0.0, last as f64);
        let below = lod.floor();
        let (l0, f) = (below as usize, lod - below);
        let a = self.bilinear(l0, uv, address, transfer);
        if f == 0.0 {
            return Ok(a);
        }
        let b = self.bilinear((l0 + 1).min(last), uv, address, transfer);
        Ok(std::array::from_fn(|c| a[c] + (b[c] - a[c]) * f))
    }
}

/// One level's texels in f32, their transfer applied.
#[derive(Clone, Debug)]
pub struct Decoded {
    width: usize,
    height: usize,
    texels: Vec<[f32; 4]>,
}

impl Decoded {
    pub fn dimensions(&self) -> [usize; 2] {
        [self.width, self.height]
    }
    pub fn decoded_bytes(&self) -> usize {
        self.texels.len() * 16
    }
    /// Bilinear at `uv`, in f32 (the coordinate, the weights `(1 - t)` and `t` per axis, and the sum over the four
    /// texels row by row); a coordinate that isn't finite or past an i32's range is refused.
    pub fn bilinear(&self, uv: [f32; 2], address: [Address; 2]) -> Result<[f32; 4]> {
        let x = uv[0] * self.width as f32 - 0.5;
        let y = uv[1] * self.height as f32 - 0.5;
        if !x.is_finite() || !y.is_finite() || x.abs() >= 2147483520.0 || y.abs() >= 2147483520.0 {
            return Err(Error::Invalid(
                "sampled texture: nonfinite or excessive texel coordinate".into(),
            ));
        }
        let (ix, iy) = (x.floor() as i64, y.floor() as i64);
        let (tx, ty) = (x - ix as f32, y - iy as f32);
        let mut result = [0.; 4];
        for dy in 0..2 {
            for dx in 0..2 {
                let xx = address[0].index(ix + dx, self.width as u32);
                let yy = address[1].index(iy + dy, self.height as u32);
                let weight = (if dx == 1 { tx } else { 1. - tx }) * (if dy == 1 { ty } else { 1. - ty });
                for (c, out) in result.iter_mut().enumerate() {
                    *out += self.texels[yy * self.width + xx][c] * weight;
                }
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test fixtures")]
mod tests {
    use super::*;

    /// Texel indices out of range, worked by hand for four texels: repeat, mirrored repeat (-1 reads 0, 4 reads 3, the
    /// pattern 0 1 2 3 3 2 1 0 repeating), clamp, and mirrored once about the origin then the edge.
    #[test]
    fn addressing_out_of_range() {
        let read =
            |a: Address, from: i64, to: i64| -> Vec<usize> { (from..=to).map(|i| a.index(i, 4)).collect() };
        assert_eq!(read(Address::Wrap, -5, 5), [3, 0, 1, 2, 3, 0, 1, 2, 3, 0, 1]);
        assert_eq!(
            read(Address::Mirror, -5, 8),
            [3, 3, 2, 1, 0, 0, 1, 2, 3, 3, 2, 1, 0, 0]
        );
        assert_eq!(read(Address::Clamp, -2, 5), [0, 0, 0, 1, 2, 3, 3, 3]);
        assert_eq!(
            read(Address::MirrorOnce, -5, 5),
            [3, 3, 2, 1, 0, 0, 1, 2, 3, 3, 3]
        );
    }

    /// A 2 x 1 texture, black then white: halfway between the texel centres is half white; at the left texel's centre
    /// black; wrapped, the left edge blends the right texel in, clamped it doesn't; sRGB colour decoded before
    /// filtering (a half of white's linear 1), alpha as stored.
    #[test]
    fn bilinear_between_texel_centres() {
        let l = Levels::one(2, 1, vec![0, 0, 0, 0, 255, 255, 255, 255]).unwrap();
        let wrap = [Address::Wrap; 2];
        let clamp = [Address::Clamp; 2];
        assert_eq!(l.bilinear(0, [0.5, 0.5], wrap, Transfer::Stored), [0.5; 4]);
        assert_eq!(l.bilinear(0, [0.25, 0.5], wrap, Transfer::Stored), [0.0; 4]);
        assert_eq!(l.bilinear(0, [0.0, 0.5], wrap, Transfer::Stored), [0.5; 4]);
        assert_eq!(l.bilinear(0, [0.0, 0.5], clamp, Transfer::Stored), [0.0; 4]);
        let d = l.decoded(0, Transfer::SrgbColour);
        let half = d.bilinear([0.5, 0.5], wrap).unwrap();
        assert!(half.iter().all(|c| (c - 0.5).abs() < 1e-6), "{half:?}");
        // a 50% grey texel: its sRGB colour decodes to linear 0.2158...; its alpha stays 128/255
        let g = Levels::one(1, 1, vec![128, 128, 128, 128]).unwrap();
        let c = g.bilinear(0, [0.5, 0.5], wrap, Transfer::SrgbColour);
        assert!(
            (c[0] - crate::srgb_to_linear(128.0 / 255.0)).abs() < 1e-15
                && (c[3] - 128.0 / 255.0).abs() < 1e-15
        );
    }

    /// A footprint of one base texel is level 0, of four texels level 2 (the longer derivative counts, in texels of
    /// each axis's size); trilinear between levels 0 (black) and 1 (white) gives the fraction, and clamps to the
    /// stored levels.
    #[test]
    fn trilinear_at_a_footprint() {
        assert_eq!(
            footprint_lod(8, 4, [[1.0 / 8.0, 0.0], [0.0, 1.0 / 4.0]], 0.0),
            0.0
        );
        assert_eq!(footprint_lod(8, 4, [[0.0, 0.0], [0.0, 1.0]], 0.0), 2.0);
        assert_eq!(footprint_lod(8, 4, [[0.0; 2]; 2], 3.0), f64::NEG_INFINITY);
        let l = Levels {
            width: 2,
            height: 2,
            levels: vec![vec![0; 16], vec![255; 4]],
        };
        let wrap = [Address::Wrap; 2];
        let at = |lod| l.trilinear([0.5, 0.5], lod, wrap, Transfer::Stored).unwrap()[0];
        assert_eq!((at(-1.0), at(0.25), at(1.0), at(7.0)), (0.0, 0.25, 1.0, 1.0));
        assert!(l.trilinear([0.5, 0.5], f64::NAN, wrap, Transfer::Stored).is_err());
    }
}
