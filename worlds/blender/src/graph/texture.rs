//! A texture slot ([`TextureRef`]) sampled as core states it.
//!
//! - **Coordinates**: the UV set the slot names (`TEXCOORD_<n>`, the save stage's name for the set), through the
//!   transforms that apply, each one Mapping node. Core's texture convention is glTF's (origin top-left, v down) and
//!   Blender's is v up, its UVs `(u, 1 - v)` (the glTF importer's flip). A transform `uv' = o + R(a) (S uv)` in core's
//!   frame is, in Blender's, scale `S`, rotation `-a` and location `(o_x - s_y sin a, 1 - o_y - s_y cos a)`: the
//!   Mapping node's point transform, exactly.
//! - **Wrap**: the image node's extension where both axes wrap alike (`REPEAT`, `EXTEND` for clamp, `MIRROR`). Where
//!   they differ, the image repeats and a clamped or mirrored axis has its coordinate made so first: mirrored by
//!   ping-pong, then held within the edge texels' centres (`0.5 / n` to `1 - 0.5 / n`), which is what clamping to the
//!   edge reads with either filter. Both are symmetric under `v -> 1 - v`, so they hold in Blender's frame.
//! - **Filters**: magnification nearest is `Closest`, otherwise `Linear`; a data texture with no filter stated is read
//!   exactly (`Closest`), as core's data role states. Cycles samples the largest level with that one filter: a stated
//!   minification filter that differs, or a mip filter, has no form, counted. Cycles filters an 8-bit sRGB image
//!   before decoding it, core after: each filtered colour slot is counted.
//! - **Images**: a texture's largest level as an RGBA8 PNG, its alpha channel-packed (not premultiplied), read as
//!   sRGB in a colour slot and as data (`Non-Color`) in any other, one image per texture and reading. A texture still
//!   only a file, or one whose format an 8-bit PNG cannot hold exactly ([`primase_image::rgba8_exact`]), is not
//!   sampled: its slot is left empty, counted by reason (the glTF writer's rule and its passes:
//!   `lower-textures-rgba8`, a reader's load step).

use std::collections::BTreeMap;

use iridia_interface::{Filter, MipFilter};
use primase_core::pbr::{TextureRef, UvTransform, Wrap};
use primase_core::{Entity, Name, NormalConvention, TexRole, Texture, TextureAbsent, TextureFile, World};
use serde::Serialize;
use serde_json::json;

use super::{Float, Graph, Socket, Vec3};
use crate::{Error, Result};

/// How an image's pixels are read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Reading {
    /// sRGB-encoded colour, decoded to linear.
    #[serde(rename = "sRGB")]
    Srgb,
    /// Values as stored.
    #[serde(rename = "Non-Color")]
    Data,
}

/// An image the graphs sample: a texture's largest level, read one way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageUse {
    pub texture: Entity,
    pub reading: Reading,
    /// The texture's name (its entity where it has none).
    pub name: String,
}

/// The images every graph samples, each once per (texture, reading), in first-use order.
#[derive(Clone, Debug, Default)]
pub struct Images {
    index: BTreeMap<(Entity, Reading), usize>,
    pub list: Vec<ImageUse>,
}

/// What the graphs could not draw as core states it, counted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Texture slots left empty, by reason, counted per slot per graph.
    pub slots_unsampled: BTreeMap<String, u64>,
    /// Slots stating a minification filter other than the one Cycles samples with, or a mip filter: Cycles samples the
    /// largest level with the magnification filter.
    pub minification_unexpressed: u64,
    /// Filtered colour (sRGB) slots: Cycles filters an 8-bit sRGB image's encoded values and decodes the result, where
    /// core's sampling decodes each texel and filters the linear values. Between two texels the two differ (a texel
    /// centre reads alike).
    pub srgb_filtered_encoded: u64,
    /// Occlusion textures: Cycles traces the occlusion of the scene itself, and a stated occlusion map has no form.
    pub occlusion_unwritten: u64,
    /// Normal maps drawn in the tangent frame Blender computes from the slot's UV set (MikkTSpace), not the mesh's
    /// stored tangents.
    pub normal_maps_mikktspace: u64,
    /// Specular strengths below 1 or textured: `KHR_materials_specular` scales the reflectance at grazing angles too
    /// (f90), Blender's specular level the reflectance at normal incidence only.
    pub specular_f90_unexpressed: u64,
}

