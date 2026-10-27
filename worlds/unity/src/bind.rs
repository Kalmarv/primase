//! A variant of a material's compiled programs ([`UnityProgramVariant`], from `unity.programs`) bound for evaluation
//! on the CPU ([`iridia`]): what Unity itself sets when it draws, as far as the material says it.
//!
//! - [`bind`]: the program (its DXBC, whose reflection Unity strips) with the constant layout its parameters record
//!   (field name, buffer slot, byte offset), and its texture slots by property name.
//! - [`constants`]: the material's floats, colours (gamma-encoded as stored, set linear, as Unity sets them in a
//!   linear-space player) and texture tilings (`<property>_ST`: scale x and y, offset x and y) into that layout; the
//!   fields no property sets stay zero, named in what it returns.
//! - [`directional_light`]: the built-in renderer's forward globals for one directional light and nothing else (the
//!   caller's choice of light; ambient, probes and shadows stated off), returning the fields it set so the caller
//!   reports the bindings it evaluates with.
//! - [`sampled_texture`]: a texture as a Unity shader samples it.
//!
//! Struct fields (instancing's per-draw arrays) are not laid out: a variant drawn without instancing reads none.

use std::collections::BTreeMap;

use iridia_exec::{Field, Filtering, Layout, SampledTexture, SamplerUse, evaluate::Constants};
use primase_core::{ColorSpace, Texture};
use primase_foundations::color::srgb_to_linear;
use primase_foundations::{sampling::Address, sampling::Transfer};

use crate::components::{Shape, UnityMaterial, UnityProgramVariant, UnityTexture};
use crate::{Result, invalid};

