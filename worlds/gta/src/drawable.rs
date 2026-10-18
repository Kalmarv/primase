//! Drawables (ydr) in Enhanced's layout: bounds, the shader group (shaders with their parameters, an embedded texture
//! dictionary), and up to four LOD model lists whose geometries carry a vertex buffer and an index buffer.
//!
//! Layouts (offsets in bytes):
//! - drawable: resource file header (16), shader group pointer 0x10, skeleton 0x18, bounding centre and radius 0x20,
//!   box min 0x30, box max 0x40, model list pointers high/med/low/vlow 0x50..0x70, LOD distances 0x70, render masks
//!   0x80, joints 0x90, models block 0xA0;
//! - model list: pointer, count, capacity, then that many model pointers;
//! - model (48): geometry pointer array 0x08 with counts 0x10/0x12, bounds 0x18 (one box, or count + 1 boxes with the
//!   whole first), shader mapping 0x20 (one shader index per geometry), skeleton binding 0x28, render mask 0x2C;
//! - geometry (152): vertex buffer 0x18, index buffer 0x38, index and triangle counts 0x58, vertex count 0x60, vertex
//!   stride 0x70;
//! - vertex buffer (64): vertex count 0x08, stride 0x0C, data 0x18, declaration 0x38; the declaration (320) gives per
//!   semantic slot (52: POSITION0-3, NORMAL0-3, TANGENT0-3, BINORMAL0-3, BLENDWEIGHT0-3, BLENDINDICES0-3, COLOR0-3,
//!   TEXCOORD0-23) an offset, the stride, and a buffer format (0: absent); vertices interleave, in offset order;
//! - index buffer (64): index count 0x08, index size 0x0C (2), indices 0x18;
//! - skeleton (112): bones 0x20 (80 each: its tag at 0x44), bone local transforms 0x30 (a 4 x 4 row-vector matrix per
//!   bone, its fourth column unused), parent indices 0x38 (16-bit, -1 for a root), bone count 0x5E;
//! - shader (64): name 0x00, preset 0x04, parameter buffers 0x08, texture references 0x10, parameter infos 0x20, render
//!   bucket 0x39, bucket mask 0x3C; parameter infos: buffer, texture, other, sampler and parameter counts and a copy
//!   count (8 bytes), then per parameter a name and a packed word (kind 2 bits; texture index or cbuffer index; offset
//!   and length in the cbuffer, 12 bits each). The buffer pointers come `copy count` times over; the first copy is read.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::bounds::Bound;
use crate::le;
use crate::resource::Resource;
use crate::sga::BufferFormat;
use crate::texture::{Texture, TextureDictionary};
use crate::{Result, invalid};

fn err(at: u64, detail: impl ToString) -> crate::Error {
    invalid("drawable", at, detail)
}

/// Semantic slot names, by slot.
pub const SEMANTICS: [&str; 52] = [
    "POSITION0",
    "POSITION1",
    "POSITION2",
    "POSITION3",
    "NORMAL0",
    "NORMAL1",
    "NORMAL2",
    "NORMAL3",
    "TANGENT0",
    "TANGENT1",
    "TANGENT2",
    "TANGENT3",
    "BINORMAL0",
    "BINORMAL1",
    "BINORMAL2",
    "BINORMAL3",
    "BLENDWEIGHT0",
    "BLENDWEIGHT1",
    "BLENDWEIGHT2",
    "BLENDWEIGHT3",
    "BLENDINDICES0",
    "BLENDINDICES1",
    "BLENDINDICES2",
    "BLENDINDICES3",
    "COLOR0",
    "COLOR1",
    "COLOR2",
    "COLOR3",
    "TEXCOORD0",
    "TEXCOORD1",
    "TEXCOORD2",
    "TEXCOORD3",
    "TEXCOORD4",
    "TEXCOORD5",
    "TEXCOORD6",
    "TEXCOORD7",
    "TEXCOORD8",
    "TEXCOORD9",
    "TEXCOORD10",
    "TEXCOORD11",
    "TEXCOORD12",
    "TEXCOORD13",
    "TEXCOORD14",
    "TEXCOORD15",
    "TEXCOORD16",
    "TEXCOORD17",
    "TEXCOORD18",
    "TEXCOORD19",
    "TEXCOORD20",
    "TEXCOORD21",
    "TEXCOORD22",
    "TEXCOORD23",
];

/// One vertex element: semantic slot, buffer format, byte offset in the vertex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Element {
    pub slot: u8,
    pub format: BufferFormat,
    pub offset: u32,
}

impl Element {
    pub fn semantic(self) -> &'static str {
        SEMANTICS
            .get(usize::from(self.slot))
            .copied()
            .unwrap_or("an unknown slot")
    }
}

/// A geometry: interleaved vertices, 16-bit triangle-list indices, its shader.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    /// Index into the drawable's shaders.
    pub shader: u16,
    /// Bounds, as stored (`min`, `max`, each with a fourth component).
    pub bounds: [[f32; 4]; 2],
    pub vertex_count: u32,
    pub stride: u16,
    /// Elements in slot order.
    pub elements: Vec<Element>,
    pub vertices: Vec<u8>,
    pub indices: Vec<u16>,
}

impl Geometry {
    /// The bytes of one element across all vertices, vertex after vertex.
    pub fn element_bytes(&self, e: Element) -> Vec<u8> {
        let size = e.format.element_size().expect("checked on reading");
        let (stride, off) = (
            usize::from(self.stride),
            usize::try_from(e.offset).unwrap_or(usize::MAX),
        );
        if stride == 0 {
            return Vec::new();
        }
        // each vertex holds the element (checked on reading)
        self.vertices
            .chunks_exact(stride)
            .filter_map(|v| v.get(off..off.checked_add(size)?))
            .flatten()
            .copied()
            .collect()
    }
}

/// A model: geometries drawn with one bone binding and render mask.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub skeleton_binding: u32,
    pub render_mask: u16,
    pub geometries: Vec<Geometry>,
}

/// A shader parameter's value.
#[derive(Clone, Debug, PartialEq)]
pub enum ParamValue {
    /// A texture by name (`None`: no texture bound).
    Texture(Option<String>),
    /// Floats from a constant buffer (the parameter's length / 4).
    Floats(Vec<f32>),
    /// A sampler index.
    Sampler(u8),
    /// Kind 1, index as stored.
    Other(u8),
}

