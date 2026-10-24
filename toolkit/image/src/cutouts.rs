//! `clip-alpha-cutouts{resolution=N}` (`resolution` required): every primitive drawn with an alpha-`Mask`
//! `pbr.material` has the parts of its triangles whose alpha is below the material's cutoff cut away, and the material
//! is made `Opaque`, for a destination that draws with one opaque material (straw, hair cards and frayed cloth become
//! geometry). Run it after `split-uv-tiles` and `bake-texture-transforms`.
//!
//! **Which primitives.** Those drawn with a material (its own or a per-placement variant) whose alpha mode is
//! `Mask{cutoff}`; all the masked materials one primitive is drawn with must be masked alike (same texture, coordinate
//! set, wraps, cutoff and base-colour alpha: one geometry serves them all). Refused, naming it: a primitive that is not a
//! triangle list or has no two-float `TexCoord` of the base-colour texture's set, one whose coordinates leave 0-1
//! (`split-uv-tiles` first), a texture transform (`bake-texture-transforms` first), a texture with no pixels or not
//! exact in 8-bit RGBA (`lower-textures-rgba8` first), and a material with no texture whose alpha is below the cutoff.
//! The vertex colour's alpha is not applied (counted as an approximation where some is below 255).
//!
//! **Method.** The cut-out is the iso-contour of the filtered alpha at the cutoff, as polygons: a mask grid of at most
//! `resolution` texels on its longer side (the texels themselves when the texture is not larger, else their box mean),
//! its contour by marching squares, solid areas merged into rectangles, each triangle clipped in UV against them
//! ([`crate::cutout_field`]). Triangle count grows with `resolution` about as the contour's length in cells, and a
//! strand narrower than a grid texel whose mean alpha stays under the cutoff is lost. A rectangle of merged cells meets
//! a contour cell beside it with a T-junction.
//!
//! **Vertices.** A new vertex lies in one source triangle, so every stream (skin weights and morph deltas included) is
//! interpolated as `split-uv-tiles` does, by the same code ([`primase_meshops::point_blend`]). Pieces meeting at a point
//! share its vertex (barycentric weights equal to 2^-20). Winding is kept; vertices are numbered in first use.
//!
//! **Nothing drawn.** A triangle with none of its area drawn is dropped; a primitive left with no triangle is left out
//! of its mesh (a mesh left with no primitive loses its `core.mesh`), declared as dropped.
//!
//! Reported: primitives clipped, triangles and vertices before and after, the triangles lost; as approximations, the
//! contour at the grid's resolution, skinned vertices with interpolated weights and blended normals that cancel.

use std::collections::{BTreeMap, btree_map::Entry};

use primase_core::ledger::Measure;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{AlphaMode, PbrMaterial, UvTransform};
use primase_core::skin::Skin;
use primase_core::{
    Component, Entity, Influences, Mesh, MeshFile, Name, Primitive, Semantic, Stream, Texture, Topology,
    World,
};
use primase_meshops::point_blend::{
    Bary, Corner, Key, Replacement, blend_streams, describe, drawn_with, replace_primitives,
};

use crate::cutout_field::{Clip, Field, address, downsample, effective};
use crate::{Samples, rgba8_exact, texture_level_rgba8};

/// Barycentric weights this close (2^-20) are one point.
const WELD_BITS: u32 = 20;

/// See the module documentation. Made by [`ClipAlphaCutouts::new`]; from options by [`CLIP_ALPHA_CUTOUTS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipAlphaCutouts {
    resolution: u32,
}

impl ClipAlphaCutouts {
    /// Clip with a mask grid of at most `resolution` texels on its longer side (at least 2).
    pub fn new(resolution: u32) -> Result<ClipAlphaCutouts, String> {
        if resolution < 2 {
            return Err(format!("resolution={resolution}: at least 2"));
        }
        Ok(ClipAlphaCutouts { resolution })
    }

    pub fn resolution(&self) -> u32 {
        self.resolution
    }

    fn from_given(g: &Given) -> Result<ClipAlphaCutouts, String> {
        let v = g.required("resolution");
        ClipAlphaCutouts::new(v.parse().map_err(|_| format!("resolution={v} is not a number"))?)
    }
}

/// `clip-alpha-cutouts`' declaration ([`primase_core::pass::PassRegistry::add`]): the grid's resolution is the
/// program's trade of triangles against fine strands, no default.
pub const CLIP_ALPHA_CUTOUTS: PassSpec = PassSpec {
    name: "clip-alpha-cutouts",
    options: &[OptionSpec::required(
        "resolution",
        "256",
        "the longer side of the mask grid, in texels, at least 2",
    )],
    make: |g| Ok(Box::new(ClipAlphaCutouts::from_given(g)?)),
};

