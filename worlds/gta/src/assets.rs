//! Prototypes' drawables and textures into the world, for the prototypes placements still use.
//!
//! - A prototype whose archetype is a drawable (asset type 2: the live ydr), a fragment (1: the live yft's main
//!   drawable) or a drawable dictionary's entry (3: the archetype's live ydd, entry by asset name) gets `core.mesh` from
//!   that drawable's high LOD: every model's geometries in order, one primitive each, streams as stored (frame
//!   converted), 16-bit indices widened;
//!   [`GtaSources`] (LOD 0, each primitive's model and geometry), [`GtaPoses`] when a model is drawn at its bone's
//!   pose (a fragment's; `gta-pose-fragments` places it), and [`GtaDrawable`] with the drawable's box, sphere, LOD
//!   distances and render masks as stored. Each coarser LOD that has models (medium, low, very low) is a
//!   `core.lods` form of the prototype at its level (1 to 3): an unplaced entity with that LOD's `core.mesh` and
//!   [`GtaSources`]. GTA draws by distance and authors no level of its own choice, so `core.lods` has no default
//!   (`select-lod` then takes the high LOD, counted); a pipeline picks a level with `select-lod{form=N}`.
//! - [`GtaEmbeddedBounds`] when the asset embeds a collision bound: a fragment's first physics LOD's, else the
//!   drawable's own (a ydr's; no drawable dictionary entry of the install has one).
//! - A material per (drawable, shader) with [`GtaShader`]: every parameter by stored name, textures resolved the way
//!   the game looks them up: the drawable's own dictionary, then the archetype's texture dictionary, then that
//!   dictionary's parents ([`TextureParents`]), then the resident detail dictionary ([`RESIDENT`]).
//! - A texture entity per (dictionary, texture): `core.texture` (blocks as stored, levels split, colour space from the
//!   format; role `Other` until a pass knows which sampler reads it), `core.name`, `core.source`.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: reads drawables and textures from the install"
)]
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;

use primase_core::{
    Entity, Instance, Mesh, Name, Primitive, Semantic, Source, Stream, TexRole, Texture as CoreTexture,
    Topology, World,
};
use primase_core::{LodForm, Lods};

use crate::archive_set::ArchiveSet;
use crate::collision::GtaEmbeddedBounds;
use crate::components::{GtaArchetype, GtaDrawable, GtaLights, GtaParam, GtaPoses, GtaShader, GtaSources};
use crate::drawable::{Drawable, DrawableDictionary, Fragment, Geometry, Model, ParamValue, resource_lights};
use crate::gtxd::TextureParents;
use crate::keys::Keys;
use crate::le;
use crate::pose::Pose;
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
    /// High-LOD models with a bone's pose to be drawn at (a fragment's models; `gta-pose-fragments` places them).
    pub posed_models: usize,
    /// Prototypes whose archetype names no live file or entry.
    pub asset_missing: usize,
    /// Coarser LODs made `core.lods` forms.
    pub lod_forms: usize,
    /// Prototypes given their drawable's or fragment's collision bound ([`GtaEmbeddedBounds`]).
    pub embedded_bounds: usize,
    /// Vertex streams left out because the file stores values that are not finite (NaN, infinity), by semantic: one
    /// per primitive of any LOD. Such a value is unknown, and core holds finite streams only.
    pub non_finite_streams: BTreeMap<String, usize>,
}

struct Ctx<'a> {
    set: &'a ArchiveSet,
    keys: &'a Keys,
    parents: &'a TextureParents,
    /// Texture dictionaries by name hash (`None`: no live ytd of that name).
    dicts: BTreeMap<u32, Option<(String, TextureDictionary)>>,
    /// Texture entities by (dictionary key, texture name hash).
    textures: BTreeMap<(String, u32), Entity>,
    /// Drawable dictionaries read, by name hash.
    ydds: BTreeMap<u32, DrawableDictionary>,
    report: AssetReport,
}

/// One vertex element as a core stream (frame converted).
fn stream(g: &Geometry, e: crate::drawable::Element) -> Result<(Semantic, Stream)> {
    let bytes = g.element_bytes(e);
    let f3 = |b: &[u8]| -> Vec<[f32; 3]> { b.as_chunks::<12>().0.iter().map(|c| le::vec3(c, 0)).collect() };
    let fmt = e.format.0;
    Ok(match (e.slot, fmt) {
        (0, 0x06) => (
            Semantic::Position,
            Stream::F32x3(f3(&bytes).into_iter().map(|p| GTA.point(p)).collect()),
        ),
        (4, 0x06) => (
            Semantic::Normal,
            Stream::F32x3(f3(&bytes).into_iter().map(|n| GTA.direction(n)).collect()),
        ),
        // GTA's vertex shaders take the binormal as cross(T, N) * w (VS_TransformD_81053d2c); glTF's bitangent is
        // cross(N, T) * w, and the frame change is a rotation: w negated
        (8, 0x02) => (
            Semantic::Tangent,
            Stream::F32x4(
                bytes
                    .as_chunks::<16>()
                    .0
                    .iter()
                    .map(|c| {
                        let v = le::vec4(c, 0);
                        let d = GTA.direction([v[0], v[1], v[2]]);
                        [d[0], d[1], d[2], -v[3]]
                    })
                    .collect(),
            ),
        ),
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
    })
}

