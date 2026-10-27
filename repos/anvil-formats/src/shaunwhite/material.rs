//! A stored Material (class `0xB71F6261`): its blend and alpha-test fields, its colour and its texture slots.
//!
//! The body is 647 bytes in 1,971 of the 1,973 distinct Materials of the game's cells, the rest 659 and 663 (refused:
//! [`Error::Length`]), and every field below is at the same offset in all of them. It begins `[managed u8][id][class]`
//! and holds no length or count. Each texture slot is a TextureMapSpec's id (a bare u32; the spec's last u32 is the
//! TextureMap's id); the slots of the layer kind carry five f32 after it, **scale u, scale
//! v, offset u, offset v, rotation in degrees** (the UV transform the slot samples with: the scale is 1 in most, 0.25 to 8 where a texture
//! tiles, negative where it mirrors; the offset is 0 except in the second and third diffuse layers). The slots and the
//! offsets of their ids:
//!
//! | name | at | layer floats | what the slot holds |
//! |---|---|---|---|
//! | `detail` | 118 | no | a ramp (`Gen_Ramp_FakeReflection01`), a character's skin wrap |
//! | `mask` | 132 | yes | `Gen_Mask_*`, `*_Mask_*`: an RGB mask (alpha 255 in all 47 of level 01's); RGB channels weight the base tint and later layers in the shipped shader |
//! | `diffuse` | 192 | yes | the surface's colour; the one slot nearly every Material has (1,882 of 1,971) |
//! | `normal` | 233 | yes | a normal map (format code 12) |
//! | `specular` | 277 | yes | an RGB map (BC1) |
//! | `diffuse_b` | 360 | yes | a second colour layer (rust, moss, a worn paint) |
//! | `specular_b` | 401 | yes | the second layer's specular map |
//! | `diffuse_c` | 462 | yes | a third colour layer |
//! | `diffuse2` | 521 | no | a map of type 8 (`*_Diffuse2Map`): its two floats after the id are not tiling (0 and 0.5 or 1) |
//! | `generic` | 537 | no | a cube map (`*_GenericMap`, map type 7): the reflection of glass, metal and water |
//!
//! The scalar fields, over the 1,971 Materials of 647 bytes: the **blend** u8 at 21 (0 on 1,747; 4 on 151, 6 on 62, 1 on
//! 10, 3 on one), the **blended** bool at 25 (set on 225: all 62 of blend 6, 148 of the 151 of blend 4), the **alpha
//! test** bool at 26 (223) and its **reference** at 27 (a byte: 128 on 1,718 Materials, 50 of the 223 that test; 0, 131,
//! 99, 58, 20 ... on the others, read as a 0 to 255 reference). The names and textures say what they mean: blended
//! Materials of blend 4 are decals, graffiti, glass and text whose diffuse maps carry alpha (77 of 82 whose map the
//! cell holds); blend 6 are `GFX_*` glows and light paint (`LightPaintBlue`, `Glow_Solarize`, `LaserBeam`) whose maps
//! carry none in 22 of 32: added light; the alpha-tested are leaves, ferns and decals. Of blend 1 and 3 nothing but
//! the names (lights: `Totem*Light`, `ShellLight`, `Gyrophare`) is known. The **colour** is four f32 at 168 (white on
//! 1,017; 0.12 grey on dark glass, a green on the checkpoint glow, a blue on `BlueRusty`): it is the first layer's modulation tint, interpolated by the red vertex/mask factor in the shader. Three floats are read from their values' spread, not from a stated name: the **illumination** at 87 (0 on
//! 1,729 Materials; 0.02 to 50 on 242, among them `ExitSign_GLOW` 6, `Light_InteriorHall` 10, `TelevisionLCD` 1, the
//! skies 0.3: a self-lit factor), the **specular level** at 110 (0 to 1: 1 on 627, 0 on 218) and the **specular power**
//! at 114 (1 to 333: 10 on 703, 1 on 492, 2 on 253, 50 on 151, 100 on 13: round numbers of an exponent). The
//! quad at 332 is **Layer2_ModulationColor**, not specular colour (native property hash 0xc865f2a3); layer 3 is at
//! 434. UseTextureAlpha lives at 224/392/494. Layer maps also store TexCoordRotation and two TexMapMode codes
//! (0 wrap, 1 clamp, 2 border); Mask/Layer2/Layer3_UVChannel are at 164/348/450 (0 or 1).
//! One more colour quad sits at 94 (a reflection tint of the
//! Materials with a `detail` slot); it is not read.
//!
//! Whether a surface is drawn from both sides is stored in two of the Material base class's bit-field bools, which the
//! native property descriptors name by the CRC-32 of their name: **TwoSided** (`0x403525b2`) at 36, bit 20 of the
//! runtime flag word, and **InvertCulling** (`0x4f892276`) at 37, bit 21. The native material update turns them into
//! the D3DRS_CULLMODE the draw sets: TwoSided is `NONE`, else InvertCulling is `CCW` and its absence `CW`. Of the
//! 1,971 distinct Materials of 647 bytes both are 0 or 1; TwoSided is set on 153 (fences, leaves, glass, cloth, light
//! paint, `02_Wall_ConcreteExteriorBlend01_DOUBLESIDED_Mat`) and InvertCulling on two, `GEN_GFX_Bonus1_Mat` and
//! `GEN_GFX_Bonus2_Mat`, which are two-sided too. The byte at 79 (1 on 406, 2 on 37: characters, vehicles,
//! helicopters, light paint) is not read by that update and is not sidedness. What no field here says is how `detail` and `diffuse2` combine.
//! Their slots are kept without assigning those semantics. Restored RGB layering is established separately
//! from the shipped SDB2 HLSL and compiled SM3 pixel shaders.
//!
//! Byte 91 is the water shader switch: an independent inspection of 1,971 fixed-size materials finds it set on
//! exactly six (Port, Pool, Toxic, Ocean, PirateShip, Reservoir). All six have normal/specular/cube-reflection
//! slots and no diffuse; tanks, foam decals, waterfalls and fountain effects leave it clear. It is a stored bool,
//! not a test on a name or on slot absence. Height is the placed mesh's, tint is the colour at 168 and wave-normal
//! tiling is the normal slot's UV transform. No shader-template id or animation-field semantics are established.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use primase_foundations::bytes::{Endian, Field, FieldError, View};