/// What a masked material tests: compared by bits, so equal means identical.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Spec {
    texture: Option<Entity>,
    texcoord: u8,
    repeat: [bool; 2],
    cutoff: u32,
    factor: u32,
}

impl Spec {
    fn cutoff(&self) -> f32 {
        f32::from_bits(self.cutoff)
    }

    fn factor(&self) -> f32 {
        f32::from_bits(self.factor)
    }
}

/// The spec of a masked material, `None` when it is not masked.
fn spec_of(m: &PbrMaterial, what: &str) -> Result<Option<Spec>, String> {
    let AlphaMode::Mask { cutoff } = m.alpha_mode else {
        return Ok(None);
    };
    if !cutoff.is_finite() || !m.base_color[3].is_finite() {
        return Err(format!("{what}: the cutoff or base-colour alpha is not finite"));
    }
    let r = m.base_color_texture;
    if r.is_some_and(|r| r.transform.is_some_and(|t| t != UvTransform::IDENTITY)) {
        return Err(format!(
            "{what}: the base-colour texture has a transform (bake-texture-transforms first)"
        ));
    }
    Ok(Some(Spec {
        texture: r.map(|r| r.texture),
        texcoord: r.map_or(0, |r| r.texcoord),
        repeat: r.map_or([false; 2], |r| {
            r.wrap.map(|w| address(w) == crate::sampling::Address::Wrap)
        }),
        cutoff: cutoff.to_bits(),
        factor: m.base_color[3].to_bits(),
    }))
}

/// The field of a masked material, `None` for one every point of which is drawn (no texture, base-colour alpha at or
/// above the cutoff).
fn build_field(w: &World, spec: Spec, resolution: usize, what: &str) -> Result<Option<Field>, String> {
    let Some(t) = spec.texture else {
        if effective(spec.factor(), 255.0) < spec.cutoff() {
            return Err(format!(
                "{what}: no texture and a base-colour alpha below the cutoff, so it draws nothing"
            ));
        }
        return Ok(None);
    };
    let tex = w
        .get::<Texture>(t)
        .ok_or_else(|| format!("{what}: texture {t:?} has no pixels (load it first)"))?;
    if !rgba8_exact(tex.format) {
        return Err(format!(
            "{what}: {:?} is not exact in 8-bit RGBA (lower-textures-rgba8 first)",
            tex.format
        ));
    }
    let image = texture_level_rgba8(tex, 0).map_err(|e| format!("{what}: {e}"))?;
    let Samples::U8(px) = image.samples else {
        return Err(format!("{what}: decoded to something other than 8-bit"));
    };
    let alpha: Vec<u8> = px.as_chunks::<4>().0.iter().map(|p| p[3]).collect();
    let (gw, gh, grid) = downsample(&alpha, image.width as usize, image.height as usize, resolution);
    let values = grid.into_iter().map(|a| effective(spec.factor(), a)).collect();
    Ok(Some(Field::new(
        gw,
        gh,
        values,
        spec.cutoff(),
        spec.repeat.map(|r| {
            if r {
                crate::sampling::Address::Wrap
            } else {
                crate::sampling::Address::Clamp
            }
        }),
    )))
}

/// New vertices in first-use order, one per point (barycentric weights agreeing to 2^-20).
#[derive(Default)]
struct Welded {
    index: BTreeMap<Key, u32>,
    points: Vec<Bary>,
}

impl Welded {
    fn get(&mut self, c: &Corner) -> u32 {
        let points = &mut self.points;
        *self.index.entry(c.at.welded_key(WELD_BITS)).or_insert_with(|| {
            points.push(c.at);
            (points.len() - 1) as u32
        })
    }
}

/// A primitive to clip, checked.
struct Job {
    entity: Entity,
    primitive: usize,
    spec: Spec,
    /// The materials it is drawn with.
    materials: Vec<Entity>,
}

/// What clipping one primitive made.
struct Clipped {
    replacement: Replacement,
    triangles_before: u64,
    vertices_before: u64,
    skin_interpolated: u64,
    normals_cancelled: u64,
}