/// A shader parameter: its stored name hash, its packed info word as stored (kind in bits 0-1; texture, sampler or
/// other index in bits 2-9, or constant buffer index in bits 2-7 with byte offset in bits 8-19 and length in bits
/// 20-31), and its value.
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: u32,
    pub info: u32,
    pub value: ParamValue,
}

/// A shader: effect name, preset, render bucket and parameters in stored order.
#[derive(Clone, Debug, PartialEq)]
pub struct Shader {
    pub name: u32,
    /// Legacy's preset (`.sps`) name hash; Enhanced stores 0x6d657461 here for every shader (census), so a shader's
    /// class comes from its effect name and render bucket.
    pub preset: u32,
    pub render_bucket: u8,
    pub bucket_mask: u32,
    pub params: Vec<Param>,
}

/// A drawable.
#[derive(Clone, Debug, PartialEq)]
pub struct Drawable {
    pub bounding_centre: [f32; 3],
    pub bounding_radius: f32,
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    /// High, medium, low and very low LOD distances.
    pub lod_dist: [f32; 4],
    pub render_masks: [u32; 4],
    /// Models per LOD: high, medium, low, very low.
    pub lods: [Vec<Model>; 4],
    pub shaders: Vec<Shader>,
    /// The shader group's embedded texture dictionary.
    pub textures: Option<TextureDictionary>,
    /// Its skeleton's bones, when it has one.
    pub skeleton: Option<Skeleton>,
    /// Its light sources, in model space (a ydr's or ydd entry's own; a fragment's drawables carry none, the fragment
    /// does).
    pub lights: Vec<LightAttr>,
    /// The collision bound it embeds, in model space: a gtaDrawable's (ydr root, ydd entry) at 0xC8, a fragment's
    /// main drawable's at 0xF0.
    pub bound: Option<Bound>,
}

/// One light source as a drawable or fragment stores it (`CLightAttr`, 168 bytes), every field as stored. Positions
/// and directions are in the space of the bone whose tag is `bone_id` (the model's, when no bone has it); colours are
/// sRGB bytes.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct LightAttr {
    pub position: [f32; 3],
    pub colour: [u8; 3],
    pub flashiness: u8,
    pub intensity: f32,
    pub flags: u32,
    pub bone_id: u16,
    /// 1 point, 2 spot, 4 capsule.
    pub kind: u8,
    pub group_id: u8,
    /// The hours it is on: bit h for hour h (the low 24 bits), higher bits as stored.
    pub time_flags: u32,
    pub falloff: f32,
    pub falloff_exponent: f32,
    /// Culling plane normal and offset.
    pub culling_plane: [f32; 4],
    pub shadow_blur: u8,
    pub volume_intensity: f32,
    pub volume_size_scale: f32,
    pub volume_outer_colour: [u8; 3],
    pub light_hash: u8,
    pub volume_outer_intensity: f32,
    pub corona_size: f32,
    pub volume_outer_exponent: f32,
    /// Light, shadow, specular and volumetric fade distances.
    pub fade_distances: [u8; 4],
    pub shadow_near_clip: f32,
    pub corona_intensity: f32,
    pub corona_z_bias: f32,
    pub direction: [f32; 3],
    pub tangent: [f32; 3],
    pub cone_inner_angle: f32,
    pub cone_outer_angle: f32,
    /// A capsule's extent.
    pub extent: [f32; 3],
    pub projected_texture_hash: u32,
    /// The words community tools know as always zero (0x00, 0x04, 0x14, 0x48, 0xA4) and the bytes at 0x45..0x48,
    /// kept to see whether they ever are not.
    pub reserved: [u32; 6],
}

impl LightAttr {
    /// The record at the start of `d` (168 bytes).
    fn read(d: &[u8]) -> Result<LightAttr> {
        let b = |o: usize| le::u8(d, o);
        Ok(LightAttr {
            position: le::vec3(d, 0x08)?,
            colour: [b(0x18)?, b(0x19)?, b(0x1A)?],
            flashiness: b(0x1B)?,
            intensity: le::f32(d, 0x1C)?,
            flags: le::u32(d, 0x20)?,
            bone_id: le::u16(d, 0x24)?,
            kind: b(0x26)?,
            group_id: b(0x27)?,
            time_flags: le::u32(d, 0x28)?,
            falloff: le::f32(d, 0x2C)?,
            falloff_exponent: le::f32(d, 0x30)?,
            culling_plane: le::vec4(d, 0x34)?,
            shadow_blur: b(0x44)?,
            volume_intensity: le::f32(d, 0x4C)?,
            volume_size_scale: le::f32(d, 0x50)?,
            volume_outer_colour: [b(0x54)?, b(0x55)?, b(0x56)?],
            light_hash: b(0x57)?,
            volume_outer_intensity: le::f32(d, 0x58)?,
            corona_size: le::f32(d, 0x5C)?,
            volume_outer_exponent: le::f32(d, 0x60)?,
            fade_distances: [b(0x64)?, b(0x65)?, b(0x66)?, b(0x67)?],
            shadow_near_clip: le::f32(d, 0x68)?,
            corona_intensity: le::f32(d, 0x6C)?,
            corona_z_bias: le::f32(d, 0x70)?,
            direction: le::vec3(d, 0x74)?,
            tangent: le::vec3(d, 0x80)?,
            cone_inner_angle: le::f32(d, 0x8C)?,
            cone_outer_angle: le::f32(d, 0x90)?,
            extent: le::vec3(d, 0x94)?,
            projected_texture_hash: le::u32(d, 0xA0)?,
            reserved: [
                le::u32(d, 0x00)?,
                le::u32(d, 0x04)?,
                le::u32(d, 0x14)?,
                u32::from(b(0x45)?) | u32::from(le::u16(d, 0x46)?) << 8,
                le::u32(d, 0x48)?,
                le::u32(d, 0xA4)?,
            ],
        })
    }
}

/// A drawable's bones as stored: its skeleton (each bone in its parent's space, with its parent and tag) and, for a
/// fragment, the fragment's own bone pose (by bone index, into the model's space).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Bones {
    pub skeleton: Option<Skeleton>,
    /// A fragment's bone pose (the list at 0xA8 of its header); empty for a drawable or a dictionary entry.
    pub fragment_pose: Vec<[[f32; 4]; 3]>,
}

impl Bones {
    /// Neither a skeleton nor a fragment pose.
    pub fn is_empty(&self) -> bool {
        self.skeleton.is_none() && self.fragment_pose.is_empty()
    }