/// Layer tints and alpha switches established by native property hashes and the shipped shader programs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerState {
    /// Layer2_ModulationColor at 332 and Layer3_ModulationColor at 434.
    pub colors: [[f32; 4]; 2],
    /// UseTextureAlpha on diffuse1/2/3 at 224/392/494.
    pub use_alpha: [bool; 3],
}

impl Default for LayerState {
    fn default() -> Self {
        Self {
            colors: [[1.; 4]; 2],
            use_alpha: [true; 3],
        }
    }
}

const LE: Endian = Endian::Little;

/// CRC-32 of the Material class's name as Shaun White Skateboarding stores it.
pub const MATERIAL: u32 = 0xB71F_6261;
/// The body length every Material read has.
pub const BODY_LEN: usize = 647;

/// Why a Material body is not read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Field(#[from] FieldError),
    #[error("material body: class {0:#x}, not Material")]
    NotMaterial(u32),
    #[error("material body is {0} bytes, not {BODY_LEN}")]
    Length(usize),
    #[error("{what}: {value}")]
    Invalid { what: &'static str, value: String },
}

/// A texture slot of a Material: which one, where its id is stored and whether four layer floats follow it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotSpec {
    pub name: &'static str,
    pub at: usize,
    pub layer: bool,
}

/// The texture slots, in the order of the body.
pub const SLOTS: [SlotSpec; 10] = [
    SlotSpec {
        name: "detail",
        at: 118,
        layer: false,
    },
    SlotSpec {
        name: "mask",
        at: 132,
        layer: true,
    },
    SlotSpec {
        name: "diffuse",
        at: 192,
        layer: true,
    },
    SlotSpec {
        name: "normal",
        at: 233,
        layer: true,
    },
    SlotSpec {
        name: "specular",
        at: 277,
        layer: true,
    },
    SlotSpec {
        name: "diffuse_b",
        at: 360,
        layer: true,
    },
    SlotSpec {
        name: "specular_b",
        at: 401,
        layer: true,
    },
    SlotSpec {
        name: "diffuse_c",
        at: 462,
        layer: true,
    },
    SlotSpec {
        name: "diffuse2",
        at: 521,
        layer: false,
    },
    SlotSpec {
        name: "generic",
        at: 537,
        layer: false,
    },
];

/// The slots whose texture specs are read (the others are named by the material, their textures not read).
pub const READ_SLOTS: [&str; 7] = [
    "mask",
    "diffuse",
    "normal",
    "specular",
    "diffuse_b",
    "specular_b",
    "diffuse_c",
];

/// One slot that holds a spec: the TextureMapSpec's id and, for a layer slot, the UV transform it states.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    pub name: &'static str,
    pub spec: u32,
    /// Scale u, scale v; (1, 1) for a slot without layer floats.
    pub scale: [f32; 2],
    /// Offset u, offset v; (0, 0) for a slot without layer floats.
    pub offset: [f32; 2],
    /// Rotation about the UV origin in degrees, before scale and offset (native UV setter).
    pub rotation: f32,
    pub texcoord: u8,
    pub address: Option<[u32; 2]>,
}

