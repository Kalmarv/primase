//! A variant of a material's compiled programs ([`UnityProgramVariant`], from `unity.programs`) bound for evaluation
//! on the CPU ([`primase_shader`]): what Unity itself sets when it draws, as far as the material says it.
//!
//! - [`bind`]: the program (its DXBC, whose reflection Unity strips) with the constant layout its parameters record
//!   (field name, buffer slot, byte offset), and its texture slots by property name.
//! - [`constants`]: the material's floats, colours (gamma-encoded as stored, set linear, as Unity sets them in a
//!   linear-space player) and texture tilings (`<property>_ST`: scale x and y, offset x and y) into that layout; the
//!   fields no property sets stay zero, named in what it returns.
//! - [`directional_light`]: the built-in renderer's forward globals for one directional light and nothing else (the
//!   caller's choice of light; ambient, probes and shadows stated off), returning the fields it set so the caller
//!   reports the bindings it evaluates with.
//! - [`UnityImage`]: a texture as a Unity shader samples it.
//!
//! Struct fields (instancing's per-draw arrays) are not laid out: a variant drawn without instancing reads none.

use std::collections::BTreeMap;

use primase_core::{ColorSpace, Texture};
use primase_image::srgb_to_linear;
use primase_shader::{Constants, Field, Layout};
use primase_texcodec::TexDecoder;

use crate::components::{Shape, UnityMaterial, UnityProgramVariant};
use crate::{Result, invalid};

/// A program bound to its layout.
#[derive(Debug)]
pub struct Bound {
    pub shader: primase_dxbc::Shader,
    pub layout: Layout,
    /// Each texture parameter's slot (`t<n>`), by property name.
    pub textures: BTreeMap<String, u32>,
}

/// The words a field of `shape` and `array_size` elements takes under D3D packing: a vector its columns, a matrix
/// four per row (Unity's are row-major float4 rows), each array element but the last padded to whole registers.
fn words(shape: Shape, array_size: u32) -> u32 {
    let one = match shape {
        Shape::Vector { columns } => columns,
        Shape::Matrix { rows } => rows * 4,
    };
    match array_size {
        0 => one,
        n => (n - 1) * one.div_ceil(4) * 4 + one,
    }
}

/// Bind variant `p` of shader `name`: its program read without reflection, the layout of every field of a buffer the
/// program reads, and its texture slots. A field named twice is an error.
pub fn bind(name: &str, p: &UnityProgramVariant) -> Result<Bound> {
    let shader = primase_dxbc::Shader::parse_stripped(&p.code)
        .map_err(|e| invalid(format!("program of {name}: {e}")))?;
    let read = shader.program.constant_buffers().to_vec();
    let mut layout = Layout {
        buffers: read.iter().map(|&(slot, rows)| (slot, rows * 4)).collect(),
        fields: BTreeMap::new(),
    };
    for cb in &p.parameters.constant_buffers {
        let Some(slot) = cb.slot.filter(|s| read.iter().any(|&(r, _)| r == *s)) else {
            continue;
        };
        for f in &cb.fields {
            let field = Field {
                buffer: slot,
                offset: f.offset / 4,
                words: words(f.shape, f.array_size),
            };
            if !f.offset.is_multiple_of(4) || layout.fields.insert(f.name.clone(), field).is_some() {
                return Err(invalid(format!(
                    "program of {name}: field {} unaligned or named twice",
                    f.name
                )));
            }
        }
    }
    let textures = p
        .parameters
        .textures
        .iter()
        .map(|t| (t.name.clone(), t.slot))
        .collect();
    Ok(Bound {
        shader,
        layout,
        textures,
    })
}

/// What [`constants`] set and left.
#[derive(Debug)]
pub struct MaterialConstants {
    pub constants: Constants,
    /// Layout fields no material property set (zero), by name.
    pub unset: Vec<String>,
}

