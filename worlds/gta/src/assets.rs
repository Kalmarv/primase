//! Prototypes' drawables and textures into the world, for the prototypes placements still use.
//!
//! - A prototype whose archetype is a drawable (asset type 2: the live ydr), a fragment (1: the live yft's main
//!   drawable) or a drawable dictionary's entry (3: the archetype's live ydd, entry by asset name) gets `core.mesh` from
//!   that drawable's high LOD: every model's geometries in order, one primitive each, streams as stored (frame
//!   converted), 16-bit indices widened;
//!   [`GtaDrawable`] with the drawable's box and sphere as stored and the coarser LODs as meshes (until core has a
//!   representation for alternative LOD meshes).
//! - A material per (drawable, shader) with [`GtaShader`]: every parameter by stored name, textures resolved the way
//!   the game looks them up: the drawable's own dictionary, then the archetype's texture dictionary, then that
//!   dictionary's parents ([`TextureParents`]), then the resident detail dictionary ([`RESIDENT`]).
//! - A texture entity per (dictionary, texture): `core.texture` (blocks as stored, levels split, colour space from the
//!   format; role `Other` until a pass knows which sampler reads it), `core.name`, `core.source`.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;

use primase_core::{
    Entity, Instance, Mesh, Name, Primitive, Semantic, Source, Stream, TexRole, Texture as CoreTexture,
    Topology, World,
};

use crate::archive_set::ArchiveSet;
use crate::components::{GtaArchetype, GtaDrawable, GtaParam, GtaShader};
use crate::drawable::{Drawable, DrawableDictionary, Fragment, Geometry, Model, ParamValue};
use crate::gtxd::TextureParents;
use crate::keys::Keys;
use crate::le;
use crate::region::GTA;
use crate::texture::{Texture, TextureDictionary};
use crate::{Result, invalid};

/// The resident detail dictionary searched last (`x64a.rpf\textures\mapdetail.ytd`, the global textures folder): detail
/// maps (`env_*`) are bound by name and no parent chain reaches it. In Legion it is the only dictionary holding all four
/// detail textures the chains missed (188 others hold scattered copies); CodeWalker also keeps it resident.
const RESIDENT: &str = "mapdetail";

/// What attaching counted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AssetReport {
    pub drawables: usize,
    pub geometries: usize,
    pub materials: usize,
    pub textures: usize,
    /// Texture parameters naming a texture none of the dictionaries in the lookup holds, by texture name.
    pub textures_not_found: BTreeMap<String, usize>,
    /// Of `drawables`, those that are a fragment's main drawable and a drawable dictionary's entry.
    pub fragments: usize,
    pub dictionary_entries: usize,
    /// High-LOD models placed by their bone's pose (a fragment's models).
    pub posed_models: usize,
    /// Of those, the ones whose pose is not a rotation (beyond 1e-4) but has an inverse: normals turned by its inverse
    /// transpose, tangents by it, both kept at their stored length, handedness flipped by a mirror.
    pub posed_models_not_rigid: usize,
    /// Of those, the ones whose pose has no inverse (it flattens the model): normals and tangents left out.
    pub posed_models_singular: usize,
    /// Prototypes whose archetype names no live file or entry.
    pub asset_missing: usize,
    /// Vertex streams left out because the file stores values that are not finite (NaN, infinity), by semantic: one
    /// per primitive of any LOD. Such a value is unknown, and core holds finite streams only.
    pub non_finite_streams: BTreeMap<String, usize>,
}

struct Ctx<'a> {
    set: &'a ArchiveSet,
    keys: &'a Keys,
    parents: TextureParents,
    /// Texture dictionaries by name hash (`None`: no live ytd of that name).
    dicts: BTreeMap<u32, Option<(String, TextureDictionary)>>,
    /// Texture entities by (dictionary key, texture name hash).
    textures: BTreeMap<(String, u32), Entity>,
    /// Drawable dictionaries read, by name hash.
    ydds: BTreeMap<u32, DrawableDictionary>,
    report: AssetReport,
}

/// A model's transform in GTA's frame: rows `[x, y, z, translation]`.
type Pose = [[f32; 4]; 3];