/// The fields of a Material (the module documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub id: u32,
    /// The byte at 21.
    pub blend: u8,
    /// The bool at 25.
    pub blended: bool,
    /// The bool at 26.
    pub alpha_test: bool,
    /// The byte at 27.
    pub alpha_ref: u8,
    /// Stored water shader switch at 91.
    pub water: bool,
    /// The bool at 36 (TwoSided): no face is culled.
    pub two_sided: bool,
    /// The bool at 37 (InvertCulling): the culled winding is the other one. Moot where `two_sided` is set.
    pub invert_culling: bool,
    /// RGBA at 168.
    pub color: [f32; 4],
    /// The float at 87.
    pub illumination: f32,
    /// The float at 110.
    pub specular_level: f32,
    /// The float at 114.
    pub specular_power: f32,
    /// The normal slot's `HeightScale` at 265.
    pub normal_scale: f32,
    pub layering: LayerState,
    /// The slots that hold a spec id, in body order.
    pub slots: Vec<Slot>,
}

impl Material {
    /// The slot named `name`, if the Material holds a spec there.
    pub fn slot(&self, name: &str) -> Option<&Slot> {
        self.slots.iter().find(|s| s.name == name)
    }

    /// The Material object whose body is `body`.
    pub fn parse(body: &[u8]) -> Result<Material, Error> {
        let v = View::new(body);
        let class = v.u32(5, LE).field("Material class")?;
        if class != MATERIAL {
            return Err(Error::NotMaterial(class));
        }
        if body.len() != BODY_LEN {
            return Err(Error::Length(body.len()));
        }
        let flag = |at: usize, what: &'static str| -> Result<bool, Error> {
            match v.u8(at).field(what)? {
                0 => Ok(false),
                1 => Ok(true),
                x => Err(Error::Invalid {
                    what,
                    value: x.to_string(),
                }),
            }
        };
        let id = v.u32(1, LE).field("Material id")?;
        let blend = v.u8(21).field("Material blend")?;
        let blended = flag(25, "Material blended")?;
        let alpha_test = flag(26, "Material alpha test")?;
        let alpha_ref = v.u8(27).field("Material alpha reference")?;
        let water = flag(91, "Material water")?;
        let two_sided = flag(36, "Material TwoSided")?;
        let invert_culling = flag(37, "Material InvertCulling")?;
        let mut color = [0f32; 4];
        for (k, c) in color.iter_mut().enumerate() {
            *c = v
                .f32(168usize.saturating_add(k.saturating_mul(4)), LE)
                .field("Material colour")?;
        }
        let mut colors = [[0.; 4]; 2];
        for (layer, rgba) in colors.iter_mut().enumerate() {
            let start = if layer == 0 { 332usize } else { 434usize };
            for (k, c) in rgba.iter_mut().enumerate() {
                *c = v
                    .f32(start.saturating_add(k.saturating_mul(4)), LE)
                    .field("Layer modulation colour")?;
            }
        }
        let layering = LayerState {
            colors,
            use_alpha: [
                flag(224, "diffuse UseTextureAlpha")?,
                flag(392, "diffuse_b UseTextureAlpha")?,
                flag(494, "diffuse_c UseTextureAlpha")?,
            ],
        };
        let mut uv = [0u8; 3];
        for (target, at) in uv.iter_mut().zip([164usize, 348, 450]) {
            let code = v.u32(at, LE).field("layer UVChannel")?;
            *target = match code {
                0 => 0,
                1 => 1,
                _ => {
                    return Err(Error::Invalid {
                        what: "layer UVChannel",
                        value: code.to_string(),
                    });
                }
            };
        }
        let illumination = v.f32(87, LE).field("Material illumination")?;
        let specular_level = v.f32(110, LE).field("Material specular level")?;
        let specular_power = v.f32(114, LE).field("Material specular power")?;
        let normal_scale = v.f32(265, LE).field("normal map HeightScale")?;
        let finite = |xs: &[f32]| xs.iter().all(|x| x.is_finite());
        if !finite(&[illumination, specular_level, specular_power, normal_scale])
            || !colors.iter().all(|c| finite(c))
        {
            return Err(Error::Invalid {
                what: "Material illumination, specular, normal scale and layer colours",
                value: format!("{illumination} {specular_level} {specular_power} {normal_scale} {colors:?}"),
            });
        }
        if !finite(&color) {
            return Err(Error::Invalid {
                what: "Material colour",
                value: format!("{color:?}"),
            });
        }
        let mut slots = Vec::new();
        for s in SLOTS {
            let spec = v.u32(s.at, LE).field(s.name)?;
            if spec == 0 {
                continue;
            }
            let (mut scale, mut offset) = ([1.0, 1.0], [0.0, 0.0]);
            let mut rotation = 0.;
            let mut address = None;
            if s.layer {
                let at = |k: usize| s.at.saturating_add(4).saturating_add(k.saturating_mul(4));
                let f = |k: usize| v.f32(at(k), LE).field(s.name);
                (scale, offset) = ([f(0)?, f(1)?], [f(2)?, f(3)?]);
                rotation = f(4)?;
                let a = [
                    v.u32(s.at.saturating_add(24), LE).field(s.name)?,
                    v.u32(s.at.saturating_add(28), LE).field(s.name)?,
                ];
                if a.iter().any(|c| *c > 2) {
                    return Err(Error::Invalid {
                        what: "TexMapMode",
                        value: format!("{a:?}"),
                    });
                }
                address = Some(a);
                if !finite(&scale) || !finite(&offset) || !rotation.is_finite() {
                    return Err(Error::Invalid {
                        what: s.name,
                        value: format!("scale {scale:?} offset {offset:?}"),
                    });
                }
            }
            slots.push(Slot {
                name: s.name,
                spec,
                scale,
                offset,
                rotation,
                texcoord: match s.name {
                    "mask" => uv.first().copied().unwrap_or(0),
                    "diffuse_b" => uv.get(1).copied().unwrap_or(0),
                    "diffuse_c" => uv.get(2).copied().unwrap_or(0),
                    _ => 0,
                },
                address,
            });
        }
        Ok(Material {
            id,
            blend,
            blended,
            alpha_test,
            alpha_ref,
            water,
            two_sided,
            invert_culling,
            color,
            illumination,
            specular_level,
            specular_power,
            normal_scale,
            layering,
            slots,
        })
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "tests lay a body at fixed offsets"
)]
mod tests {
    use super::*;