/// A texture sampled: its colour (linear for an sRGB reading) and alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub color: Vec3,
    pub alpha: Float,
}

/// Where a slot reads its coordinates when a layer samples it: the layer's UV set in place of the slot's own, and the
/// layer's transform before the slot's; and, for a slot that moves (a water body's wave), how far its coordinates move
/// per second after the slot's transform, in core's frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Coordinates {
    pub texcoord: Option<u8>,
    pub before: Option<UvTransform>,
    pub scroll: Option<[f32; 2]>,
}

/// The graphs' shared state: the world, the images sampled and what is counted.
#[derive(Debug)]
pub struct Sampler<'w> {
    pub world: &'w World,
    pub images: Images,
    pub counts: Counts,
}

impl<'w> Sampler<'w> {
    pub fn new(world: &'w World) -> Self {
        Sampler {
            world,
            images: Images::default(),
            counts: Counts::default(),
        }
    }

    /// The UV set's name in the saved scene.
    pub fn uv_map(set: u8) -> String {
        format!("TEXCOORD_{set}")
    }

    /// Slot `r` sampled in `g`, read as `reading`, through `at` (a layer's coordinates); `None`, counted, where its
    /// texture is not sampled.
    pub fn sample(
        &mut self,
        g: &mut Graph,
        r: &TextureRef,
        reading: Reading,
        at: Coordinates,
    ) -> Result<Option<Sample>> {
        let Some(Image {
            index: image,
            width,
            height,
            data,
        }) = self.image(r.texture, reading)?
        else {
            return Ok(None);
        };
        let set = at.texcoord.unwrap_or(r.texcoord);
        let uv = g.node("ShaderNodeUVMap", &[("uv_map", json!(Self::uv_map(set)))]);
        let mut v = Vec3::Out(Socket {
            node: uv,
            output: "UV",
        });
        for t in [at.before, r.transform].into_iter().flatten() {
            if t != UvTransform::IDENTITY {
                v = mapping(g, v, t);
            }
        }
        // core's v grows down, Blender's up: a movement of (u, v) is (u, -v)
        if let Some([du, dv]) = at.scroll {
            let moved = g.per_second([du, -dv]);
            v = g.vadd(v, moved);
        }
        let [wu, wv] = r.wrap;
        let extension = if wu == wv {
            extension(wu)
        } else {
            let [u, w, z] = g.separate(v);
            let u = wrap_axis(g, u, wu, width);
            let w = wrap_axis(g, w, wv, height);
            v = g.combine([u, w, z]);
            "REPEAT"
        };
        let filters = r.filters;
        let closest = filters.mag == Some(Filter::Nearest) || (data && filters == Default::default());
        let interpolation = if closest { Filter::Nearest } else { Filter::Linear };
        if filters.min.is_some_and(|m| m != interpolation)
            || matches!(filters.mip, Some(MipFilter::Nearest | MipFilter::Linear))
        {
            self.counts.minification_unexpressed += 1;
        }
        if reading == Reading::Srgb && !closest {
            self.counts.srgb_filtered_encoded += 1;
        }
        let n = g.node(
            "ShaderNodeTexImage",
            &[
                ("interpolation", json!(if closest { "Closest" } else { "Linear" })),
                ("extension", json!(extension)),
                ("projection", json!("FLAT")),
            ],
        );
        g.nodes[n].image = Some(image);
        g.vector_in(n, "Vector", v);
        Ok(Some(Sample {
            color: Vec3::Out(Socket {
                node: n,
                output: "Color",
            }),
            alpha: Float::Out(Socket {
                node: n,
                output: "Alpha",
            }),
        }))
    }