/// How far from orthonormal (the largest entry of `M M^T - I`, `M` a pose's 3x3 part) a pose may be and still turn
/// normals and tangents as it turns positions: within it, `M`'s inverse transpose equals `M` to that order (an
/// approximation). Every pose a model of the install uses is either within 1e-4 or singular (census).
const RIGID: f64 = 1e-4;

/// How a pose turns directions.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Turn {
    /// A rotation (orthonormal, determinant 1): normals and tangents as positions, without the translation.
    Rigid,
    /// Any other invertible 3x3 part: normals by its inverse transpose (rows), tangents by it, both kept at their
    /// stored length; a negative determinant (a mirror) flips the tangent's handedness.
    General {
        inverse_transpose: [[f64; 3]; 3],
        mirror: bool,
    },
    /// No inverse (determinant 0): the model is flattened, and its normals and tangents are not defined.
    Singular,
}

/// A model's pose and how it turns directions.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Placing {
    pose: Pose,
    turn: Turn,
}

impl Placing {
    fn new(pose: Pose) -> Placing {
        let m: [[f64; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| f64::from(pose[i][j])));
        // cofactors: C[i][j] = m[i+1][j+1] m[i+2][j+2] - m[i+1][j+2] m[i+2][j+1] (indices mod 3); M^-T = C / det
        let c: [[f64; 3]; 3] = std::array::from_fn(|i| {
            std::array::from_fn(|j| {
                let (a, b, x, y) = ((i + 1) % 3, (i + 2) % 3, (j + 1) % 3, (j + 2) % 3);
                m[a][x] * m[b][y] - m[a][y] * m[b][x]
            })
        });
        let det: f64 = (0..3).map(|j| m[0][j] * c[0][j]).sum();
        let scale = m.iter().flatten().fold(0f64, |a, &v| a.max(v.abs()));
        let orth = (0..3)
            .flat_map(|i| (0..3).map(move |j| (i, j)))
            .map(|(i, j)| {
                ((0..3).map(|k| m[i][k] * m[j][k]).sum::<f64>() - f64::from(u8::from(i == j))).abs()
            })
            .fold(0f64, f64::max);
        let turn = if !det.is_finite() || det.abs() <= 1e-9 * scale.powi(3) {
            Turn::Singular
        } else if det > 0.0 && orth <= RIGID {
            Turn::Rigid
        } else {
            Turn::General {
                inverse_transpose: c.map(|row| row.map(|v| v / det)),
                mirror: det < 0.0,
            }
        };
        Placing { pose, turn }
    }

    /// A position.
    fn point(&self, v: [f32; 3]) -> [f32; 3] {
        transform(&self.pose, v, 1.0)
    }

    /// A normal; `None` under a singular pose.
    fn normal(&self, n: [f32; 3]) -> Option<[f32; 3]> {
        match self.turn {
            Turn::Rigid => Some(transform(&self.pose, n, 0.0)),
            Turn::General {
                inverse_transpose, ..
            } => Some(keep_length(n, inverse_transpose.map(|r| dot(r, n)))),
            Turn::Singular => None,
        }
    }

    /// A tangent with its handedness in w; `None` under a singular pose.
    fn tangent(&self, t: [f32; 4]) -> Option<[f32; 4]> {
        let xyz = [t[0], t[1], t[2]];
        match self.turn {
            Turn::Rigid => {
                let d = transform(&self.pose, xyz, 0.0);
                Some([d[0], d[1], d[2], t[3]])
            }
            Turn::General { mirror, .. } => {
                let m = self
                    .pose
                    .map(|r| [f64::from(r[0]), f64::from(r[1]), f64::from(r[2])]);
                let d = keep_length(xyz, m.map(|r| dot(r, xyz)));
                Some([d[0], d[1], d[2], if mirror { -t[3] } else { t[3] }])
            }
            Turn::Singular => None,
        }
    }
}

fn dot(r: [f64; 3], v: [f32; 3]) -> f64 {
    r[0] * f64::from(v[0]) + r[1] * f64::from(v[1]) + r[2] * f64::from(v[2])
}