/// The material's properties in `b`'s layout (see the module documentation).
pub fn constants(b: &Bound, m: &UnityMaterial) -> Result<MaterialConstants> {
    let mut c = Constants::new(&b.layout);
    let mut set = std::collections::BTreeSet::new();
    let mut put = |c: &mut Constants, name: &str, values: &[f32]| -> Result<()> {
        if c.set(name, values).map_err(|e| invalid(e.to_string()))? {
            set.insert(name.to_owned());
        }
        Ok(())
    };
    for (name, _) in &m.floats {
        if let Some(v) = m.float(name) {
            put(&mut c, name, &[v])?;
        }
    }
    for (name, _) in &m.colors {
        if let Some(v) = m.color(name) {
            let l = |x: f32| srgb_to_linear(f64::from(x)) as f32;
            put(&mut c, name, &[l(v[0]), l(v[1]), l(v[2]), v[3]])?;
        }
    }
    for t in &m.textures {
        if let Some((scale, offset)) = t.finite_tiling() {
            put(
                &mut c,
                &format!("{}_ST", t.name),
                &[scale[0], scale[1], offset[0], offset[1]],
            )?;
        }
    }
    let unset = b
        .layout
        .fields
        .keys()
        .filter(|k| !set.contains(*k))
        .cloned()
        .collect();
    Ok(MaterialConstants { constants: c, unset })
}

/// Unity's built-in forward globals for one directional light of linear colour `color` arriving from `towards` (the
/// world direction to the light, `_WorldSpaceLightPos0` with w 0) and nothing else: spherical-harmonics ambient zero,
/// no light probe volume (`unity_ProbeVolumeParams` x 0, its transform and bounds zero), and the light unshadowed
/// (`unity_OcclusionMaskSelector` selecting the first channel of the attenuation the unshadowed branch fills with 1).
/// Fields the program does not read are skipped; the names of those set, returned.
pub fn directional_light(c: &mut Constants, towards: [f32; 3], color: [f32; 3]) -> Result<Vec<String>> {
    let mut set = Vec::new();
    let mut put = |name: &str, values: &[f32]| -> Result<()> {
        if c.set(name, values).map_err(|e| invalid(e.to_string()))? {
            set.push(name.to_owned());
        }
        Ok(())
    };
    put("_WorldSpaceLightPos0", &[towards[0], towards[1], towards[2], 0.0])?;
    put("_LightColor0", &[color[0], color[1], color[2], 1.0])?;
    put("unity_OcclusionMaskSelector", &[1.0, 0.0, 0.0, 0.0])?;
    put("unity_ProbeVolumeParams", &[0.0; 4])?;
    put("unity_ProbeVolumeWorldToObject", &[0.0; 16])?;
    put("unity_ProbeVolumeSizeInv", &[0.0; 3])?;
    put("unity_ProbeVolumeMin", &[0.0; 3])?;
    for sh in [
        "unity_SHAr",
        "unity_SHAg",
        "unity_SHAb",
        "unity_SHBr",
        "unity_SHBg",
        "unity_SHBb",
        "unity_SHC",
    ] {
        put(sh, &[0.0; 4])?;
    }
    Ok(set)
}

/// How a Unity texture addresses a coordinate outside 0-1 (`TextureWrapMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Address {
    Repeat,
    Clamp,
    Mirror,
    MirrorOnce,
}

impl Address {
    fn of(mode: i64) -> Address {
        match mode {
            1 => Address::Clamp,
            2 => Address::Mirror,
            3 => Address::MirrorOnce,
            _ => Address::Repeat,
        }
    }

    /// Texel index `i` of a row of `n`, addressed.
    fn index(self, i: i64, n: u32) -> usize {
        let n = i64::from(n);
        let i = match self {
            Address::Repeat => i.rem_euclid(n),
            Address::Clamp => i.clamp(0, n - 1),
            Address::Mirror => {
                let m = i.rem_euclid(2 * n);
                if m < n { m } else { 2 * n - 1 - m }
            }
            Address::MirrorOnce => {
                let m = if i < 0 { -i - 1 } else { i };
                m.clamp(0, n - 1)
            }
        };
        i as usize
    }
}

