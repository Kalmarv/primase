//! `rebase-texcoord-set{set=N,limit=L}`: a material whose slots sample two coordinate sets, drawn through one, for a
//! destination that samples one set. The slots that sample another set are re-rasterised into the space of set `N`
//! (an atlas per primitive, the geometry's own parameterisation drawn as the texture), and the primitive then samples
//! only set `N`, as its first. The reverse of `first-texcoord-set`'s swap, which cannot hold two sets.
//!
//! **Which primitives.** One drawn with a `pbr.material` that has a slot sampling set `N` and a slot sampling another
//! set. Every other slot of the material samples one same set `s`; a primitive left as it is, counted by reason and
//! by triangle, when its slots sample three sets, it has material variants (`specialize-material-variants` first), the
//! material is layered (`core.layered`), it is not a triangle list with the streams of both sets (float2) and
//! positions, a texture is not loaded as pixels (`load-textures`) or is signed (`lower-textures-rgba8` remaps those),
//! or the normal map's convention is not X in red along the tangent (a normal map is turned between the sets' frames,
//! below). A texture wider than 8 bits or floating point is read as 8-bit RGBA, the atlases being 8-bit: rounded, or
//! clamped to 0-1, and counted with the floating-point texels that lay outside 0-1.
//!
//! **A set the texture is constant over.** When every slot sampling set `N` is the emissive one and its texture, read
//! through the slot's transform and wrap over every texel any triangle's coordinates touch (a bilinear sample reads
//! the four texels around it; a triangle whose coordinates span more than 64 texels a side is not tried), holds one
//! colour within 2 of 255 in each channel, nothing is drawn through set `N` that a factor does not hold: the material
//! is copied with the emissive colour multiplied by the mean of those texels (sRGB to linear) and no emissive
//! texture. Counted as emission folded into a factor; no coordinates change.
//!
//! **Otherwise, an atlas.** The primitive's set `N` is its own parameterisation when no two triangles' coordinates
//! overlap. The window is the bounding box of the primitive's set-`N` coordinates; the atlas is its texels, a power of
//! two a side on each axis, at the density that keeps the most texels per unit area of any slot: the slots sampling
//! set `s` (texture size times the slot transform's scale, times the area of the triangles' set-`s` coordinates, over
//! the area of their set-`N` coordinates) and the slots sampling set `N` (their texture size times the transform's
//! scale), at most `L` a side. Each triangle claims its texels ([`crate::claimed_texels`]); the triangles are put into
//! groups in order, each in the first group where it overlaps no other triangle's texels, at most
//! [`MAX_GROUPS`] groups; a primitive that needs more is left as it is, counted. Each group is an atlas of its own: the
//! primitive's triangles split into one primitive per group (every stream kept, vertices compacted; a mesh with
//! per-vertex components is left as it is) when there are several. Every slot of the material becomes a texture of the
//! group's size in the new coordinates: [`crate::rebake`] draws the group's triangles, each texel the slot's texture
//! sampled bilinearly at the coordinates its slot samples (set `s` interpolated across the triangle for the slots on
//! `s`, set `N` itself for the slots on `N`), through the slot's transform and wrap, with [`PADDING`] rounds of padding
//! around the triangles. The group's primitive gets a copy of the material ([`primase_core::World::duplicate`]) whose
//! slots point at the new textures: `TexCoord(0)`, no transform, clamped, the slot's filters. The primitive's
//! `TexCoord(0)` becomes its window coordinates (`(uv - window origin) / window size`), replacing the first set; the
//! other sets are as they were. A texel no triangle reached is the slot's neutral value (white; flat normal; black
//! emission).
//!
//! **Normal maps.** A tangent-space normal is written against the frame its coordinates give: the tangent along
//! increasing u, the bitangent along v (decreasing v for a map whose green is up, as the world's v runs down the
//! image; increasing for green down), each in the triangle's plane, the bitangent made perpendicular to the tangent.
//! The frame of set `N` differs from the frame of set `s` by a rotation (or a mirror) that is different for every
//! triangle, so each triangle's texels are turned by it: the (x, y) of the map, decoded `2 v / 255 - 1`, become the
//! components of the same surface vector along the new axes, `x' = (T' . T) x + (T' . B) y` and
//! `y' = (B' . T) x + (B' . B) y`, re-encoded with the same rounding. The blue channel (Z) is unchanged. A primitive
//! with supplied tangents (`Tangent(0)`) keeps the map as it is: its frame is the supplied one, whatever the
//! coordinates say. A triangle with no frame (degenerate coordinates or area) keeps its texels, counted.
//!
//! The pass is approximate in what it resamples: bilinear sampling at the atlas' texel density; the neighbouring
//! texels of a triangle's edge are its own colours, not the neighbour triangle's. It knows no game.