    /// A Material body laid by hand: every field the reader reads at its offset, the rest zero.
    fn body() -> Vec<u8> {
        let mut b = vec![0u8; BODY_LEN];
        b[0] = 1;
        b[1..5].copy_from_slice(&0xAAu32.to_le_bytes());
        b[5..9].copy_from_slice(&MATERIAL.to_le_bytes());
        b[21] = 4;
        b[25] = 1;
        b[26] = 1;
        b[27] = 0x80;
        for (k, c) in [0.5f32, 0.25, 0.125, 1.0].iter().enumerate() {
            b[168 + 4 * k..172 + 4 * k].copy_from_slice(&c.to_le_bytes());
        }
        b[87..91].copy_from_slice(&6.0f32.to_le_bytes());
        b[110..114].copy_from_slice(&0.5f32.to_le_bytes());
        b[114..118].copy_from_slice(&10.0f32.to_le_bytes());
        for (k, c) in [1.0f32, 0.75, 0.5, 1.0].iter().enumerate() {
            b[332 + 4 * k..336 + 4 * k].copy_from_slice(&c.to_le_bytes());
        }
        // diffuse: spec 0x11, scale (2, 3), offset (0.5, -1)
        b[192..196].copy_from_slice(&0x11u32.to_le_bytes());
        for (k, f) in [2.0f32, 3.0, 0.5, -1.0].iter().enumerate() {
            b[196 + 4 * k..200 + 4 * k].copy_from_slice(&f.to_le_bytes());
        }
        // the normal slot's HeightScale
        b[265..269].copy_from_slice(&1.5f32.to_le_bytes());
        // generic: a spec id and no layer floats (the floats after it are not tiling)
        b[537..541].copy_from_slice(&0x22u32.to_le_bytes());
        b[541..545].copy_from_slice(&0.45f32.to_le_bytes());
        b
    }