/// `turned` scaled to the length of `stored` (as it is when it has no length).
fn keep_length(stored: [f32; 3], turned: [f64; 3]) -> [f32; 3] {
    let len = |v: [f64; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let (a, b) = (len(stored.map(f64::from)), len(turned));
    let k = if b > 0.0 { a / b } else { 1.0 };
    turned.map(|v| (v * k) as f32)
}

fn transform(r: &Pose, v: [f32; 3], w: f64) -> [f32; 3] {
    r.map(|row| {
        (f64::from(row[0]) * f64::from(v[0])
            + f64::from(row[1]) * f64::from(v[1])
            + f64::from(row[2]) * f64::from(v[2])
            + f64::from(row[3]) * w) as f32
    })
}

/// One vertex element as a core stream, placed by the model's pose when it has one; `None` for normals and tangents
/// under a singular pose.
fn stream(
    g: &Geometry,
    e: crate::drawable::Element,
    t: Option<&Placing>,
) -> Result<Option<(Semantic, Stream)>> {
    let bytes = g.element_bytes(e);
    let f3 = |b: &[u8]| -> Vec<[f32; 3]> { b.as_chunks::<12>().0.iter().map(|c| le::vec3(c, 0)).collect() };
    let fmt = e.format.0;
    Ok(Some(match (e.slot, fmt) {
        (0, 0x06) => (
            Semantic::Position,
            Stream::F32x3(
                f3(&bytes)
                    .into_iter()
                    .map(|p| GTA.point(t.map_or(p, |t| t.point(p))))
                    .collect(),
            ),
        ),
        (4, 0x06) => {
            let normals: Option<Vec<[f32; 3]>> = f3(&bytes)
                .into_iter()
                .map(|n| t.map_or(Some(n), |t| t.normal(n)).map(|n| GTA.direction(n)))
                .collect();
            let Some(normals) = normals else { return Ok(None) };
            (Semantic::Normal, Stream::F32x3(normals))
        }
        (8, 0x02) => {
            let tangents: Option<Vec<[f32; 4]>> = bytes
                .as_chunks::<16>()
                .0
                .iter()
                .map(|c| {
                    let v = le::vec4(c, 0);
                    t.map_or(Some(v), |t| t.tangent(v)).map(|v| {
                        let d = GTA.direction([v[0], v[1], v[2]]);
                        [d[0], d[1], d[2], v[3]]
                    })
                })
                .collect();
            let Some(tangents) = tangents else { return Ok(None) };
            (Semantic::Tangent, Stream::F32x4(tangents))
        }
        (16, 0x1C) => (Semantic::Weights(0), Stream::U8x4(u8x4(&bytes))),
        (20, 0x1E) => (Semantic::Joints(0), Stream::U8x4(u8x4(&bytes))),
        (24..=27, 0x1C) => (Semantic::Color(e.slot - 24), Stream::U8x4(u8x4(&bytes))),
        (28..=51, 0x10) => (
            Semantic::TexCoord(e.slot - 28),
            Stream::F32x2(
                bytes
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|c| [le::f32(c, 0), le::f32(c, 4)])
                    .collect(),
            ),
        ),
        _ => {
            return Err(invalid(
                "drawable",
                0,
                format!("{} in format {fmt:#x} has no core stream", e.semantic()),
            ));
        }
    }))
}

fn u8x4(b: &[u8]) -> Vec<[u8; 4]> {
    b.as_chunks::<4>().0.to_vec()
}

/// Each model's transform: its bone's pose (a fragment's), none for skinned models (bound to bone weights) or without a
/// pose. As the game draws a fragment at rest; only for drawables with a skeleton.
fn model_poses(d: &Drawable, models: &[Model], pose: &[Pose]) -> Vec<Option<Placing>> {
    models
        .iter()
        .map(|m| {
            let (bone, skinned) = (
                (m.skeleton_binding >> 24) & 0xFF,
                (m.skeleton_binding >> 8) & 0xFF != 0,
            );
            if !d.skeleton || skinned {
                None
            } else {
                pose.get(bone as usize).map(|&p| Placing::new(p))
            }
        })
        .collect()
}

/// Whether every value of a float stream is finite (integer streams always are).
fn finite(s: &Stream) -> bool {
    match s {
        Stream::F32x2(v) => v.iter().flatten().all(|x| x.is_finite()),
        Stream::F32x3(v) => v.iter().flatten().all(|x| x.is_finite()),
        Stream::F32x4(v) => v.iter().flatten().all(|x| x.is_finite()),
        Stream::U8x4(_) | Stream::U16x4(_) => true,
    }
}