    /// Whether texture `e` is sampled: why not where it is not (its pixels not loaded, or no exact 8-bit form).
    pub fn unsampled(&self, e: Entity) -> Result<Option<String>> {
        let Some(t) = self.world.get::<Texture>(e) else {
            // a texture the source names that its read could not give: the slot is drawn bound to none
            if let Some(absent) = self.world.get::<TextureAbsent>(e) {
                return Ok(Some(format!("the read could not give it: {}", absent.reason)));
            }
            if self.world.has::<TextureFile>(e) {
                // a reader's load step makes the pixels (primase_image::load_textures); the writer reads no file
                return Ok(Some("texture files not loaded".into()));
            }
            return Err(Error::World {
                entity: e,
                detail: "texture entity with neither core.texture_file nor core.texture".into(),
            });
        };
        if t.role == TexRole::Normal && t.normal != Some(NormalConvention::GLTF) {
            return Err(Error::World {
                entity: e,
                detail:
                    "normal texture requires tangent/bitangent basis, X in red, green up and stored Z; run                          normalize-normals first"
                        .into(),
            });
        }
        if !primase_image::rgba8_exact(t.format) {
            // its values have no exact 8-bit form: lower-textures-rgba8 converts it, as a program chooses
            return Ok(Some(format!("{:?} (no exact 8-bit form)", t.format)));
        }
        Ok(None)
    }

    /// The image of texture `e` read as `reading`; `None`, counted, where it is not sampled ([`Sampler::unsampled`]).
    fn image(&mut self, e: Entity, reading: Reading) -> Result<Option<Image>> {
        if let Some(reason) = self.unsampled(e)? {
            *self.counts.slots_unsampled.entry(reason).or_default() += 1;
            return Ok(None);
        }
        let t = self
            .world
            .get::<Texture>(e)
            .expect("a sampled texture has core.texture");
        let next = self.images.list.len();
        let i = *self.images.index.entry((e, reading)).or_insert(next);
        if i == next {
            let name = self
                .world
                .get::<Name>(e)
                .map_or_else(|| format!("texture {}", e.index()), |n| n.0.clone());
            self.images.list.push(ImageUse {
                texture: e,
                reading,
                name,
            });
        }
        Ok(Some(Image {
            index: i,
            width: t.width,
            height: t.height,
            data: t.role == TexRole::Data,
        }))
    }
}

/// An image a slot samples: its index in [`Images::list`], its size and whether its texture is data.
struct Image {
    index: usize,
    width: u32,
    height: u32,
    data: bool,
}

/// One axis's coordinate `c` made so for a wrap `w` other than repeat, over `texels` texels, for an image that repeats
/// (see the module docs).
fn wrap_axis(g: &mut Graph, c: Float, w: Wrap, texels: u32) -> Float {
    let c = match w {
        Wrap::Repeat => return c,
        Wrap::Clamp => c,
        // ping-pong of period 2: c on 0-1, 2 - c on 1-2, ...
        Wrap::Mirror => g.math("PINGPONG", &[c, Float::Const(1.0)]),
    };
    let half = 0.5 / texels as f32;
    let n = g.node("ShaderNodeClamp", &[("clamp_type", json!("MINMAX"))]);
    g.float_in(n, "Value", c);
    g.float_in(n, "Min", Float::Const(half));
    g.float_in(n, "Max", Float::Const(1.0 - half));
    Float::Out(Socket {
        node: n,
        output: "Result",
    })
}

/// A Mapping node applying core's transform `t` to Blender-frame coordinates `v` (see the module docs).
fn mapping(g: &mut Graph, v: Vec3, t: UvTransform) -> Vec3 {
    let (sin, cos) = t.rotation.sin_cos();
    let location = [
        t.offset[0] - t.scale[1] * sin,
        1.0 - t.offset[1] - t.scale[1] * cos,
        0.0,
    ];
    let n = g.node("ShaderNodeMapping", &[("vector_type", json!("POINT"))]);
    g.vector_in(n, "Vector", v);
    g.vector_in(n, "Location", Vec3::Const(location));
    g.vector_in(n, "Rotation", Vec3::Const([0.0, 0.0, -t.rotation]));
    g.vector_in(n, "Scale", Vec3::Const([t.scale[0], t.scale[1], 1.0]));
    Vec3::Out(Socket {
        node: n,
        output: "Vector",
    })
}

/// Blender's image extension for a wrap along both axes.
fn extension(w: Wrap) -> &'static str {
    match w {
        Wrap::Repeat => "REPEAT",
        Wrap::Clamp => "EXTEND",
        Wrap::Mirror => "MIRROR",
    }
}