    /// The fields are read from their offsets, a layer slot with its UV transform and a slot without layer floats
    /// at the identity.
    #[test]
    fn fields_and_slots_are_read_from_their_offsets() {
        let m = Material::parse(&body()).unwrap();
        assert_eq!(m.id, 0xAA);
        assert_eq!(
            (m.blend, m.blended, m.alpha_test, m.alpha_ref),
            (4, true, true, 0x80)
        );
        assert_eq!(m.color, [0.5, 0.25, 0.125, 1.0]);
        assert_eq!(
            (m.illumination, m.specular_level, m.specular_power),
            (6.0, 0.5, 10.0)
        );
        assert_eq!(m.layering.colors[0], [1.0, 0.75, 0.5, 1.0]);
        assert_eq!(m.normal_scale, 1.5);
        let names: Vec<_> = m.slots.iter().map(|s| s.name).collect();
        assert_eq!(names, ["diffuse", "generic"]);
        let d = m.slot("diffuse").unwrap();
        assert_eq!((d.spec, d.scale, d.offset), (0x11, [2.0, 3.0], [0.5, -1.0]));
        let g = m.slot("generic").unwrap();
        assert_eq!((g.spec, g.scale, g.offset), (0x22, [1.0, 1.0], [0.0, 0.0]));
        assert!(m.slot("normal").is_none());
    }

    /// Native field hashes identify the second tint, UV channel, authored rotation/address and alpha switch.
    #[test]
    fn layer_inputs_are_retained_and_unknown_codes_are_named() {
        let mut b = body();
        b[360..364].copy_from_slice(&0x33u32.to_le_bytes());
        b[348..352].copy_from_slice(&1u32.to_le_bytes());
        b[380..384].copy_from_slice(&90f32.to_le_bytes());
        b[384..388].copy_from_slice(&1u32.to_le_bytes());
        b[392] = 1;
        let m = Material::parse(&b).unwrap();
        let slot = m.slot("diffuse_b").unwrap();
        assert_eq!(
            (slot.texcoord, slot.rotation, slot.address),
            (1, 90., Some([1, 0]))
        );
        assert!(m.layering.use_alpha[1]);
        assert_eq!(m.layering.colors[0], [1., 0.75, 0.5, 1.]);
        for (at, bad, field) in [
            (348, 2u32.to_le_bytes(), "UVChannel"),
            (380, f32::NAN.to_le_bytes(), "diffuse_b"),
            (384, 3u32.to_le_bytes(), "TexMapMode"),
        ] {
            let mut bad_body = b.clone();
            bad_body[at..at + 4].copy_from_slice(&bad);
            assert!(
                Material::parse(&bad_body)
                    .unwrap_err()
                    .to_string()
                    .contains(field)
            );
        }
        b[392] = 2;
        assert!(
            Material::parse(&b)
                .unwrap_err()
                .to_string()
                .contains("UseTextureAlpha")
        );
    }

    /// TwoSided and InvertCulling are the bools at 36 and 37, each its own; the byte at 79 is neither, and a value other
    /// than 0 or 1 is refused by name.
    #[test]
    fn sidedness_is_the_two_stored_bools_at_36_and_37() {
        let mut b = body();
        let m = Material::parse(&b).unwrap();
        assert!(!m.two_sided && !m.invert_culling);
        b[79] = 2;
        let m = Material::parse(&b).unwrap();
        assert!(!m.two_sided && !m.invert_culling);
        b[36] = 1;
        let m = Material::parse(&b).unwrap();
        assert!(m.two_sided && !m.invert_culling);
        b[36] = 0;
        b[37] = 1;
        let m = Material::parse(&b).unwrap();
        assert!(!m.two_sided && m.invert_culling);
        for (at, field) in [(36, "TwoSided"), (37, "InvertCulling")] {
            let mut bad = body();
            bad[at] = 2;
            assert!(Material::parse(&bad).unwrap_err().to_string().contains(field));
        }
    }

    #[test]
    fn water_is_a_stored_flag_not_a_missing_diffuse_or_a_name() {
        let mut b = body();
        assert!(!Material::parse(&b).unwrap().water);
        b[91] = 1;
        assert!(Material::parse(&b).unwrap().water);
        b[91] = 2;
        assert!(matches!(
            Material::parse(&b),
            Err(Error::Invalid {
                what: "Material water",
                ..
            })
        ));
    }

    /// A body of another length, another class, a flag that is no bool and a non-finite scale are refused, named.
    #[test]
    fn what_the_layout_does_not_hold_is_refused() {
        let ok = body();
        let mut long = ok.clone();
        long.resize(659, 0);
        assert!(matches!(Material::parse(&long), Err(Error::Length(659))));
        let mut b = ok.clone();
        b[5] ^= 1;
        assert!(matches!(Material::parse(&b), Err(Error::NotMaterial(_))));
        let mut b = ok.clone();
        b[25] = 2;
        assert!(matches!(
            Material::parse(&b),
            Err(Error::Invalid {
                what: "Material blended",
                ..
            })
        ));
        let mut b = ok;
        b[196..200].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(matches!(
            Material::parse(&b),
            Err(Error::Invalid { what: "diffuse", .. })
        ));
    }
}