    /// Each bone's transform into the model's space, by bone index: the fragment's pose of it where it has one, else
    /// the skeleton's rest pose composed up to the root ([`Skeleton::pose`]).
    pub fn pose(&self) -> Vec<[[f32; 4]; 3]> {
        let rest = self.skeleton.as_ref().map(Skeleton::pose).unwrap_or_default();
        (0..rest.len().max(self.fragment_pose.len()))
            .filter_map(|b| self.fragment_pose.get(b).or(rest.get(b)).copied())
            .collect()
    }

    /// The index of the bone tagged `tag` (the skeleton's).
    pub fn bone_of_tag(&self, tag: u16) -> Option<usize> {
        self.skeleton.as_ref()?.bone_of_tag(tag)
    }
}

/// Light records with their bones, read by `bones` only when there are records.
fn on_bones(
    lights: Vec<LightAttr>,
    bones: impl FnOnce() -> Result<Bones>,
) -> Result<(Vec<LightAttr>, Bones)> {
    if lights.is_empty() {
        return Ok((lights, Bones::default()));
    }
    Ok((lights, bones()?))
}

/// The skeleton of the drawable at `pointer`, if it has one.
fn skeleton_at(r: &Resource, pointer: u64) -> Result<Option<Skeleton>> {
    match le::u64(r.at(pointer, 0x20)?, 0x18)? {
        0 => Ok(None),
        p => Skeleton::read(r, p).map(Some),
    }
}

/// A fragment's bone pose (its header at 0x5000_0000: the list at 0xA8), by bone index.
fn fragment_pose(r: &Resource, h: &[u8]) -> Result<Vec<[[f32; 4]; 3]>> {
    Ok(match le::u64(h, 0xA8)? {
        0 => Vec::new(),
        p => {
            let n = usize::from(r.block::<0x20>(p)?[0x10]);
            let bones = r.records(offset(p, 0x20)?, n, 48)?;
            bones
                .as_chunks::<48>()
                .0
                .iter()
                .map(|b| rows::<3, 48>(b, p))
                .collect::<Result<_>>()?
        }
    })
}

/// The light records of a resource's root without reading its drawables' geometry, with the bones they are on as
/// stored ([`Bones`]): a ydr's (list at 0xB0 of its gtaDrawable), a yft's (the fragment's list at 0x110, on its main
/// drawable's skeleton and the fragment's pose), or a ydd entry's (`entry`: the name hash), after checking the
/// resource's layout version. `kind`: `ydr`, `yft` or `ydd`. A resource without records has no bones read.
pub fn resource_lights(r: &Resource, kind: &str, entry: u32) -> Result<(Vec<LightAttr>, Bones)> {
    let want = if kind == "yft" { FRAGMENT_VERSION } else { VERSION };
    if r.version != want {
        return Err(err(
            0,
            format!("{kind}: resource version {}, Enhanced's is {want}", r.version),
        ));
    }
    match kind {
        "ydr" => on_bones(read_lights(r, 0x5000_00B0)?, || {
            Ok(Bones {
                skeleton: skeleton_at(r, 0x5000_0000)?,
                fragment_pose: Vec::new(),
            })
        }),
        "yft" => {
            let h = r.at(0x5000_0000, 0x130)?;
            on_bones(read_lights(r, 0x5000_0110)?, || {
                Ok(Bones {
                    skeleton: match le::u64(h, 0x30)? {
                        0 => None,
                        d => skeleton_at(r, d)?,
                    },
                    fragment_pose: fragment_pose(r, h)?,
                })
            })
        }
        "ydd" => {
            let hashes = pointer_list_u32(r, 0x5000_0020)?;
            let ptrs = pointer_list(r, 0x5000_0030)?;
            if hashes.len() != ptrs.len() {
                return Err(err(
                    0,
                    format!("{} name hashes for {} drawables", hashes.len(), ptrs.len()),
                ));
            }
            match hashes.iter().zip(&ptrs).find(|&(&h, _)| h == entry) {
                Some((_, &p)) => on_bones(read_lights(r, offset(p, 0xB0)?)?, || {
                    Ok(Bones {
                        skeleton: skeleton_at(r, p)?,
                        fragment_pose: Vec::new(),
                    })
                }),
                None => Ok((Vec::new(), Bones::default())),
            }
        }
        _ => Err(err(0, format!("lights of a {kind}"))),
    }
}

/// The light records of a list (pointer, count, capacity) at `at`.
fn read_lights(r: &Resource, at: u64) -> Result<Vec<LightAttr>> {
    let h = r.at(at, 16)?;
    let (count, cap) = (le::u16(h, 8)?, le::u16(h, 10)?);
    if cap < count {
        return Err(err(at, format!("light list count {count} over capacity {cap}")));
    }
    let n = usize::from(count);
    if n == 0 {
        return Ok(Vec::new());
    }
    let d = r.records(le::u64(h, 0)?, n, 168)?;
    d.chunks(168).map(LightAttr::read).collect()
}

/// A skeleton's bones: each one's transform in its parent's space, its parent and its tag.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Skeleton {
    /// Per bone, three rows `[x, y, z, translation]` of its transform into its parent's space.
    pub local: Vec<[[f32; 4]; 3]>,
    /// Per bone, its parent's index (`None` for a root).
    pub parents: Vec<Option<u16>>,
    /// Per bone, its tag: the id a light record names its bone by (`None` when the skeleton stores no bones).
    pub tags: Option<Vec<u16>>,
}

impl Skeleton {
    fn read(r: &Resource, pointer: u64) -> Result<Skeleton> {
        let s = r.at(pointer, 0x70)?;
        let count = usize::from(le::u16(s, 0x5E)?);
        let transforms = match le::u64(s, 0x30)? {
            0 => return Err(err(pointer, "a skeleton without bone transforms")),
            p => r.records(p, count, 64)?,
        };
        let parents = match le::u64(s, 0x38)? {
            0 => vec![-1; count],
            p => r
                .records(p, count, 2)?
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes(*b))
                .collect(),
        };
        let tags = match le::u64(s, 0x20)? {
            0 => None,
            p => Some(
                r.records(p, count, 80)?
                    .as_chunks::<80>()
                    .0
                    .iter()
                    .map(|b| le::u16(b, 0x44))
                    .collect::<Result<Vec<_>>>()?,
            ),
        };
        let local = transforms
            .as_chunks::<64>()
            .0
            .iter()
            .map(|m| {
                // row-vector matrix M (p' = p M, translation in its fourth row): row i of ours is column i of M
                let [r0, r1, r2, r3] = rows::<4, 64>(m, pointer)?;
                Ok([
                    [r0[0], r1[0], r2[0], r3[0]],
                    [r0[1], r1[1], r2[1], r3[1]],
                    [r0[2], r1[2], r2[2], r3[2]],
                ])
            })
            .collect::<Result<_>>()?;
        let parents = parents
            .iter()
            .map(|&p| match p {
                -1 => Ok(None),
                p => u16::try_from(p)
                    .ok()
                    .filter(|&q| usize::from(q) < count)
                    .map(Some)
                    .ok_or_else(|| err(pointer, format!("bone parent {p} of {count} bones"))),
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Skeleton { local, parents, tags })
    }