/// A model list as a mesh, each geometry a primitive with its shader's material. A stream other than the positions
/// holding a value that is not finite is left out and counted in `non_finite`; such positions are an error.
fn mesh(
    path: &str,
    models: &[Model],
    materials: &[Entity],
    poses: &[Option<Placing>],
    non_finite: &mut BTreeMap<String, usize>,
) -> Result<Mesh> {
    let mut primitives = Vec::new();
    for (m, pose) in models.iter().zip(poses) {
        for g in &m.geometries {
            let mut streams = BTreeMap::new();
            for &e in &g.elements {
                let Some((sem, s)) = stream(g, e, pose.as_ref())? else {
                    continue;
                };
                if !finite(&s) {
                    if sem == Semantic::Position {
                        return Err(invalid(
                            "drawable",
                            0,
                            format!("{path}: positions that are not finite"),
                        ));
                    }
                    *non_finite.entry(format!("{sem:?}")).or_default() += 1;
                    continue;
                }
                streams.insert(sem, s);
            }
            primitives.push(Primitive {
                topology: Topology::Triangles,
                streams,
                indices: Some(g.indices.iter().map(|&i| u32::from(i)).collect()),
                material: materials.get(usize::from(g.shader)).copied(),
            });
        }
    }
    Ok(Mesh { primitives })
}

impl Ctx<'_> {
    fn decode(&self, f: crate::archive_set::FileRef) -> Result<crate::resource::Resource> {
        let a = &self.set.install.archives[f.archive];
        a.resource(&mut File::open(&a.file)?, f.entry, self.keys)
    }

    fn dictionary(&mut self, txd: u32) -> Result<Option<&(String, TextureDictionary)>> {
        if !self.dicts.contains_key(&txd) {
            let d = match self.set.tables.get("ytd").and_then(|t| t.get(&txd)).copied() {
                Some(f) => Some((
                    self.set.path(f).to_string(),
                    TextureDictionary::read_resource(&self.decode(f)?)?,
                )),
                None => None,
            };
            self.dicts.insert(txd, d);
        }
        Ok(self.dicts[&txd].as_ref())
    }

    fn texture_entity(&mut self, w: &mut World, source: &str, t: &Texture) -> Result<Entity> {
        let key = (source.to_string(), primase_hash::joaat(t.name.as_bytes()));
        if let Some(&e) = self.textures.get(&key) {
            return Ok(e);
        }
        let (format, color_space) = t
            .format
            .texture()
            .ok_or_else(|| invalid("texture", 0, format!("{}: format {:#x}", t.name, t.format.0)))?;
        let mut levels = Vec::new();
        let mut at = 0usize;
        for level in 0..u32::from(t.levels) {
            let n = format.level_size(
                (u32::from(t.width) >> level).max(1),
                (u32::from(t.height) >> level).max(1),
            ) * usize::from(t.depth);
            levels.push(t.data[at..at + n].to_vec());
            at += n;
        }
        let e = w.spawn();
        w.insert(
            e,
            CoreTexture {
                width: u32::from(t.width),
                height: u32::from(t.height),
                format,
                levels,
                color_space,
                role: TexRole::Other,
                normal: None,
            },
        );
        w.insert(e, Name(t.name.clone()));
        w.insert(e, Source(format!("{source}#{}", t.name)));
        self.textures.insert(key, e);
        self.report.textures += 1;
        Ok(e)
    }

    /// The texture of this name the game finds for a drawable of this archetype.
    fn find_texture(
        &mut self,
        w: &mut World,
        drawable: &Drawable,
        ydr: &str,
        txd: u32,
        name: &str,
    ) -> Result<Option<Entity>> {
        let h = primase_hash::joaat(name.to_lowercase().as_bytes());
        if let Some(own) = &drawable.textures
            && let Some(t) = own
                .hashes
                .iter()
                .position(|&x| x == h)
                .and_then(|i| own.textures[i].as_ref())
        {
            return self.texture_entity(w, ydr, t).map(Some);
        }
        let resident = primase_hash::joaat(RESIDENT.as_bytes());
        for d in std::iter::once(txd)
            .chain(self.parents.chain(txd))
            .chain(std::iter::once(resident))
        {
            let found = self.dictionary(d)?.and_then(|(path, dict)| {
                let i = dict.hashes.iter().position(|&x| x == h)?;
                dict.textures[i].clone().map(|t| (path.clone(), t))
            });
            if let Some((path, t)) = found {
                return self.texture_entity(w, &path, &t).map(Some);
            }
        }
        *self
            .report
            .textures_not_found
            .entry(name.to_lowercase())
            .or_default() += 1;
        Ok(None)
    }
}