fn clip_primitive(
    j: &Job,
    p: &Primitive,
    field: Option<&Field>,
    influences: Option<Influences>,
    what: &str,
) -> Result<Clipped, String> {
    let Some(Stream::F32x2(uv)) = p.streams.get(&Semantic::TexCoord(j.spec.texcoord)) else {
        unreachable!("checked when the job was made");
    };
    let n = p.vertex_count();
    let indices: Vec<u32> = p.indices.clone().unwrap_or_else(|| (0..n as u32).collect());
    let mut welded = Welded::default();
    let mut tris: Vec<u32> = Vec::new();
    let mut emit = |poly: &[Corner], welded: &mut Welded| {
        let ids: Vec<u32> = poly.iter().map(|c| welded.get(c)).collect();
        for k in 1..ids.len() - 1 {
            if ids[0] != ids[k] && ids[k] != ids[k + 1] && ids[0] != ids[k + 1] {
                tris.extend([ids[0], ids[k], ids[k + 1]]);
            }
        }
    };
    for tri in indices.as_chunks::<3>().0 {
        let corners = tri.map(|v| Corner {
            at: Bary::corner(v),
            uv: uv[v as usize].map(f64::from),
        });
        match field.map_or(Clip::Whole, |f| f.clip(corners)) {
            Clip::Whole => emit(&corners, &mut welded),
            Clip::Nothing => {}
            Clip::Pieces(pieces) => pieces.iter().for_each(|piece| emit(piece, &mut welded)),
        }
    }
    let blends: Vec<_> = welded.points.iter().map(Bary::blend).collect();
    let made = blend_streams(p, &blends, influences, what)?;
    Ok(Clipped {
        triangles_before: (indices.len() / 3) as u64,
        vertices_before: n as u64,
        skin_interpolated: made.skin_interpolated,
        normals_cancelled: made.normals_cancelled,
        replacement: Replacement {
            entity: j.entity,
            primitive: j.primitive,
            streams: made.streams,
            indices: tris,
            blends,
            influences: made.influences,
        },
    })
}

fn name_of(w: &World, e: Entity) -> String {
    w.get::<Name>(e).map_or_else(|| format!("{e:?}"), |n| n.0.clone())
}

/// The jobs: every primitive drawn with masked materials, checked.
fn plan(w: &World, log: &mut PassLog) -> Result<Vec<Job>, String> {
    let mut jobs = Vec::new();
    for (e, mesh) in w.iter::<Mesh>() {
        for (i, p) in mesh.primitives().iter().enumerate() {
            let what = describe(w, e, i);
            let materials = drawn_with(w, e, i, p);
            let mut specs = Vec::new();
            for &m in &materials {
                let mat = w.get::<PbrMaterial>(m).ok_or_else(|| {
                    format!("{what}: material {m:?} has no pbr.material, so its alpha mode is not known")
                })?;
                specs.push(spec_of(mat, &format!("{what}: material {}", name_of(w, m)))?);
            }
            let Some(spec) = specs.iter().flatten().next().copied() else {
                continue;
            };
            if specs.iter().any(|s| *s != Some(spec)) {
                return Err(format!(
                    "{what}: drawn with materials that are not masked alike (one geometry serves them all)"
                ));
            }
            if p.topology != Topology::Triangles {
                return Err(format!(
                    "{what}: {:?}, not triangles, drawn with a masked material",
                    p.topology
                ));
            }
            if let Some(Stream::U8x4(c)) = p.streams.get(&Semantic::Color(0))
                && c.iter().any(|c| c[3] < 255)
            {
                log.approximate(
                    "masked primitives with a vertex colour alpha the cut-out did not see",
                    1,
                );
            }
            let Some(Stream::F32x2(uv)) = p.streams.get(&Semantic::TexCoord(spec.texcoord)) else {
                return Err(format!("{what}: no TexCoord({}) of two floats", spec.texcoord));
            };
            let drawn: Vec<u32> = p
                .indices
                .clone()
                .unwrap_or_else(|| (0..uv.len() as u32).collect());
            for &v in &drawn {
                let c = uv
                    .get(v as usize)
                    .ok_or_else(|| format!("{what}: index {v} outside its vertices"))?;
                if !c
                    .iter()
                    .all(|x| x.is_finite() && (-1e-6..=1.0 + 1e-6).contains(x))
                {
                    return Err(format!(
                        "{what}: TexCoord({}) leaves 0-1 (split-uv-tiles first)",
                        spec.texcoord
                    ));
                }
            }
            jobs.push(Job {
                entity: e,
                primitive: i,
                spec,
                materials,
            });
        }
    }
    Ok(jobs)
}