    /// The index of the bone tagged `tag` (the first, should two share it).
    pub fn bone_of_tag(&self, tag: u16) -> Option<usize> {
        self.tags.as_ref()?.iter().position(|&t| t == tag)
    }

    /// Each bone's transform into the drawable's space: its own, then its parents' up to the root.
    pub fn pose(&self) -> Vec<[[f32; 4]; 3]> {
        let apply = |a: &[[f32; 4]; 3], b: &[[f32; 4]; 3]| -> [[f32; 4]; 3] {
            // a after b: rows of a times the 4 x 4 of b (its fourth row (0, 0, 0, 1))
            let [b0, b1, b2] = *b;
            a.map(|[x, y, z, w]| {
                let mut row = [0.0f32; 4];
                for (((r, p), q), s) in row.iter_mut().zip(b0).zip(b1).zip(b2) {
                    *r = x * p + y * q + z * s;
                }
                row[3] += w;
                row
            })
        };
        self.local
            .iter()
            .zip(&self.parents)
            .map(|(&own, &parent)| {
                let (mut t, mut p) = (own, parent);
                let mut guard = 0usize;
                while let Some(q) = p
                    && guard < self.local.len()
                    && let (Some(l), Some(&up)) =
                        (self.local.get(usize::from(q)), self.parents.get(usize::from(q)))
                {
                    t = apply(l, &t);
                    p = up;
                    guard = guard.saturating_add(1);
                }
                t
            })
            .collect()
    }
}

/// The resource version of Enhanced's drawables (ydr).
pub const VERSION: u32 = 159;

impl Drawable {
    /// A ydr's drawable: its root, after checking the resource is in Enhanced's layout.
    pub fn read_resource(r: &Resource) -> Result<Drawable> {
        if r.version != VERSION {
            return Err(err(
                0,
                format!(
                    "resource version {}, Enhanced's ydr layout is {VERSION}",
                    r.version
                ),
            ));
        }
        Self::read_gta(r, 0x5000_0000)
    }

    /// The drawable at `pointer` (`0x5000_0000` for a ydr's root).
    pub fn read(r: &Resource, pointer: u64) -> Result<Drawable> {
        let d = r.at(pointer, 0xA8)?;
        let mut lods: [Vec<Model>; 4] = Default::default();
        for (lod, at) in lods.iter_mut().zip([0x50, 0x58, 0x60, 0x68]) {
            let p = le::u64(d, at)?;
            if p != 0 {
                *lod = read_models(r, p)?;
            }
        }
        let group = le::u64(d, 0x10)?;
        let (shaders, textures) = read_shader_group(r, group)?;
        // a drawable without a shader group (a fragment's extra drawables) uses its owner's shaders
        if group != 0 {
            check_shaders(pointer, &lods, shaders.len())?;
        }
        Ok(Drawable {
            bounding_centre: le::vec3(d, 0x20)?,
            bounding_radius: le::f32(d, 0x2C)?,
            bbox_min: le::vec3(d, 0x30)?,
            bbox_max: le::vec3(d, 0x40)?,
            lod_dist: [
                le::f32(d, 0x70)?,
                le::f32(d, 0x74)?,
                le::f32(d, 0x78)?,
                le::f32(d, 0x7C)?,
            ],
            render_masks: [
                le::u32(d, 0x80)?,
                le::u32(d, 0x84)?,
                le::u32(d, 0x88)?,
                le::u32(d, 0x8C)?,
            ],
            lods,
            shaders,
            textures,
            skeleton: match le::u64(d, 0x18)? {
                0 => None,
                p => Some(Skeleton::read(r, p)?),
            },
            lights: Vec::new(),
            bound: None,
        })
    }

    /// A gtaDrawable (a ydr's root, a ydd's entry): the drawable, its light list at 0xB0 and its bound at 0xC8.
    fn read_gta(r: &Resource, pointer: u64) -> Result<Drawable> {
        let mut d = Self::read(r, pointer)?;
        d.lights = read_lights(r, offset(pointer, 0xB0)?)?;
        d.bound = read_bound(r, le::u64(r.at(pointer, 0xD0)?, 0xC8)?)?;
        Ok(d)
    }
}

/// Where a constant buffer parameter's `length` bytes start: `offset` past its buffer's pointer `start`, inside the
/// buffer when the next buffer's pointer `next` lies after it (the buffers' order is not assumed: a next buffer
/// stored before this one bounds nothing; the resource bounds every read). Addresses that do not fit are refused.
fn parameter_at(
    start: u64,
    next: Option<u64>,
    offset: u64,
    length: usize,
) -> std::result::Result<u64, String> {
    let at = start
        .checked_add(offset)
        .ok_or("parameter address past the address space")?;
    let end = at
        .checked_add(length as u64)
        .ok_or("parameter end past the address space")?;
    if let Some(next) = next
        && next > start
        && end > next
    {
        return Err(format!(
            "parameter at {offset:#x} + {length} runs past its constant buffer ({:#x} bytes)",
            next.abs_diff(start)
        ));
    }
    Ok(at)
}

/// `o` bytes past resource pointer `p`.
fn offset(p: u64, o: u64) -> Result<u64> {
    p.checked_add(o)
        .ok_or_else(|| err(p, format!("{o:#x} past the address space")))
}

/// The `R` rows of four f32 that a record of `B` bytes holds (`B` = 16 `R`).
fn rows<const R: usize, const B: usize>(m: &[u8; B], at: u64) -> Result<[[f32; 4]; R]> {
    let rows = <&[[u8; 16]; R]>::try_from(m.as_chunks::<16>().0)
        .map_err(|_| err(at, format!("a {B}-byte record is not {R} rows")))?;
    Ok(rows.map(|r| le::vec4_of(&r)))
}