/// A texture as a Unity shader samples it: the UV origin bottom-left (the world stores rows top first), each axis
/// addressed by the texture's wrap mode, texels linear (an sRGB texture's colour decoded, as the GPU does). The filter
/// is the texture's (`FilterMode`): point takes the nearest texel of the nearest mip, bilinear interpolates within the
/// nearest mip, trilinear also between the two nearest mips. The mip is that of the sample's gradients (the larger
/// footprint, in texels, of d/dx and d/dy), the largest when the sample states none.
#[derive(Debug)]
pub struct UnityImage {
    /// Width, height and linear RGBA texels (top row first) of every mip, largest first.
    levels: Vec<(u32, u32, Vec<[f32; 4]>)>,
    address: [Address; 2],
    filter: i64,
}

impl UnityImage {
    /// `t` (decoded from its format) with `wrap` (u, v) and `filter` as Unity's `TextureWrapMode` and `FilterMode`.
    pub fn new(t: &Texture, wrap: [i64; 2], filter: i64) -> Result<UnityImage> {
        let srgb = t.color_space == ColorSpace::Srgb;
        let mut levels = Vec::with_capacity(t.levels.len());
        for (i, level) in t.levels.iter().enumerate() {
            let shift = u32::try_from(i).unwrap_or(u32::MAX);
            let w = t.width.checked_shr(shift).unwrap_or(0).max(1);
            let h = t.height.checked_shr(shift).unwrap_or(0).max(1);
            let px = primase_texcodec::Decoder::default()
                .decode_rgba8(t.format, w, h, level)
                .map_err(|e| invalid(format!("decoding a texture to sample: {e}")))?;
            let texel = |p: &[u8; 4]| {
                let c = |x: u8| {
                    let x = f64::from(x) / 255.0;
                    (if srgb { srgb_to_linear(x) } else { x }) as f32
                };
                [c(p[0]), c(p[1]), c(p[2]), f32::from(p[3]) / 255.0]
            };
            levels.push((w, h, px.as_chunks::<4>().0.iter().map(texel).collect()));
        }
        if levels.is_empty() {
            return Err(invalid("a texture without levels to sample"));
        }
        Ok(UnityImage {
            levels,
            address: [Address::of(wrap[0]), Address::of(wrap[1])],
            filter,
        })
    }