impl Pass for ClipAlphaCutouts {
    fn name(&self) -> &'static str {
        "clip-alpha-cutouts"
    }
    fn options(&self) -> Options {
        Options::from([("resolution".to_owned(), self.resolution.to_string())])
    }
    /// A mesh left with no primitive goes with its per-primitive components, whichever dialects they are.
    fn removes_by_log(&self) -> bool {
        true
    }
    fn requires(&self) -> &[&'static str] {
        &[Mesh::NAME, PbrMaterial::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME, "core.morphs", Skin::NAME, PbrMaterial::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[MeshFile::NAME]
    }
    /// The file links it removes are a side effect: the changed meshes no longer match their files.
    fn legalizes(&self) -> &[&'static str] {
        &[]
    }
    fn approximates(&self) -> &'static str {
        "the cut-out's contour at the mask grid's resolution; skin weights interpolated at new vertices; normals that \
         cancel keep the nearest corner's"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        if let Some(e) = w.iter::<MeshFile>().map(|(e, _)| e).find(|&e| !w.has::<Mesh>(e)) {
            return Err(format!("{e:?}: geometry still in its file (load it first)"));
        }
        // everything is planned and checked before anything changes
        let jobs = plan(w, log)?;
        let mut fields: BTreeMap<Spec, Option<Field>> = BTreeMap::new();
        for j in &jobs {
            if let Entry::Vacant(v) = fields.entry(j.spec) {
                let what = format!("material {}", name_of(w, j.materials[0]));
                let field = build_field(w, j.spec, self.resolution as usize, &what)?;
                v.insert(field);
            }
        }
        let (mut replacements, mut emptied) = (Vec::new(), BTreeMap::<Entity, Vec<usize>>::new());
        let mut lost_triangles = 0u64;
        for j in &jobs {
            let p = &w.get::<Mesh>(j.entity).expect("listed").primitives()[j.primitive];
            let influences = w
                .get::<Skin>(j.entity)
                .and_then(|s| s.influences.get(j.primitive).copied().flatten());
            let what = describe(w, j.entity, j.primitive);
            let c = clip_primitive(j, p, fields[&j.spec].as_ref(), influences, &what)?;
            let after = c.replacement.indices.len() as u64 / 3;
            log.count("primitives clipped", 1);
            log.count("triangles before", c.triangles_before);
            log.count("triangles after", after);
            log.count("vertices before", c.vertices_before);
            log.count("vertices after", c.replacement.blends.len() as u64);
            if after == 0 {
                // nothing of it is drawn: the primitive goes (its triangles with it)
                log.count(&format!("primitives with nothing drawn, left out: {what}"), 1);
                emptied.entry(j.entity).or_default().push(j.primitive);
                continue;
            }
            lost_triangles += c.triangles_before.saturating_sub(after);
            log.approximate("skinned vertices with interpolated weights", c.skin_interpolated);
            log.approximate(
                "blended normals that cancel, the nearest corner's kept",
                c.normals_cancelled,
            );
            replacements.push(c.replacement);
        }
        replace_primitives(w, replacements)?;
        log.dropped(
            Measure::Triangles,
            "triangles of masked cards that the cut-out leaves fewer of (net per primitive)",
            lost_triangles,
        );
        let mut left_out = 0u64;
        for (e, gone) in emptied {
            left_out += gone.len() as u64;
            let of = w.get::<Mesh>(e).expect("listed").primitives().len();
            if gone.len() == of {
                for name in w.remove_mesh(e)? {
                    log.record_removed(name);
                }
                log.count("meshes left with no primitive, removed", 1);
            } else {
                w.rebuild_primitives(e, |mesh| {
                    let from: Vec<usize> = (0..mesh.primitives().len())
                        .filter(|i| !gone.contains(i))
                        .collect();
                    Ok((from.iter().map(|&i| mesh.primitives()[i].clone()).collect(), from))
                })?;
                w.remove::<MeshFile>(e);
            }
        }
        log.dropped(
            Measure::Primitives,
            "masked primitives the cut-out leaves nothing of",
            left_out,
        );
        let mut made_opaque: Vec<Entity> = jobs.iter().flat_map(|j| j.materials.iter().copied()).collect();
        made_opaque.sort();
        made_opaque.dedup();
        for m in made_opaque {
            w.get_mut::<PbrMaterial>(m).expect("checked").alpha_mode = AlphaMode::Opaque;
            log.count(&format!("material {}: masked made opaque", name_of(w, m)), 1);
        }
        let unused = w
            .iter::<PbrMaterial>()
            .filter(|(_, m)| matches!(m.alpha_mode, AlphaMode::Mask { .. }))
            .count();
        log.count(
            "masked materials no primitive draws, left as they are",
            unused as u64,
        );
        Ok(())
    }
}