/// The bound at `p`, if any (`0`: none).
fn read_bound(r: &Resource, p: u64) -> Result<Option<Bound>> {
    match p {
        0 => Ok(None),
        p => crate::bounds::read(r, p, 0).map(Some),
    }
}

/// Every geometry's shader index is below `shaders`.
fn check_shaders(at: u64, lods: &[Vec<Model>; 4], shaders: usize) -> Result<()> {
    match lods
        .iter()
        .flatten()
        .flat_map(|m| &m.geometries)
        .find(|g| usize::from(g.shader) >= shaders)
    {
        Some(g) => Err(err(at, format!("geometry shader {} of {shaders}", g.shader))),
        None => Ok(()),
    }
}

/// A pointer list: pointer, count, capacity; the `count` pointers it holds.
fn pointer_list(r: &Resource, at: u64) -> Result<Vec<u64>> {
    let h = r.at(at, 16)?;
    let (count, cap) = (le::u16(h, 8)?, le::u16(h, 10)?);
    if cap < count {
        return Err(err(at, format!("list count {count} over capacity {cap}")));
    }
    let n = usize::from(count);
    if n == 0 {
        return Ok(Vec::new());
    }
    let p = r.records(le::u64(h, 0)?, n, 8)?;
    Ok(p.as_chunks::<8>()
        .0
        .iter()
        .map(|c| u64::from_le_bytes(*c))
        .collect())
}

fn read_models(r: &Resource, list: u64) -> Result<Vec<Model>> {
    pointer_list(r, list)?
        .into_iter()
        .map(|p| {
            let m = r.at(p, 0x30)?;
            let n = usize::from(le::u16(m, 0x10)?);
            if le::u16(m, 0x12)? != le::u16(m, 0x10)? || le::u16(m, 0x2E)? != le::u16(m, 0x10)? {
                return Err(err(p, "model geometry counts differ"));
            }
            let geoms = if n == 0 {
                &[][..]
            } else {
                r.records(le::u64(m, 0x08)?, n, 8)?
            };
            let mapping = if n == 0 {
                &[][..]
            } else {
                r.records(le::u64(m, 0x20)?, n, 2)?
            };
            // one box for one geometry; for more, the whole model's box first, then one per geometry
            let first = usize::from(n > 1);
            let bounds = if n == 0 {
                &[][..]
            } else {
                r.records(le::u64(m, 0x18)?, n.saturating_add(first), 32)?
            };
            let geometries = geoms
                .as_chunks::<8>()
                .0
                .iter()
                .zip(mapping.as_chunks::<2>().0)
                .zip(bounds.as_chunks::<32>().0.iter().skip(first))
                .map(|((g, s), b)| {
                    read_geometry(
                        r,
                        u64::from_le_bytes(*g),
                        u16::from_le_bytes(*s),
                        [le::vec4(b, 0)?, le::vec4(b, 16)?],
                    )
                })
                .collect::<Result<_>>()?;
            Ok(Model {
                skeleton_binding: le::u32(m, 0x28)?,
                render_mask: le::u16(m, 0x2C)?,
                geometries,
            })
        })
        .collect()
}

fn read_geometry(r: &Resource, p: u64, shader: u16, bounds: [[f32; 4]; 2]) -> Result<Geometry> {
    let g = r.at(p, 0x98)?;
    let index_count = le::u32(g, 0x58)?;
    if u64::from(le::u32(g, 0x5C)?).checked_mul(3) != Some(u64::from(index_count)) || le::u16(g, 0x62)? != 3 {
        return Err(err(
            p,
            format!("{index_count} indices, {} triangles", le::u32(g, 0x5C)?),
        ));
    }
    // vertex buffer
    let vb_ptr = le::u64(g, 0x18)?;
    let vb = r.at(vb_ptr, 0x40)?;
    let vertex_count = le::u32(vb, 0x08)?;
    let stride = le::u16(vb, 0x0C)?;
    if u32::from(le::u16(g, 0x60)?) != vertex_count & 0xFFFF || le::u16(g, 0x70)? != stride {
        return Err(err(
            p,
            "geometry and vertex buffer disagree on vertex count or stride",
        ));
    }
    let decl_ptr = le::u64(vb, 0x38)?;
    let decl = r.at(decl_ptr, 320)?;
    // per slot: its offset (52 words), its stride (52 bytes), its format (52 bytes)
    let table = |at: usize, len: usize| {
        decl.get(at..at.saturating_add(len))
            .ok_or_else(|| err(decl_ptr, "vertex declaration shorter than its tables"))
    };
    let (offsets, strides, formats) = (table(0, 208)?, table(208, 52)?, table(260, 52)?);
    let mut elements = Vec::new();
    for ((((slot, offset), &slot_stride), &format), semantic) in (0u8..)
        .zip(offsets.as_chunks::<4>().0)
        .zip(strides)
        .zip(formats)
        .zip(SEMANTICS)
    {
        let format = BufferFormat(format);
        if format.0 == 0 {
            continue;
        }
        let offset = u32::from_le_bytes(*offset);
        let size = format
            .element_size()
            .ok_or_else(|| err(decl_ptr, format!("{semantic}: vertex format {:#x}", format.0)))?;
        let fits = usize::try_from(offset)
            .ok()
            .and_then(|o| o.checked_add(size))
            .is_some_and(|end| end <= usize::from(stride));
        if u16::from(slot_stride) != stride || !fits {
            return Err(err(
                decl_ptr,
                format!("{semantic} at {offset} in a {stride}-byte vertex"),
            ));
        }
        elements.push(Element { slot, format, offset });
    }
    let packed = le::u64(decl, 312)?;
    if (packed >> 2) & 0xFF != u64::from(stride) || packed & 1 != 0 {
        return Err(err(
            decl_ptr,
            format!("declaration word {packed:#x} for a {stride}-byte vertex"),
        ));
    }
    let n_vertices = usize::try_from(vertex_count).map_err(|_| err(vb_ptr, "vertex count"))?;
    let vertices = r
        .records(le::u64(vb, 0x18)?, n_vertices, usize::from(stride))?
        .to_vec();
    // index buffer
    let ib_ptr = le::u64(g, 0x38)?;
    let ib = r.at(ib_ptr, 0x40)?;
    if le::u32(ib, 0x08)? != index_count || le::u16(ib, 0x0C)? != 2 {
        return Err(err(
            ib_ptr,
            format!(
                "index buffer of {} indices of {} bytes",
                le::u32(ib, 8)?,
                le::u16(ib, 0x0C)?
            ),
        ));
    }
    let raw = r.at(le::u64(ib, 0x18)?, 2 * index_count as usize)?;
    let indices: Vec<u16> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    if indices.iter().any(|&i| u32::from(i) >= vertex_count) {
        return Err(err(ib_ptr, "an index past the vertices"));
    }
    Ok(Geometry {
        shader,
        bounds,
        vertex_count,
        stride,
        elements,
        vertices,
        indices,
    })
}