    /// Mip `l` at `uv` (Unity's: v up), nearest or bilinear.
    fn level(&self, l: usize, [u, v]: [f32; 2], bilinear: bool) -> [f32; 4] {
        let (w, h, px) = &self.levels[l];
        let at = |x: i64, y: i64| {
            let x = self.address[0].index(x, *w);
            // v up: texel row y from the bottom is world row h - 1 - y
            let y = *h as usize - 1 - self.address[1].index(y, *h);
            px[y * *w as usize + x]
        };
        let (fx, fy) = (f64::from(u) * f64::from(*w), f64::from(v) * f64::from(*h));
        if !bilinear {
            return at(fx.floor() as i64, fy.floor() as i64);
        }
        let (x, y) = (fx - 0.5, fy - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (tx, ty) = ((x - x0) as f32, (y - y0) as f32);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let (a, b, c, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
        std::array::from_fn(|k| {
            let bottom = a[k] + (b[k] - a[k]) * tx;
            let top = c[k] + (d[k] - c[k]) * tx;
            bottom + (top - bottom) * ty
        })
    }
}

impl primase_shader::Image for UnityImage {
    fn sample(&self, uv: [f32; 2], lod: primase_shader::Lod) -> std::result::Result<[f32; 4], String> {
        let (w, h, _) = self.levels[0];
        let lod = match lod {
            primase_shader::Lod::Gradients([dx, dy]) => {
                let texels =
                    |d: [f32; 2]| (f64::from(d[0]) * f64::from(w)).hypot(f64::from(d[1]) * f64::from(h));
                texels(dx).max(texels(dy)).max(1e-12).log2().max(0.0)
            }
            // a scalar evaluation has no footprint: the implicit level is the finest, plus a stated bias or level
            primase_shader::Lod::Implicit => 0.0,
            primase_shader::Lod::Bias(b) => f64::from(b).max(0.0),
            primase_shader::Lod::Level(l) => f64::from(l).max(0.0),
        };
        let last = self.levels.len() - 1;
        Ok(match self.filter {
            0 => self.level((lod.round() as usize).min(last), uv, false),
            1 => self.level((lod.round() as usize).min(last), uv, true),
            _ => {
                let l0 = (lod.floor() as usize).min(last);
                let l1 = (l0 + 1).min(last);
                let t = (lod - lod.floor()) as f32;
                let (a, b) = (self.level(l0, uv, true), self.level(l1, uv, true));
                std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use primase_core::{TexFormat, TexRole};
    use primase_shader::Image;

    fn texture(width: u32, height: u32, rows_top_first: &[[u8; 4]], space: ColorSpace) -> Texture {
        Texture {
            width,
            height,
            format: TexFormat::Rgba8Unorm,
            levels: vec![rows_top_first.concat()],
            color_space: space,
            role: TexRole::Other,
            normal: None,
        }
    }

    /// A 2x2 texture whose top row (world) is red and green, bottom row blue and white: Unity's v = 0.25 is the
    /// bottom row; repeat wraps, clamp holds the edge; point sampling takes a texel, bilinear halfway mixes.
    #[test]
    fn a_unity_image_samples_with_v_up_and_its_wrap() {
        let t = texture(
            2,
            2,
            &[
                [255, 0, 0, 255],
                [0, 255, 0, 255],
                [0, 0, 255, 255],
                [255, 255, 255, 255],
            ],
            ColorSpace::Linear,
        );
        let point = UnityImage::new(&t, [0, 1], 0).unwrap();
        assert_eq!(
            point.sample([0.25, 0.25], primase_shader::Lod::Implicit).unwrap(),
            [0., 0., 1., 1.]
        );
        assert_eq!(
            point.sample([0.25, 0.75], primase_shader::Lod::Implicit).unwrap(),
            [1., 0., 0., 1.]
        );
        // u repeats: 1.25 is 0.25; v clamps: 1.5 is the top row
        assert_eq!(
            point.sample([1.25, 1.5], primase_shader::Lod::Implicit).unwrap(),
            [1., 0., 0., 1.]
        );
        let bilinear = UnityImage::new(&t, [0, 0], 1).unwrap();
        let mid = bilinear
            .sample([0.5, 0.5], primase_shader::Lod::Implicit)
            .unwrap();
        assert_eq!(mid, [0.5, 0.5, 0.5, 1.]);
    }

    /// sRGB texels are decoded to linear.
    #[test]
    fn an_srgb_image_samples_linear() {
        let t = texture(1, 1, &[[128, 128, 128, 128]], ColorSpace::Srgb);
        let s = UnityImage::new(&t, [0, 0], 0)
            .unwrap()
            .sample([0.5, 0.5], primase_shader::Lod::Implicit)
            .unwrap();
        assert!((s[0] - srgb_to_linear(128.0 / 255.0) as f32).abs() < 1e-6);
        assert!((s[3] - 128.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn packed_words_pad_array_elements_and_matrix_rows() {
        assert_eq!(words(Shape::Vector { columns: 3 }, 0), 3);
        assert_eq!(words(Shape::Vector { columns: 1 }, 3), 9);
        assert_eq!(words(Shape::Matrix { rows: 4 }, 0), 16);
        assert_eq!(words(Shape::Matrix { rows: 4 }, 2), 32);
    }
}