use crate::rgba8::{Conversion, conversion};
use crate::texture::level_dimensions;
use crate::uv_resample::wrapped;
use crate::{
    Canvas, Image, Samples, Source, UvTriangle, claimed_texels, rebake, srgb8_to_linear, texture_level_rgba8,
};
use iridia_interface::Filters;
use primase_core::layered::LayeredMaterial;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{PbrMaterial, TextureRef, TextureSlot, UvTransform, Wrap};
use primase_core::{
    ColorSpace, Component, Entity, MaterialVariants, Mesh, MeshFile, Name, NormalBasis, NormalConvention,
    NormalGreen, NormalX, Primitive, Semantic, Stream, TexFormat, TexRole, Texture, Topology, World,
};
use primase_meshops::frames::uv_axes;
use primase_meshops::subset_primitive;
use primase_texcodec::{Decoder, TexDecoder};
use std::collections::BTreeMap;

/// The most groups of non-overlapping triangles a primitive is split into; a primitive needing more is left as it is.
pub const MAX_GROUPS: usize = 16;
/// Rounds of padding around the triangles of an atlas.
pub const PADDING: u32 = 4;
/// A texture is constant when its texels differ from one another by at most this, in 8-bit channel values.
const CONSTANT_TOLERANCE: u8 = 2;
/// The most texels a triangle's coordinates may span along an axis for the constant test.
const FOOTPRINT: i64 = 64;

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RebaseTexcoordSet {
    set: u8,
    limit: u32,
}

impl RebaseTexcoordSet {
    /// Rebase onto `TexCoord(set)`, atlases of at most `limit` x `limit` texels (4 to 8192).
    pub fn new(set: u8, limit: u32) -> Result<RebaseTexcoordSet, String> {
        if !(4..=8192).contains(&limit) {
            return Err(format!("limit={limit}: an atlas is 4 to 8192 texels a side"));
        }
        Ok(RebaseTexcoordSet { set, limit })
    }

    fn from_given(g: &Given) -> Result<RebaseTexcoordSet, String> {
        let (s, l) = (g.required("set"), g.required("limit"));
        RebaseTexcoordSet::new(
            s.parse::<u8>()
                .map_err(|_| format!("set={s} is not a coordinate set (0-255)"))?,
            l.parse::<u32>()
                .map_err(|_| format!("limit={l} is not a number of texels"))?,
        )
    }
}

/// `rebase-texcoord-set`'s declaration ([`primase_core::pass::PassRegistry::add`]): the set and the largest atlas are
/// the destination's, required.
pub const REBASE_TEXCOORD_SET: PassSpec = PassSpec {
    name: "rebase-texcoord-set",
    options: &[
        OptionSpec::required(
            "set",
            "4",
            "the coordinate set the slots of other sets are re-rasterised into",
        ),
        OptionSpec::required("limit", "256", "texels a side of the largest atlas"),
    ],
    make: |g| Ok(Box::new(RebaseTexcoordSet::from_given(g)?)),
};

/// What a slot's texture is, apart from its pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Meta {
    color_space: ColorSpace,
    role: TexRole,
    normal: Option<NormalConvention>,
}