fn read_shader_group(r: &Resource, p: u64) -> Result<(Vec<Shader>, Option<TextureDictionary>)> {
    if p == 0 {
        return Ok((Vec::new(), None));
    }
    let g = r.at(p, 0x40)?;
    let textures = match le::u64(g, 0x08)? {
        0 => None,
        t => Some(TextureDictionary::read(r, t)?),
    };
    let n = usize::from(le::u16(g, 0x18)?);
    if le::u16(g, 0x1A)? < le::u16(g, 0x18)? {
        return Err(err(p, "shader count over capacity"));
    }
    let ptrs = if n == 0 {
        &[][..]
    } else {
        r.records(le::u64(g, 0x10)?, n, 8)?
    };
    let shaders = ptrs
        .as_chunks::<8>()
        .0
        .iter()
        .map(|p| read_shader(r, u64::from_le_bytes(*p)))
        .collect::<Result<_>>()?;
    Ok((shaders, textures))
}

fn read_shader(r: &Resource, p: u64) -> Result<Shader> {
    let s = r.block::<0x40>(p)?;
    let infos_ptr = le::u64(s, 0x20)?;
    let h = r.block::<8>(infos_ptr)?;
    let (n_buffers, n_textures, n_other, _n_samplers, n_params, copies) = (
        usize::from(h[0]),
        usize::from(h[1]),
        usize::from(h[2]),
        h[3],
        usize::from(h[4]),
        usize::from(h[7]),
    );
    if h[5] != 0 || h[6] != 0 || copies == 0 {
        return Err(err(infos_ptr, format!("parameter info header {:02x?}", h)));
    }
    let infos = r.records(offset(infos_ptr, 8)?, n_params, 8)?;
    // constant buffers: the first copy's pointers (the next copy's first pointer ends the last buffer)
    let buffers: Vec<u64> = if n_buffers == 0 {
        Vec::new()
    } else {
        let n = n_buffers.saturating_add(1).min(n_buffers.saturating_mul(copies));
        let b = r.records(le::u64(s, 0x08)?, n, 8)?;
        b.as_chunks::<8>()
            .0
            .iter()
            .map(|c| u64::from_le_bytes(*c))
            .collect()
    };
    let texture_refs: Vec<u64> = if n_textures == 0 {
        Vec::new()
    } else {
        let t = r.records(le::u64(s, 0x10)?, n_textures, 8)?;
        t.as_chunks::<8>()
            .0
            .iter()
            .map(|c| u64::from_le_bytes(*c))
            .collect()
    };
    let mut params = Vec::with_capacity(n_params);
    for info in infos.as_chunks::<8>().0 {
        let [name, word] = le::u32x2_of(info);
        // the packed word: kind (2 bits), then an 8-bit index (a 6-bit cbuffer index), a 12-bit offset and length
        let [index8, ..] = (word >> 2).to_le_bytes();
        let [o0, o1, ..] = (word >> 8).to_le_bytes();
        let [l0, l1, ..] = (word >> 20).to_le_bytes();
        let value = match word & 3 {
            0 => {
                let index = usize::from(index8);
                let ptr = *texture_refs
                    .get(index)
                    .ok_or_else(|| err(infos_ptr, format!("texture index {index} of {n_textures}")))?;
                ParamValue::Texture(match ptr {
                    0 => None,
                    t => Some(Texture::read_base(r, t)?.0.name),
                })
            }
            1 => {
                let index = index8;
                if usize::from(index) >= n_other {
                    return Err(err(infos_ptr, format!("other index {index} of {n_other}")));
                }
                ParamValue::Other(index)
            }
            2 => ParamValue::Sampler(index8),
            _ => {
                let cb = usize::from(index8 & 0x3F);
                let (offset, length) = (
                    u64::from(u16::from_le_bytes([o0, o1]) & 0xFFF),
                    usize::from(u16::from_le_bytes([l0, l1]) & 0xFFF),
                );
                if cb >= n_buffers || length & 3 != 0 {
                    return Err(err(
                        infos_ptr,
                        format!("cbuffer {cb} of {n_buffers}, length {length}"),
                    ));
                }
                let start = *buffers
                    .get(cb)
                    .ok_or_else(|| err(infos_ptr, format!("cbuffer {cb} of {}", buffers.len())))?;
                let next = cb.checked_add(1).and_then(|c| buffers.get(c)).copied();
                let at = parameter_at(start, next, offset, length).map_err(|e| err(infos_ptr, e))?;
                let v = r.at(at, length)?;
                ParamValue::Floats(
                    v.as_chunks::<4>()
                        .0
                        .iter()
                        .map(|c| f32::from_le_bytes(*c))
                        .collect(),
                )
            }
        };
        params.push(Param {
            name,
            info: word,
            value,
        });
    }
    Ok(Shader {
        name: le::u32(s, 0x00)?,
        preset: le::u32(s, 0x04)?,
        render_bucket: s[0x39],
        bucket_mask: le::u32(s, 0x3C)?,
        params,
    })
}

/// The resource version of Enhanced's fragments (yft).
pub const FRAGMENT_VERSION: u32 = 171;

/// A fragment (yft): its name, main drawable (the one the world draws), its extra drawables (by name; those without a
/// shader group use the main drawable's shaders), its cloth drawable and the bound of its first physics LOD. The rest
/// of its physics (groups, children's masses and joints), glass and bones are not read yet.
#[derive(Clone, Debug, PartialEq)]
pub struct Fragment {
    pub name: String,
    pub drawable: Option<Drawable>,
    pub extra: Vec<(String, Drawable)>,
    pub cloth: Option<Drawable>,
    /// The bones' pose, by bone index: three rows `[x, y, z, translation]` (`world = row . (p, 1)`).
    pub pose: Vec<[[f32; 4]; 3]>,
    /// The fragment's light sources (list at 0x110), in the space of their bones.
    pub lights: Vec<LightAttr>,
    /// The bound of its first physics LOD, in the fragment's space: what the game collides the fragment with.
    pub physics: Option<Bound>,
}