fn u8x4(b: &[u8]) -> Vec<[u8; 4]> {
    b.as_chunks::<4>().0.to_vec()
}

/// Each model's transform: its bone's pose (a fragment's), none for skinned models (bound to bone weights) or without a
/// pose. As the game draws a fragment at rest; only for drawables with a skeleton.
fn model_poses(d: &Drawable, models: &[Model], pose: &[Pose]) -> Vec<Option<Pose>> {
    models
        .iter()
        .map(|m| {
            let (bone, skinned) = (
                (m.skeleton_binding >> 24) & 0xFF,
                (m.skeleton_binding >> 8) & 0xFF != 0,
            );
            if d.skeleton.is_none() || skinned {
                None
            } else {
                pose.get(bone as usize).copied()
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
    non_finite: &mut BTreeMap<String, usize>,
) -> Result<Mesh> {
    let mut primitives = Vec::new();
    for m in models {
        for g in &m.geometries {
            let mut streams = BTreeMap::new();
            for &e in &g.elements {
                let (sem, s) = stream(g, e)?;
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
    attach(w, set, keys, &TextureParents::read(set, keys)?)
}

/// What [`attach_lights`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LightsReport {
    /// Prototypes whose drawable or fragment was read for its lights.
    pub prototypes_read: usize,
    /// Of those, the ones with lights, and their light records.
    pub prototypes_with_lights: usize,
    pub lights: usize,
    /// Prototypes whose archetype names no live file or entry.
    pub asset_missing: usize,
}

/// The light records ([`GtaLights`]) of every prototype in `w`, read from its live drawable, fragment or drawable
/// dictionary entry without its geometry or textures. A writer that plans lights before it loads geometry (the web
/// writer plans scene-wide) has them from the read world.
pub fn attach_lights(w: &mut World, set: &ArchiveSet, keys: &Keys) -> Result<LightsReport> {
    let mut report = LightsReport::default();
    let decode = |f: crate::archive_set::FileRef| -> Result<crate::resource::Resource> {
        let a = &set.install.archives[f.archive];
        a.resource(&mut File::open(&a.file)?, f.entry, keys)
    };
    let live = |ext: &str, hash: u32| set.tables.get(ext).and_then(|t| t.get(&hash)).copied();
    // per prototype the file to read and, for a dictionary entry, its name; dictionaries are read once, below
    let mut files: Vec<(Entity, &'static str, crate::archive_set::FileRef)> = Vec::new();
    let mut entries: BTreeMap<u32, (crate::archive_set::FileRef, Vec<(Entity, u32)>)> = BTreeMap::new();
    for (proto, a) in w.iter::<GtaArchetype>() {
        match a.asset_type {
            2 | 1 => {
                let ext = if a.asset_type == 2 { "ydr" } else { "yft" };
                match live(ext, a.asset_name) {
                    Some(f) => files.push((proto, ext, f)),
                    None => report.asset_missing += 1,
                }
            }
            3 => match live("ydd", a.drawable_dictionary) {
                Some(f) => entries
                    .entry(a.drawable_dictionary)
                    .or_insert_with(|| (f, Vec::new()))
                    .1
                    .push((proto, a.asset_name)),
                None => report.asset_missing += 1,
            },
            _ => {}
        }
    }
    // the reads are independent: spread them over the cores
    let jobs: Vec<Job> = files
        .into_iter()
        .map(|(p, ext, f)| Job::One(p, ext, f))
        .chain(entries.into_values().map(|(f, e)| Job::Dictionary(f, e)))
        .collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let results: Vec<Result<Vec<ProtoLights>>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| -> Result<Vec<ProtoLights>> {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(job) = jobs.get(i) else { break };
                        match job {
                            Job::One(p, ext, f) => out.push((*p, resource_lights(&decode(*f)?, ext, 0)?)),
                            Job::Dictionary(f, protos) => {
                                let r = decode(*f)?;
                                for (p, name) in protos {
                                    out.push((*p, resource_lights(&r, "ydd", *name)?));
                                }
                            }
                        }
                    }
                    Ok(out)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err(invalid("lights", 0, "a reading thread panicked")))
            })
            .collect()
    });
    let mut all = Vec::new();
    for r in results {
        all.extend(r?);
    }
    all.sort_by_key(|(e, _)| *e);
    for (proto, lights) in all {
        report.prototypes_read += 1;
        if !lights.is_empty() {
            report.prototypes_with_lights += 1;
            report.lights += lights.len();
            w.insert(proto, GtaLights(lights));
        }
    }
    Ok(report)
}

/// A prototype and the light records read for it.
type ProtoLights = (Entity, Vec<crate::drawable::LightAttr>);

/// One read of [`attach_lights`]: a drawable or fragment, or a dictionary with the prototypes of its entries.
enum Job {
    One(Entity, &'static str, crate::archive_set::FileRef),
    Dictionary(crate::archive_set::FileRef, Vec<(Entity, u32)>),
}

/// [`attach_assets`] with the texture dictionary parents already read ([`TextureParents::read`]): a driver that loads a
/// large region cell by cell reads them once.
pub fn attach_assets_with(
    w: &mut World,
    set: &ArchiveSet,
    keys: &Keys,
    parents: &TextureParents,
) -> Result<AssetReport> {
    attach(w, set, keys, parents)
}

fn attach(w: &mut World, set: &ArchiveSet, keys: &Keys, parents: &TextureParents) -> Result<AssetReport> {
    let mut ctx = Ctx {
        set,
        keys,
        parents,
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
        // a fragment's physics bound (what it collides with); a drawable's own otherwise
        let mut bound = None;
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
                let mut frag = Fragment::read_resource(&ctx.decode(f)?)?;
                bound = frag.physics.take();
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
        if let Some(bound) = bound.or_else(|| d.bound.clone()) {
            w.insert(
                proto,
                GtaEmbeddedBounds {
                    path: path.clone(),
                    bound,
                },
            );
            ctx.report.embedded_bounds += 1;
        }
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
        // a drawable's models at their bones' rest pose (a fragment's own pose replaces it)
        if pose.is_empty()
            && let Some(s) = &d.skeleton
        {
            pose = s.pose();
        }
        let poses: [Vec<Option<Pose>>; 4] = std::array::from_fn(|i| model_poses(&d, &d.lods[i], &pose));
        // per LOD that has models: its mesh and its primitives' sources; the high LOD is the prototype's own, the
        // others are its forms
        let mut forms = Vec::new();
        for (lod, models) in d.lods.iter().enumerate() {
            if lod > 0 && models.is_empty() {
                continue;
            }
            let m = mesh(&path, models, &materials, &mut ctx.report.non_finite_streams)?;
            // per primitive its model's pose, for gta-pose-fragments
            let posed = (models.iter().zip(&poses[lod]))
                .flat_map(|(model, p)| std::iter::repeat_n(*p, model.geometries.len()))
                .collect::<Vec<_>>();
            if lod == 0 {
                ctx.report.posed_models += poses[0].iter().flatten().count();
            }
            let sources = GtaSources {
                lod: lod as u8,
                sources: (models.iter().enumerate())
                    .flat_map(|(i, model)| (0..model.geometries.len() as u32).map(move |g| [i as u32, g]))
                    .collect(),
            };
            let holder = if lod == 0 {
                ctx.report.geometries += m.primitives.len();
                proto
            } else {
                let f = w.spawn();
                forms.push(LodForm {
                    detail: lod as u8,
                    form: f,
                });
                ctx.report.lod_forms += 1;
                f
            };
            w.insert(holder, m);
            w.insert(holder, sources);
            if posed.iter().any(Option::is_some) {
                w.insert(holder, GtaPoses(posed));
            }
        }
        if !forms.is_empty() {
            w.insert(
                proto,
                Lods {
                    detail: 0,
                    forms,
                    default: None,
                },
            );
        }
        w.insert(
            proto,
            GtaDrawable {
                path,
                bbox: [d.bbox_min, d.bbox_max],
                bounding_sphere: (d.bounding_centre, d.bounding_radius),
                lod_dist: d.lod_dist,
                render_masks: d.render_masks,
            },
        );
        ctx.report.drawables += 1;
    }
    Ok(ctx.report)
}

#[cfg(test)]
mod tests {
    use super::stream;
    use crate::drawable::{Element, Geometry};
    use crate::sga::BufferFormat;
    use primase_core::{Semantic, Stream};

    /// GTA's binormal is cross(T, N) * w and glTF's bitangent cross(N, T) * w: the tangent's w is negated, its
    /// direction turned into glTF's frame like normals.
    #[test]
    fn tangent_w_takes_gltf_meaning() {
        let mut vertices = Vec::new();
        for v in [1.0f32, 0.0, 0.0, 1.0] {
            vertices.extend(v.to_le_bytes());
        }
        let e = Element {
            slot: 8,
            format: BufferFormat(0x02),
            offset: 0,
        };
        let g = Geometry {
            shader: 0,
            bounds: [[0.0; 4]; 2],
            vertex_count: 1,
            stride: 16,
            elements: vec![e],
            vertices,
            indices: vec![],
        };
        let (semantic, s) = stream(&g, e).unwrap();
        assert_eq!(semantic, Semantic::Tangent);
        let d = crate::region::GTA.direction([1.0, 0.0, 0.0]);
        assert_eq!(s, Stream::F32x4(vec![[d[0], d[1], d[2], -1.0]]));
    }
}