/// The approximations of reading textures as 8-bit RGBA, each with its count.
type Loss = Vec<(&'static str, u64)>;

/// A slot of the primitive's material with its pixels.
struct Slot {
    slot: TextureSlot,
    reference: TextureRef,
    image: Image,
    meta: Meta,
    /// The approximations reading the texture as 8-bit RGBA made, each with its count.
    loss: Loss,
}

/// The value of a texel nothing reached.
fn neutral(slot: TextureSlot) -> [u8; 4] {
    match slot {
        TextureSlot::Normal => [128, 128, 255, 255],
        TextureSlot::Emissive => [0, 0, 0, 255],
        _ => [255; 4],
    }
}

type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(a: V3) -> Option<V3> {
    let l = dot(a, a).sqrt();
    (l > 0.0 && l.is_finite()).then(|| a.map(|x| x / l))
}

/// The two axes the channels of a normal map written against `uv` point along on triangle `p`: the tangent along
/// increasing u, and the bitangent along v made perpendicular to it (negated for a map whose green is up).
pub(crate) fn channel_axes(p: [V3; 3], uv: [[f32; 2]; 3], green: NormalGreen) -> Option<[V3; 2]> {
    let n = unit(cross(sub(p[1], p[0]), sub(p[2], p[0])))?;
    let (t, v) = uv_axes(p, uv, n)?;
    let b = unit(sub(v, t.map(|x| x * dot(t, v))))?;
    let side = match green {
        NormalGreen::Up => -1.0,
        NormalGreen::Down => 1.0,
    };
    Some([t, b.map(|x| x * side)])
}

/// How channels written against `stored` read against `target`: `out_k = sum_j (target_k . stored_j) in_j`.
fn turn(stored: [V3; 2], target: [V3; 2]) -> [[f32; 2]; 2] {
    std::array::from_fn(|k| std::array::from_fn(|j| dot(target[k], stored[j])))
}

/// The triangles of a primitive as index triples; `None` unless it is a triangle list whose indices are in range.
fn triangles(p: &Primitive) -> Option<Vec<[u32; 3]>> {
    if p.topology != Topology::Triangles {
        return None;
    }
    let n = p.vertex_count();
    let all: Vec<u32> = match &p.indices {
        Some(i) => i.clone(),
        None => (0..u32::try_from(n).ok()?).collect(),
    };
    if !all.len().is_multiple_of(3) || all.iter().any(|&i| i as usize >= n) {
        return None;
    }
    Some(all.as_chunks::<3>().0.to_vec())
}

fn uv_stream(p: &Primitive, set: u8) -> Option<&Vec<[f32; 2]>> {
    match p.streams.get(&Semantic::TexCoord(set)) {
        Some(Stream::F32x2(v)) => Some(v),
        _ => None,
    }
}

/// Twice the area of the triangle in its coordinates.
fn area2(uv: [[f32; 2]; 3]) -> f64 {
    let (a, b, c) = (uv[0].map(f64::from), uv[1].map(f64::from), uv[2].map(f64::from));
    ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs()
}

/// The one colour (mean of the texels read, 0-255) the slot's texture holds under every triangle of `uv`, if it holds
/// one (see the module documentation).
fn constant_colour(slot: &Slot, uv: &[[[f32; 2]; 3]]) -> Option<[f64; 3]> {
    let Samples::U8(px) = &slot.image.samples else {
        return None;
    };
    let (w, h) = (i64::from(slot.image.width), i64::from(slot.image.height));
    let (mut lo, mut hi) = ([u8::MAX; 3], [u8::MIN; 3]);
    let (mut sum, mut n) = ([0f64; 3], 0u64);
    for tri in uv {
        let moved = tri.map(|c| slot.reference.transform.map_or(c, |t: UvTransform| t.apply(c)));
        let mut span = [(i64::MAX, i64::MIN); 2];
        for c in moved {
            for (axis, size) in [(0, w), (1, h)] {
                if !c[axis].is_finite() {
                    return None;
                }
                let x = (f64::from(c[axis]) * size as f64 - 0.5).floor() as i64;
                span[axis] = (span[axis].0.min(x), span[axis].1.max(x + 1));
            }
        }
        if span.iter().any(|&(a, b)| b - a + 1 > FOOTPRINT) {
            return None;
        }
        for y in span[1].0..=span[1].1 {
            for x in span[0].0..=span[0].1 {
                let (cx, cy) = (
                    wrapped(x, w, slot.reference.wrap[0]),
                    wrapped(y, h, slot.reference.wrap[1]),
                );
                let at = (cy * w as usize + cx) * 4;
                for k in 0..3 {
                    lo[k] = lo[k].min(px[at + k]);
                    hi[k] = hi[k].max(px[at + k]);
                    sum[k] += f64::from(px[at + k]);
                }
                n += 1;
            }
        }
    }
    if n == 0 || (0..3).any(|k| hi[k] - lo[k] > CONSTANT_TOLERANCE) {
        return None;
    }
    Some(sum.map(|s| s / n as f64))
}

/// The atlas of one group of a primitive's triangles: its textures, one per slot.
struct GroupPlan {
    triangles: Vec<usize>,
    textures: Vec<(TextureSlot, Texture, Filters)>,
}

/// What is done to one primitive.
enum Plan {
    Fold {
        material: Entity,
        colour: [u8; 3],
        loss: Loss,
    },
    Rebase {
        material: Entity,
        window: ([f32; 2], [f32; 2]),
        size: (u32, u32),
        /// Whether the limit made an atlas smaller than the density asks.
        clamped: bool,
        groups: Vec<GroupPlan>,
        loss: Loss,
    },
}

/// Why a primitive is left as it is.
fn refuse(log: &mut PassLog, why: &str, triangles: u64) {
    log.count(&format!("primitives not rebased: {why}"), 1);
    log.count(&format!("triangles not rebased: {why}"), triangles);
}

impl RebaseTexcoordSet {
    fn load(w: &World, r: TextureRef) -> Result<(Image, Meta, Loss), String> {
        let Some(t) = w.get::<Texture>(r.texture) else {
            return Err("a texture is not loaded as pixels".to_owned());
        };
        // the atlases are 8-bit, so a wider or floating-point source is read as 8-bit RGBA, each counted
        let mut loss = Vec::new();
        match conversion(t.format) {
            None => {}
            Some(Conversion::Signed) => return Err(format!("a texture is signed ({:?})", t.format)),
            Some(Conversion::Rounded) => {
                loss.push((
                    "textures with channels wider than 8 bits, read rounded to 8-bit RGBA",
                    1,
                ));
            }
            Some(Conversion::Clamped) => {
                let (width, height) =
                    level_dimensions(t, 0).map_err(|_| "a texture does not decode".to_owned())?;
                let level = t.levels.first().ok_or("a texture does not decode")?;
                let values = Decoder::default()
                    .decode_rgba32f(t.format, width, height, level)
                    .map_err(|_| "a texture does not decode".to_owned())?;
                let outside = values
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|p| p.iter().any(|v| !(0.0..=1.0).contains(v)))
                    .count() as u64;
                loss.push(("floating-point textures read clamped to 0-1 in 8-bit RGBA", 1));
                if outside > 0 {
                    loss.push((
                        "floating-point texels of those textures with a channel outside 0-1, clamped",
                        outside,
                    ));
                }
            }
        }
        let image = texture_level_rgba8(t, 0).map_err(|_| "a texture does not decode".to_owned())?;
        Ok((
            image,
            Meta {
                color_space: t.color_space,
                role: t.role,
                normal: t.normal,
            },
            loss,
        ))
    }

    /// What to do with primitive `i` of mesh `e`; `None` when it is left as it is (counted when it is a candidate).
    fn plan(&self, w: &World, e: Entity, i: usize, p: &Primitive, log: &mut PassLog) -> Option<Plan> {
        let m = p.material?;
        let material = w.get::<PbrMaterial>(m)?;
        let n = self.set;
        let mut slots: Vec<(TextureSlot, TextureRef)> = material.slots().collect();
        let on = slots.iter().filter(|(_, r)| r.texcoord == n).count();
        if on == 0 || on == slots.len() {
            return None;
        }
        let tris = triangles(p);
        let count = tris
            .as_ref()
            .map_or_else(|| p.vertex_count() as u64 / 3, |t| t.len() as u64);
        let mut others: Vec<u8> = slots
            .iter()
            .map(|(_, r)| r.texcoord)
            .filter(|&s| s != n)
            .collect();
        others.sort_unstable();
        others.dedup();
        let &[s] = &others[..] else {
            refuse(log, "the material samples more than two coordinate sets", count);
            return None;
        };
        if w.get::<MaterialVariants>(e)
            .and_then(|v| v.0.get(i))
            .is_some_and(|v| !v.is_empty())
        {
            refuse(log, "the primitive has material variants", count);
            return None;
        }
        if w.has::<LayeredMaterial>(m) {
            refuse(log, "the material is layered", count);
            return None;
        }
        let (Some(tris), Some(positions), Some(uv_n), Some(uv_s)) =
            (tris, p.positions(), uv_stream(p, n), uv_stream(p, s))
        else {
            refuse(
                log,
                "the primitive is not a triangle list with positions and both coordinate sets",
                count,
            );
            return None;
        };
        if tris.is_empty() || uv_n.len() != positions.len() || uv_s.len() != positions.len() {
            refuse(
                log,
                "the primitive has no triangles or its coordinate sets are cut short",
                count,
            );
            return None;
        }
        // the pixels of every slot
        let mut loaded: Vec<Slot> = Vec::with_capacity(slots.len());
        for (slot, reference) in slots.drain(..) {
            match Self::load(w, reference) {
                Ok((image, meta, loss)) => loaded.push(Slot {
                    slot,
                    reference,
                    image,
                    meta,
                    loss,
                }),
                Err(why) => {
                    refuse(log, &why, count);
                    return None;
                }
            }
        }
        let corners = |set: &[[f32; 2]]| -> Vec<[[f32; 2]; 3]> {
            tris.iter().map(|t| t.map(|v| set[v as usize])).collect()
        };
        let (cn, cs) = (corners(uv_n), corners(uv_s));
        // a texture constant under the coordinates is a factor
        let on_slots: Vec<&Slot> = loaded.iter().filter(|s| s.reference.texcoord == n).collect();
        if on_slots.iter().all(|s| s.slot == TextureSlot::Emissive)
            && let Some(slot) = on_slots.first()
            && let Some(mean) = constant_colour(slot, &cn)
        {
            return Some(Plan::Fold {
                material: m,
                colour: mean.map(|c| c.round().clamp(0.0, 255.0) as u8),
                loss: loaded.iter().flat_map(|s| s.loss.iter().copied()).collect(),
            });
        }
        // the window
        let mut lo = [f32::INFINITY; 2];
        let mut hi = [f32::NEG_INFINITY; 2];
        for c in cn.iter().flatten() {
            lo = [lo[0].min(c[0]), lo[1].min(c[1])];
            hi = [hi[0].max(c[0]), hi[1].max(c[1])];
        }
        let extent = [hi[0] - lo[0], hi[1] - lo[1]];
        let area_n: f64 = cn.iter().map(|t| area2(*t)).sum::<f64>() / 2.0;
        let covers = area_n > 0.0 && area_n.is_finite();
        if !extent.iter().all(|x| x.is_finite() && *x > 0.0) || !covers {
            refuse(log, "the coordinates cover no area", count);
            return None;
        }
        let area_s: f64 = cs.iter().map(|t| area2(*t)).sum::<f64>() / 2.0;
        let per_unit = |s: &&Slot| {
            let scale = s
                .reference
                .transform
                .map_or(1.0, |t| f64::from((t.scale[0] * t.scale[1]).abs()));
            f64::from(s.image.width) * f64::from(s.image.height) * scale
        };
        let d_s = loaded
            .iter()
            .filter(|s| s.reference.texcoord != n)
            .map(|s| per_unit(&s))
            .fold(0.0, f64::max);
        let d_n = on_slots.iter().map(per_unit).fold(0.0, f64::max);
        let rho = (d_s * area_s / area_n).max(d_n).sqrt();
        let wanted = extent.map(|x| (f64::from(x) * rho).ceil().max(1.0));
        let clamped = wanted.iter().any(|x| *x > f64::from(self.limit));
        let side = |x: f64| {
            (x.min(f64::from(self.limit)) as u32)
                .next_power_of_two()
                .min(self.limit)
        };
        let size = (side(wanted[0]), side(wanted[1]));
        let window = (lo, extent);
        let to = |c: [f32; 2]| [(c[0] - lo[0]) / extent[0], (c[1] - lo[1]) / extent[1]];
        // the groups of triangles that overlap no other of their group
        let mut groups: Vec<(Vec<usize>, Vec<bool>)> = Vec::new();
        for (t, tri) in cn.iter().enumerate() {
            let texels = claimed_texels(&tri.map(to), size.0, size.1);
            let at = |&(x, y): &(u32, u32)| y as usize * size.0 as usize + x as usize;
            let fits = groups
                .iter()
                .position(|(_, taken)| texels.iter().all(|x| !taken[at(x)]));
            let g = match fits {
                Some(g) => g,
                None if groups.len() < MAX_GROUPS => {
                    groups.push((Vec::new(), vec![false; size.0 as usize * size.1 as usize]));
                    groups.len() - 1
                }
                None => {
                    refuse(
                        log,
                        &format!("the coordinates overlap beyond {MAX_GROUPS} groups of triangles"),
                        count,
                    );
                    return None;
                }
            };
            for x in &texels {
                groups[g].1[at(x)] = true;
            }
            groups[g].0.push(t);
        }
        if groups.len() > 1 && !w.per_vertex_components(e).is_empty() {
            refuse(
                log,
                "the coordinates overlap and the mesh holds data per vertex",
                count,
            );
            return None;
        }
        // each triangle's turn between the frames of the two sets, for a normal map written against the first
        let normal = loaded.iter().find(|s| s.slot == TextureSlot::Normal);
        let mut turns: Vec<Option<[[f32; 2]; 2]>> = Vec::new();
        let mut no_frame = 0u64;
        if let Some(normal) = normal {
            let convention = normal.meta.normal.unwrap_or(NormalConvention::GLTF);
            if convention.x != NormalX::Red || convention.basis != NormalBasis::TangentBitangent {
                refuse(
                    log,
                    "the normal map's convention is not X in red along the tangent",
                    count,
                );
                return None;
            }
            if !p.streams.contains_key(&Semantic::Tangent(0)) {
                for (tri, (a, b)) in tris.iter().zip(cs.iter().zip(&cn)) {
                    let pos = tri.map(|v| positions[v as usize]);
                    let turned = channel_axes(pos, *a, convention.green)
                        .zip(channel_axes(pos, *b, convention.green))
                        .map(|(stored, target)| turn(stored, target));
                    no_frame += u64::from(turned.is_none());
                    turns.push(turned);
                }
            }
        }
        // draw
        let mut plans = Vec::with_capacity(groups.len());
        for (members, _) in groups {
            let mut textures = Vec::with_capacity(loaded.len());
            for slot in &loaded {
                let from = if slot.reference.texcoord == n { &cn } else { &cs };
                let drawn: Vec<UvTriangle> = members
                    .iter()
                    .map(|&t| UvTriangle {
                        to: cn[t].map(to),
                        from: from[t],
                    })
                    .collect();
                let source = Source {
                    image: &slot.image,
                    wrap: slot.reference.wrap,
                    transform: slot.reference.transform,
                };
                let canvas = Canvas {
                    size,
                    padding: PADDING,
                    fill: neutral(slot.slot),
                };
                let normal_map = slot.slot == TextureSlot::Normal;
                let rebaked = match rebake(&source, &drawn, &canvas, |k, px| match turns.get(members[k]) {
                    Some(Some(m)) if normal_map => {
                        let (x, y) = (px[0] / 127.5 - 1.0, px[1] / 127.5 - 1.0);
                        let (nx, ny) = (m[0][0] * x + m[0][1] * y, m[1][0] * x + m[1][1] * y);
                        [(nx + 1.0) * 127.5, (ny + 1.0) * 127.5, px[2], px[3]]
                    }
                    _ => px,
                }) {
                    Ok(r) => r,
                    Err(_) => {
                        refuse(log, "a texture does not resample", count);
                        return None;
                    }
                };
                let Samples::U8(bytes) = rebaked.image.samples else {
                    refuse(log, "a texture does not resample", count);
                    return None;
                };
                let texture = Texture {
                    width: size.0,
                    height: size.1,
                    format: TexFormat::Rgba8Unorm,
                    levels: vec![bytes],
                    color_space: slot.meta.color_space,
                    role: slot.meta.role,
                    normal: slot.meta.normal,
                };
                textures.push((slot.slot, texture, slot.reference.filters));
            }
            plans.push(GroupPlan {
                triangles: members,
                textures,
            });
        }
        log.count(
            "triangles with no frame in a coordinate set: normal texels kept",
            no_frame,
        );
        Some(Plan::Rebase {
            material: m,
            window,
            size,
            clamped,
            groups: plans,
            loss: loaded.iter().flat_map(|s| s.loss.iter().copied()).collect(),
        })
    }
}