impl Fragment {
    /// A yft's fragment, after checking the resource is in Enhanced's layout. Layout: resource file header (16),
    /// bounding sphere 0x20, main drawable 0x30, extra drawables 0x38 with their names 0x40 and count 0x48, name
    /// 0x58, bone pose 0xA8 (32-byte header with the count at 0x10, then 48 bytes per bone), physics LOD group 0xF0,
    /// cloth drawable 0xF8, light list 0x110. The main drawable's bound is at 0xF0 of it; the physics LOD group holds
    /// three LOD pointers from 0x10, a physics LOD its bound at 0xE8.
    pub fn read_resource(r: &Resource) -> Result<Fragment> {
        if r.version != FRAGMENT_VERSION {
            return Err(err(
                0,
                format!(
                    "resource version {}, Enhanced's yft layout is {FRAGMENT_VERSION}",
                    r.version
                ),
            ));
        }
        let h = r.block::<0x130>(0x5000_0000)?;
        let opt = |p: u64| -> Result<Option<Drawable>> {
            match p {
                0 => Ok(None),
                p => Drawable::read(r, p).map(Some),
            }
        };
        let n = usize::try_from(le::u32(h, 0x48)?).map_err(|_| err(0x48, "drawable count"))?;
        let mut extra = Vec::new();
        if n > 0 {
            let ptrs = r.records(le::u64(h, 0x38)?, n, 8)?.as_chunks::<8>().0;
            let names = r.records(le::u64(h, 0x40)?, n, 8)?.as_chunks::<8>().0;
            for (p, name) in ptrs.iter().zip(names) {
                let name = match u64::from_le_bytes(*name) {
                    0 => String::new(),
                    p => crate::texture::c_string(r, p)?,
                };
                if let Some(d) = opt(u64::from_le_bytes(*p))? {
                    extra.push((name, d));
                }
            }
        }
        // extra and cloth drawables without their own shaders use the main drawable's
        let owner = le::u64(h, 0x30)?;
        let mut drawable = opt(owner)?;
        if let Some(d) = &mut drawable {
            d.bound = read_bound(r, le::u64(r.at(owner, 0xF8)?, 0xF0)?)?;
        }
        let main_shaders = drawable.as_ref().map_or(0, |d| d.shaders.len());
        for (_, d) in &extra {
            if d.shaders.is_empty() {
                check_shaders(owner, &d.lods, main_shaders)?;
            }
        }
        let pose = fragment_pose(r, h)?;
        let physics = match le::u64(h, 0xF0)? {
            0 => None,
            group => match le::u64(r.at(group, 0x18)?, 0x10)? {
                0 => None,
                lod => read_bound(r, le::u64(r.at(lod, 0xF0)?, 0xE8)?)?,
            },
        };
        Ok(Fragment {
            pose,
            name: match le::u64(h, 0x58)? {
                0 => String::new(),
                p => crate::texture::c_string(r, p)?,
            },
            drawable,
            extra,
            cloth: opt(le::u64(h, 0xF8)?)?,
            lights: read_lights(r, 0x5000_0110)?,
            physics,
        })
    }
}

/// A drawable dictionary (ydd): drawables sorted by name hash.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawableDictionary {
    /// Name hashes as stored (the joaat of the lowercase name), parallel to `drawables`.
    pub hashes: Vec<u32>,
    pub drawables: Vec<Drawable>,
}

impl DrawableDictionary {
    /// A ydd's dictionary, after checking the resource is in Enhanced's layout (159, as drawables). Layout: resource
    /// file header (16), hash list 0x20, drawable pointer list 0x30.
    pub fn read_resource(r: &Resource) -> Result<DrawableDictionary> {
        if r.version != VERSION {
            return Err(err(
                0,
                format!(
                    "resource version {}, Enhanced's ydd layout is {VERSION}",
                    r.version
                ),
            ));
        }
        let hash_list = pointer_list_u32(r, 0x5000_0020)?;
        let ptrs = pointer_list(r, 0x5000_0030)?;
        if hash_list.len() != ptrs.len() {
            return Err(err(
                0,
                format!("{} name hashes for {} drawables", hash_list.len(), ptrs.len()),
            ));
        }
        if hash_list
            .iter()
            .zip(hash_list.iter().skip(1))
            .any(|(a, b)| a >= b)
        {
            return Err(err(0, "name hashes not strictly ascending"));
        }
        let drawables = ptrs
            .into_iter()
            .map(|p| Drawable::read_gta(r, p))
            .collect::<Result<_>>()?;
        Ok(DrawableDictionary {
            hashes: hash_list,
            drawables,
        })
    }

    /// The drawable of this name hash.
    pub fn get(&self, hash: u32) -> Option<&Drawable> {
        self.hashes
            .binary_search(&hash)
            .ok()
            .and_then(|i| self.drawables.get(i))
    }
}

/// A simple list of u32 (pointer, count, capacity) whose 16-byte header is at `at`.
fn pointer_list_u32(r: &Resource, at: u64) -> Result<Vec<u32>> {
    let h = r.block::<16>(at)?;
    let (count, cap) = (le::u16(h, 8)?, le::u16(h, 10)?);
    if cap < count {
        return Err(err(0, format!("list count {count} over capacity {cap}")));
    }
    let n = usize::from(count);
    if n == 0 {
        return Ok(Vec::new());
    }
    let d = r.records(le::u64(h, 0)?, n, 4)?;
    Ok(d.as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect())
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    reason = "fixtures, not untrusted bytes"
)]
mod tests {
    use super::*;
    use crate::resource::PageFlags;