/// Loads drawables and textures for every prototype a placement uses.
pub fn attach_assets(w: &mut World, set: &ArchiveSet, keys: &Keys) -> Result<AssetReport> {
    let mut ctx = Ctx {
        set,
        keys,
        parents: TextureParents::read(set, keys)?,
        dicts: BTreeMap::new(),
        textures: BTreeMap::new(),
        ydds: BTreeMap::new(),
        report: AssetReport::default(),
    };
    let used: BTreeSet<Entity> = w.iter::<Instance>().map(|(_, i)| i.prototype).collect();
    for proto in used {
        let Some(a) = w.get::<GtaArchetype>(proto).cloned() else {
            continue;
        };
        let live = |ext: &str, hash: u32| set.tables.get(ext).and_then(|t| t.get(&hash)).copied();
        let mut pose: Vec<Pose> = Vec::new();
        let (path, d) = match a.asset_type {
            // a drawable
            2 => {
                let Some(f) = live("ydr", a.asset_name) else {
                    ctx.report.asset_missing += 1;
                    continue;
                };
                (set.path(f).to_string(), Drawable::read_resource(&ctx.decode(f)?)?)
            }
            // a fragment: its main drawable
            1 => {
                let Some(f) = live("yft", a.asset_name) else {
                    ctx.report.asset_missing += 1;
                    continue;
                };
                let frag = Fragment::read_resource(&ctx.decode(f)?)?;
                let Some(d) = frag.drawable else {
                    ctx.report.asset_missing += 1;
                    continue;
                };
                pose = frag.pose;
                ctx.report.fragments += 1;
                (set.path(f).to_string(), d)
            }
            // an entry of a drawable dictionary
            3 => {
                let Some(f) = live("ydd", a.drawable_dictionary) else {
                    ctx.report.asset_missing += 1;
                    continue;
                };
                if !ctx.ydds.contains_key(&a.drawable_dictionary) {
                    let dict = DrawableDictionary::read_resource(&ctx.decode(f)?)?;
                    ctx.ydds.insert(a.drawable_dictionary, dict);
                }
                let Some(d) = ctx.ydds[&a.drawable_dictionary].get(a.asset_name).cloned() else {
                    ctx.report.asset_missing += 1;
                    continue;
                };
                ctx.report.dictionary_entries += 1;
                (format!("{}#{:#010x}", set.path(f), a.asset_name), d)
            }
            _ => continue,
        };
        let mut materials = Vec::with_capacity(d.shaders.len());
        for (i, s) in d.shaders.iter().enumerate() {
            let mut params = Vec::with_capacity(s.params.len());
            for p in &s.params {
                let v = match &p.value {
                    ParamValue::Texture(None) => GtaParam::Texture {
                        name: None,
                        texture: None,
                    },
                    ParamValue::Texture(Some(n)) => GtaParam::Texture {
                        name: Some(n.clone()),
                        texture: ctx.find_texture(w, &d, &path, a.texture_dictionary, n)?,
                    },
                    ParamValue::Floats(f) => GtaParam::Floats(f.clone()),
                    ParamValue::Sampler(x) => GtaParam::Sampler(*x),
                    ParamValue::Other(x) => GtaParam::Other(*x),
                };
                params.push((p.name, v));
            }
            let m = w.spawn();
            w.insert(
                m,
                GtaShader {
                    drawable: path.clone(),
                    index: i as u32,
                    name: s.name,
                    preset: s.preset,
                    render_bucket: s.render_bucket,
                    bucket_mask: s.bucket_mask,
                    params,
                },
            );
            materials.push(m);
            ctx.report.materials += 1;
        }
        let poses: [Vec<Option<Placing>>; 4] = std::array::from_fn(|i| model_poses(&d, &d.lods[i], &pose));
        for p in poses[0].iter().flatten() {
            ctx.report.posed_models += 1;
            match p.turn {
                Turn::Rigid => {}
                Turn::General { .. } => ctx.report.posed_models_not_rigid += 1,
                Turn::Singular => ctx.report.posed_models_singular += 1,
            }
        }
        let high = mesh(
            &path,
            &d.lods[0],
            &materials,
            &poses[0],
            &mut ctx.report.non_finite_streams,
        )?;
        ctx.report.geometries += high.primitives.len();
        w.insert(proto, high);
        let mut lod = |i: usize| -> Result<Option<Mesh>> {
            if d.lods[i].is_empty() {
                Ok(None)
            } else {
                mesh(
                    &path,
                    &d.lods[i],
                    &materials,
                    &poses[i],
                    &mut ctx.report.non_finite_streams,
                )
                .map(Some)
            }
        };
        let lods = [lod(1)?, lod(2)?, lod(3)?];
        w.insert(
            proto,
            GtaDrawable {
                path,
                bbox: [d.bbox_min, d.bbox_max],
                bounding_sphere: (d.bounding_centre, d.bounding_radius),
                lod_dist: d.lod_dist,
                render_masks: d.render_masks,
                sources: std::array::from_fn(|i| {
                    (d.lods[i].iter().enumerate())
                        .flat_map(|(m, model)| (0..model.geometries.len() as u32).map(move |g| [m as u32, g]))
                        .collect()
                }),
                lods,
            },
        );
        ctx.report.drawables += 1;
    }
    Ok(ctx.report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(m: [[f32; 3]; 3]) -> Pose {
        m.map(|r| [r[0], r[1], r[2], 5.0])
    }

    #[test]
    fn poses_turn_normals_by_their_inverse_transpose() {
        // a rotation of 90 degrees about z: rigid, every direction turns as a position does
        let r = Placing::new(pose([[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]));
        assert_eq!(r.turn, Turn::Rigid);
        assert_eq!(r.normal([1.0, 0.0, 0.0]), Some([0.0, 1.0, 0.0]));
        assert_eq!(r.tangent([1.0, 0.0, 0.0, -1.0]), Some([0.0, 1.0, 0.0, -1.0]));
        assert_eq!(r.point([0.0, 0.0, 0.0]), [5.0, 5.0, 5.0]);
        // a shear: the normal stays perpendicular to the sheared surface's tangents, at its stored length
        let s = Placing::new(pose([[1.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]));
        assert!(matches!(s.turn, Turn::General { mirror: false, .. }));
        // the surface x = 0 (tangents y and z) leans to x = y: its normal becomes (1, -1, 0) / sqrt 2
        let n = s.normal([1.0, 0.0, 0.0]).expect("normal");
        let (t, u) = (
            s.tangent([0.0, 1.0, 0.0, 1.0]).expect("tangent"),
            s.tangent([0.0, 0.0, 1.0, 1.0]).expect("tangent"),
        );
        let h = std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (t[0] - h).abs() < 1e-6 && (t[1] - h).abs() < 1e-6 && t[2] == 0.0 && t[3] == 1.0,
            "{t:?}"
        );
        let d = |a: [f32; 4]| f64::from(a[0] * n[0] + a[1] * n[1] + a[2] * n[2]);
        assert!(
            d(t).abs() < 1e-6 && d(u).abs() < 1e-6,
            "{n:?} against {t:?} and {u:?}"
        );
        assert!(
            (n[0] - h).abs() < 1e-6 && (n[1] + h).abs() < 1e-6 && n[2] == 0.0,
            "{n:?}"
        );
        // a mirror flips the handedness
        let m = Placing::new(pose([[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]));
        assert!(matches!(m.turn, Turn::General { mirror: true, .. }));
        assert_eq!(m.tangent([0.0, 1.0, 0.0, 1.0]), Some([0.0, 1.0, 0.0, -1.0]));
        assert_eq!(m.normal([1.0, 0.0, 0.0]), Some([-1.0, 0.0, 0.0]));
        // no inverse: no normals or tangents
        let z = Placing::new(pose([[0.0; 3]; 3]));
        assert_eq!(z.turn, Turn::Singular);
        assert_eq!(z.normal([0.0, 0.0, 1.0]), None);
        assert_eq!(z.tangent([1.0, 0.0, 0.0, 1.0]), None);
    }
}