/// A program bound to its layout.
#[derive(Debug)]
pub struct Bound {
    pub shader: iridia_exec::ProgramShader,
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
    let program = iridia_dxbc::lift(&p.code).map_err(|e| invalid(format!("program of {name}: {e}")))?;
    let shader =
        iridia_exec::ProgramShader::new(program).map_err(|e| invalid(format!("program of {name}: {e}")))?;
    let read = iridia_exec::PixelShader::constant_buffers(&shader).to_vec();
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

/// A texture as a Unity shader samples it ([`SampledTexture`]): its rows as the GPU holds them (Unity's v runs up,
/// row 0 at v = 0, where the world stores the top row first), each axis addressed by its `TextureWrapMode` (0 repeat, 1
/// clamp, 2 mirror, 3 mirror once), filtered by its `FilterMode` (0 point and 1 bilinear at the nearest level, 2
/// trilinear between levels), its `m_MipBias` added to the level of detail, an sRGB texture's colour decoded before
/// filtering. Anisotropic filtering is not modelled: a texture Unity filters anisotropically
/// ([`UnityTexture::anisotropic`]) is sampled isotropically, for the caller to report. A wrap or filter mode Unity
/// does not define is refused by its number. A sample at an implicit level
/// without a quad footprint is refused unless the caller states the level it takes
/// ([`SampledTexture::with_footprintless_level`]).
pub fn sampled_texture(t: &Texture, unity: &UnityTexture) -> Result<SampledTexture> {
    let address = |axis: usize| -> Result<Address> {
        Ok(match unity.wrap[axis] {
            0 => Address::Wrap,
            1 => Address::Clamp,
            2 => Address::Mirror,
            3 => Address::MirrorOnce,
            other => {
                return Err(invalid(format!(
                    "TextureWrapMode {other} along {}, which Unity does not define",
                    ["u", "v"][axis]
                )));
            }
        })
    };
    let filtering = match unity.filter {
        0 => Filtering::Point,
        1 => Filtering::Bilinear,
        2 => Filtering::Trilinear,
        other => {
            return Err(invalid(format!(
                "FilterMode {other}, which Unity does not define"
            )));
        }
    };
    let transfer = match t.color_space {
        ColorSpace::Srgb => Transfer::SrgbColour,
        ColorSpace::Linear => Transfer::Stored,
    };
    let levels = primase_image::levels(t)
        .and_then(|l| l.rows_reversed().map_err(Into::into))
        .map_err(|e| invalid(format!("decoding a texture to sample: {e}")))?;
    let sampler = SamplerUse {
        address: [address(0)?, address(1)?],
        mip_lod_bias: Some(unity.mip_bias),
    };
    SampledTexture::new(levels, transfer, sampler, filtering).map_err(invalid)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::components::UnityObject;
    use iridia_exec::{Image, Lod};
    use primase_core::{TexFormat, TexRole};

    fn texture(width: u32, height: u32, levels: &[&[[u8; 4]]], space: ColorSpace) -> Texture {
        Texture {
            width,
            height,
            format: TexFormat::Rgba8Unorm,
            levels: levels
                .iter()
                .map(|rows_top_first| rows_top_first.concat())
                .collect(),
            color_space: space,
            role: TexRole::Other,
            normal: None,
        }
    }

    fn unity(wrap: [i64; 2], filter: i64, mip_bias: f32) -> UnityTexture {
        UnityTexture {
            texture: UnityObject {
                file: "sharedassets0.assets".into(),
                path_id: 1,
            },
            format: 4,
            srgb: false,
            usage: 0,
            wrap,
            filter,
            mip_bias,
            aniso: 1,
            not_decoded: None,
        }
    }

    /// A 2x2 texture whose top row (world) is red and green, bottom row blue and white: Unity's v = 0.25 is the
    /// bottom row; repeat wraps, clamp holds the edge; point sampling takes a texel, bilinear halfway mixes.
    #[test]
    fn a_unity_texture_samples_with_v_up_and_its_wrap() {
        let t = texture(
            2,
            2,
            &[&[
                [255, 0, 0, 255],
                [0, 255, 0, 255],
                [0, 0, 255, 255],
                [255, 255, 255, 255],
            ]],
            ColorSpace::Linear,
        );
        let base = Lod::Level(0.0);
        let point = sampled_texture(&t, &unity([0, 1], 0, 0.0)).unwrap();
        assert_eq!(point.sample([0.25, 0.25], base).unwrap(), [0., 0., 1., 1.]);
        assert_eq!(point.sample([0.25, 0.75], base).unwrap(), [1., 0., 0., 1.]);
        // u repeats: 1.25 is 0.25; v clamps: 1.5 is the top row
        assert_eq!(point.sample([1.25, 1.5], base).unwrap(), [1., 0., 0., 1.]);
        let bilinear = sampled_texture(&t, &unity([0, 0], 1, 0.0)).unwrap();
        assert_eq!(bilinear.sample([0.5, 0.5], base).unwrap(), [0.5, 0.5, 0.5, 1.]);
        // an implicit level without a footprint is the caller's to state
        assert!(point.sample([0.25, 0.25], Lod::Implicit).is_err());
        let stated = point.with_footprintless_level(0.0).unwrap();
        assert_eq!(
            stated.sample([0.25, 0.25], Lod::Implicit).unwrap(),
            [0., 0., 1., 1.]
        );
    }

    /// sRGB texels are decoded to linear.
    #[test]
    fn an_srgb_texture_samples_linear() {
        let t = texture(1, 1, &[&[[128, 128, 128, 128]]], ColorSpace::Srgb);
        let s = sampled_texture(&t, &unity([0, 0], 0, 0.0))
            .unwrap()
            .sample([0.5, 0.5], Lod::Level(0.0))
            .unwrap();
        assert!((s[0] - srgb_to_linear(128.0 / 255.0) as f32).abs() < 1e-6);
        assert!((s[3] - 128.0 / 255.0).abs() < 1e-6);
    }

    /// The texture's mip bias moves the level it is sampled at; a wrap or filter mode Unity does not define is
    /// refused, never read as repeat or trilinear.
    #[test]
    fn the_mip_bias_applies_and_undefined_modes_are_refused() {
        let black = [0, 0, 0, 255];
        let t = texture(2, 2, &[&[black; 4], &[[255; 4]]], ColorSpace::Linear);
        let at = |bias: f32| {
            sampled_texture(&t, &unity([0, 0], 0, bias))
                .unwrap()
                .sample([0.25, 0.25], Lod::Level(0.0))
                .unwrap()[0]
        };
        assert_eq!((at(0.0), at(1.0)), (0.0, 1.0));
        for (wrap, filter) in [([4, 0], 1), ([0, -1], 1), ([0, 0], 3)] {
            let e = sampled_texture(&t, &unity(wrap, filter, 0.0))
                .unwrap_err()
                .to_string();
            assert!(e.contains("which Unity does not define"), "{e}");
        }
    }

    #[test]
    fn packed_words_pad_array_elements_and_matrix_rows() {
        assert_eq!(words(Shape::Vector { columns: 3 }, 0), 3);
        assert_eq!(words(Shape::Vector { columns: 1 }, 3), 9);
        assert_eq!(words(Shape::Matrix { rows: 4 }, 0), 16);
        assert_eq!(words(Shape::Matrix { rows: 4 }, 2), 32);
    }
}