    /// A bone's pose is its own transform, then its parents' up to the root: a child turned a quarter about z and moved
    /// 1 along its parent's x, under a root moved 10 along z, takes (1, 0, 0) to (1, 1, 10).
    /// A fragment's own pose of a bone replaces the skeleton's rest pose of it; the others stay at rest.
    #[test]
    fn a_fragment_pose_replaces_the_rest_pose_of_its_bones() {
        let moved = [[1.0, 0.0, 0.0, 5.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
        let rest = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
        let bones = Bones {
            skeleton: Some(Skeleton {
                local: vec![rest, rest],
                parents: vec![None, Some(0)],
                tags: None,
            }),
            fragment_pose: vec![moved],
        };
        assert_eq!(bones.pose(), vec![moved, rest]);
        assert!(Bones::default().is_empty() && Bones::default().pose().is_empty());
    }

    #[test]
    fn bone_poses_compose_up_the_parent_chain() {
        let s = Skeleton {
            local: vec![
                [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 10.0]],
                [[0.0, -1.0, 0.0, 1.0], [1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]],
            ],
            parents: vec![None, Some(0)],
            tags: None,
        };
        let pose = s.pose();
        let at = |m: &[[f32; 4]; 3], p: [f32; 3]| m.map(|r| r[0] * p[0] + r[1] * p[1] + r[2] * p[2] + r[3]);
        assert_eq!(at(&pose[0], [0.0; 3]), [0.0, 0.0, 10.0]);
        assert_eq!(at(&pose[1], [1.0, 0.0, 0.0]), [1.0, 1.0, 10.0]);
        assert_eq!(at(&pose[1], [0.0, 1.0, 0.0]), [0.0, 0.0, 10.0]);
    }

    /// 8 KiB of system pages, zero but for `writes` (offset, bytes).
    fn pages(version: u32, writes: &[(usize, &[u8])]) -> Resource {
        let mut data = vec![0u8; 0x2000];
        for (o, b) in writes {
            data[*o..*o + b.len()].copy_from_slice(b);
        }
        Resource {
            version,
            system: PageFlags((1 << 27) | 4),
            graphics: PageFlags(0),
            data,
            zero_filled: 0,
        }
    }

    /// A ydr's light records come with its bones as stored; the bone a record's `bone_id` tags has its pose: a root
    /// (tag 0) at rest and a child tagged 0x1234 moved (1, 2, 3); a record naming no tag has no bone.
    #[test]
    fn light_records_take_the_pose_of_the_bone_they_tag() {
        let p = |o: u64| (0x5000_0000u64 + o).to_le_bytes();
        let mut identity = [0u8; 64];
        let mut moved = [0u8; 64];
        for k in 0..4 {
            identity[20 * k..20 * k + 4].copy_from_slice(&1f32.to_le_bytes());
            moved[20 * k..20 * k + 4].copy_from_slice(&1f32.to_le_bytes());
        }
        for (k, v) in [1f32, 2.0, 3.0].iter().enumerate() {
            moved[48 + 4 * k..52 + 4 * k].copy_from_slice(&v.to_le_bytes());
        }
        let light = |bone: u16| {
            let mut l = [0u8; 168];
            l[0x10..0x14].copy_from_slice(&1f32.to_le_bytes());
            l[0x24..0x26].copy_from_slice(&bone.to_le_bytes());
            l[0x26] = 1;
            l
        };
        let (a, b) = (light(0x1234), light(0x9999));
        let list = [&p(0x600)[..], &2u16.to_le_bytes(), &2u16.to_le_bytes()].concat();
        let r = pages(
            VERSION,
            &[
                (0x18, &p(0x100)),
                (0xB0, &list),
                (0x120, &p(0x200)),
                (0x130, &p(0x400)),
                (0x15E, &2u16.to_le_bytes()),
                (0x244, &0u16.to_le_bytes()),
                (0x294, &0x1234u16.to_le_bytes()),
                (0x400, &identity),
                (0x440, &moved),
                (0x600, &a),
                (0x600 + 168, &b),
            ],
        );
        let (lights, bones) = resource_lights(&r, "ydr", 0).unwrap();
        assert_eq!(lights.len(), 2);
        assert_eq!(lights[0].position, [0.0, 0.0, 1.0]);
        let pose = bones.pose();
        assert_eq!(
            bones.bone_of_tag(lights[0].bone_id).map(|b| pose[b]),
            Some([[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 2.0], [0.0, 0.0, 1.0, 3.0]])
        );
        assert_eq!(bones.bone_of_tag(lights[1].bone_id), None);
    }

    #[test]
    fn a_fragment_counting_more_extra_drawables_than_its_pages_hold_is_refused() {
        let ptr = (0x5000_0200u64).to_le_bytes();
        let r = pages(
            FRAGMENT_VERSION,
            &[(0x38, &ptr), (0x40, &ptr), (0x48, &u32::MAX.to_le_bytes())],
        );
        let e = Fragment::read_resource(&r).expect_err("count past the pages");
        assert!(e.to_string().contains("run past the pages"), "{e}");
    }

    #[test]
    fn a_triangle_count_whose_index_count_overflows_is_refused() {
        // 0x5555_5556 triangles: three times that wraps to 2 in 32 bits
        let r = pages(
            VERSION,
            &[
                (0x58, &2u32.to_le_bytes()),
                (0x5C, &0x5555_5556u32.to_le_bytes()),
                (0x62, &3u16.to_le_bytes()),
            ],
        );
        let e = read_geometry(&r, 0x5000_0000, 0, [[0.0; 4]; 2]).expect_err("overflow");
        assert!(e.to_string().contains("1431655766 triangles"), "{e}");
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    reason = "fixtures, not untrusted bytes"
)]
mod parameter_tests {
    use super::parameter_at;

    /// Worked by hand: in a buffer at 0x5000_0100 whose neighbour starts at 0x5000_0200 (0x100 bytes), a parameter at
    /// 0xF0 + 16 ends at its edge and one at 0xF0 + 20 runs past it; with the neighbour stored first (0x5000_0100 after
    /// 0x5000_0200), there is no edge to check and nothing to underflow; an address past the u64 range is refused.
    #[test]
    fn parameters_stay_in_their_buffer_without_overflow() {
        assert_eq!(
            parameter_at(0x5000_0100, Some(0x5000_0200), 0xF0, 16),
            Ok(0x5000_01F0)
        );
        assert!(parameter_at(0x5000_0100, Some(0x5000_0200), 0xF0, 20).is_err());
        assert_eq!(
            parameter_at(0x5000_0200, Some(0x5000_0100), 0, 4),
            Ok(0x5000_0200)
        );
        assert_eq!(parameter_at(0x5000_0200, None, 8, 4), Ok(0x5000_0208));
        assert!(parameter_at(u64::MAX - 2, None, 1, 4).is_err());
        assert!(parameter_at(u64::MAX, None, 1, 4).is_err());
    }
}