/// `name` with a suffix, as the new entity's name.
fn named(w: &World, e: Entity, suffix: &str) -> Name {
    Name(w.get::<Name>(e).map_or_else(|| format!("{e:?}"), |n| n.0.clone()) + suffix)
}

/// `material` copied with its slots pointing at `textures` (one per slot, in the slots' order).
fn material_copy(
    w: &mut World,
    material: Entity,
    textures: &[(TextureSlot, Entity, Filters)],
    suffix: &str,
) -> Result<Entity, String> {
    let name = named(w, material, suffix);
    let copy = w.duplicate(material).ok_or("a material that is not alive")?;
    let m = w
        .get_mut::<PbrMaterial>(copy)
        .ok_or("a material that is not lowered")?;
    for (slot, r) in m.slots_mut() {
        if let Some((_, texture, filters)) = textures.iter().find(|(s, _, _)| *s == slot) {
            *r = TextureRef {
                texture: *texture,
                texcoord: 0,
                wrap: [Wrap::Clamp; 2],
                transform: None,
                filters: *filters,
            };
        }
    }
    w.insert(copy, name);
    Ok(copy)
}

impl Pass for RebaseTexcoordSet {
    fn name(&self) -> &'static str {
        "rebase-texcoord-set"
    }
    fn options(&self) -> Options {
        Options::from([
            ("set".to_owned(), self.set.to_string()),
            ("limit".to_owned(), self.limit.to_string()),
        ])
    }
    /// A part with no material (a light alone) has nothing to rebase.
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn requires(&self) -> &[&'static str] {
        &[PbrMaterial::NAME]
    }
    /// The atlas textures, the material copies and their names.
    fn provides(&self) -> &[&'static str] {
        &[Texture::NAME, PbrMaterial::NAME, Name::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME]
    }
    /// A changed mesh's file link.
    fn removes(&self) -> &[&'static str] {
        &[MeshFile::NAME]
    }
    fn approximates(&self) -> &'static str {
        "slots re-sampled bilinearly into atlases at a texel density worked from the slots' textures, normal maps \
         turned between the sets' frames per triangle"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // every primitive planned before anything changes
        let mut plans: Vec<(Entity, usize, Plan)> = Vec::new();
        for (e, mesh) in w.iter::<Mesh>() {
            for (i, p) in mesh.primitives().iter().enumerate() {
                if let Some(plan) = self.plan(w, e, i, p, log) {
                    plans.push((e, i, plan));
                }
            }
        }
        let mut folded: BTreeMap<(Entity, [u8; 3]), Entity> = BTreeMap::new();
        let mut by_mesh: BTreeMap<Entity, Vec<(usize, Plan)>> = BTreeMap::new();
        for (e, i, plan) in plans {
            by_mesh.entry(e).or_default().push((i, plan));
        }
        for (e, edits) in by_mesh {
            let mut primitives: Vec<Primitive> = w
                .get::<Mesh>(e)
                .ok_or("a planned mesh is gone")?
                .primitives()
                .to_vec();
            let original = primitives.len();
            let mut from: Vec<usize> = (0..original).collect();
            let mut touched: Vec<usize> = Vec::new();
            let mut spawned: Vec<Entity> = Vec::new();
            let mut made_folds: Vec<(Entity, [u8; 3])> = Vec::new();
            let mut counts: Vec<(&str, u64)> = Vec::new();
            let mut approximations: Vec<(&str, u64)> = Vec::new();
            // the last primitive first, so a split does not move the ones still to come
            for (i, plan) in edits.into_iter().rev() {
                match plan {
                    Plan::Fold {
                        material,
                        colour,
                        loss,
                    } => {
                        approximations.extend(loss);
                        let copy = match folded.get(&(material, colour)) {
                            Some(&c) => c,
                            None => {
                                let c = w.duplicate(material).ok_or("a material that is not alive")?;
                                spawned.push(c);
                                made_folds.push((material, colour));
                                let linear = colour.map(|b| srgb8_to_linear(b) as f32);
                                let m = w
                                    .get_mut::<PbrMaterial>(c)
                                    .ok_or("a material that is not lowered")?;
                                m.emissive_texture = None;
                                for (k, f) in linear.iter().enumerate() {
                                    m.emissive[k] = (m.emissive[k] * f).clamp(0.0, 1.0);
                                }
                                folded.insert((material, colour), c);
                                counts.push((
                                    "emissive materials made with the emission folded into a factor",
                                    1,
                                ));
                                c
                            }
                        };
                        primitives[i].material = Some(copy);
                        touched.push(i);
                        counts.push(("primitives with their emission folded into a factor", 1));
                    }
                    Plan::Rebase {
                        material,
                        window: (origin, extent),
                        size,
                        clamped,
                        groups,
                        loss,
                    } => {
                        approximations.extend(loss);
                        let whole = primitives[i].clone();
                        let indices = triangles(&whole).ok_or("a planned primitive that is not triangles")?;
                        let mut pieces: Vec<Primitive> = Vec::with_capacity(groups.len());
                        for (g, group) in groups.iter().enumerate() {
                            let mut made: Vec<(TextureSlot, Entity, Filters)> = Vec::new();
                            for (slot, texture, filters) in &group.textures {
                                let t = w.spawn();
                                w.insert(t, texture.clone());
                                w.insert(
                                    t,
                                    Name(format!("{} rebased onto texcoord {}", slot.name(), self.set)),
                                );
                                spawned.push(t);
                                made.push((*slot, t, *filters));
                                counts.push(("atlas textures made", 1));
                                counts.push(("atlas texels", u64::from(size.0) * u64::from(size.1)));
                                approximations.push(("atlas textures re-sampled bilinearly", 1));
                            }
                            let suffix = if g == 0 {
                                format!(" (rebased onto texcoord {})", self.set)
                            } else {
                                format!(" (rebased onto texcoord {}, group {})", self.set, g + 1)
                            };
                            let copy = material_copy(w, material, &made, &suffix)?;
                            spawned.push(copy);
                            counts.push(("materials made for rebased primitives", 1));
                            let mut piece = if groups.len() == 1 {
                                whole.clone()
                            } else {
                                let picked: Vec<u32> =
                                    group.triangles.iter().flat_map(|&t| indices[t]).collect();
                                subset_primitive(&whole, &picked).map_err(|e| e.to_string())?
                            };
                            let own: Vec<[f32; 2]> = uv_stream(&piece, self.set)
                                .ok_or("a planned primitive without its coordinates")?
                                .iter()
                                .map(|c| [(c[0] - origin[0]) / extent[0], (c[1] - origin[1]) / extent[1]])
                                .collect();
                            piece.streams.insert(Semantic::TexCoord(0), Stream::F32x2(own));
                            piece.material = Some(copy);
                            counts.push(("triangles rebased", group.triangles.len() as u64));
                            pieces.push(piece);
                        }
                        counts.push(("primitives rebased", 1));
                        approximations.push((
                            "primitives whose first coordinate set is replaced by the rebased set's window coordinates",
                            1,
                        ));
                        if clamped {
                            approximations.push((
                                "primitives whose atlases are smaller than their textures' density asks: clamped to the limit",
                                1,
                            ));
                        }
                        let n = pieces.len();
                        if n > 1 {
                            counts.push(("primitives split into groups of triangles that do not overlap", 1));
                            counts.push(("primitives made by those splits", n as u64));
                        }
                        primitives.splice(i..=i, pieces);
                        from.splice(i..=i, std::iter::repeat_n(i, n));
                        touched.push(i);
                    }
                }
            }
            let result = if primitives.len() != original {
                w.rebuild_primitives(e, |_| Ok((primitives, from)))
            } else {
                w.edit_mesh(e, |mesh| {
                    for &k in &touched {
                        let mut target = mesh.primitive_mut(k).ok_or("a primitive that is gone")?;
                        let fields = target.fields();
                        *fields.streams = std::mem::take(&mut primitives[k].streams);
                        *fields.material = primitives[k].material;
                    }
                    Ok(None)
                })
                .map(|_| ())
            };
            match result {
                Ok(()) => {
                    w.remove::<MeshFile>(e);
                    for (what, n) in counts {
                        log.count(what, n);
                    }
                    for (what, n) in approximations {
                        log.approximate(what, n);
                    }
                }
                Err(why) => {
                    for t in spawned {
                        w.despawn(t);
                    }
                    for key in made_folds {
                        folded.remove(&key);
                    }
                    log.count(&format!("meshes not rebased: {why}"), 1);
                }
            }
        }
        Ok(())
    }
}
