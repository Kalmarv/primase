//! Prototypes' drawables and textures into the world, for the prototypes placements still use.
//!
//! - A prototype whose archetype is a drawable (asset type 2) gets `core.mesh` from the live ydr's high LOD: every
//!   model's geometries in order, one primitive each, streams as stored (frame converted), 16-bit indices widened;
//!   [`GtaDrawable`] with the drawable's box and sphere as stored and the coarser LODs as meshes (until core has a
//!   representation for alternative LOD meshes). Drawable dictionaries and fragments are counted, not read yet.
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
use crate::drawable::{Drawable, Geometry, Model, ParamValue};
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
    /// Prototypes whose archetype is a drawable dictionary entry or a fragment (not read yet), or names no file.
    pub dictionary_pending: usize,
    pub fragment_pending: usize,
    pub asset_missing: usize,
}

struct Ctx<'a> {
    set: &'a ArchiveSet,
    keys: &'a Keys,
    parents: TextureParents,
    /// Texture dictionaries by name hash (`None`: no live ytd of that name).
    dicts: BTreeMap<u32, Option<(String, TextureDictionary)>>,
    /// Texture entities by (dictionary key, texture name hash).
    textures: BTreeMap<(String, u32), Entity>,
    report: AssetReport,
}

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
        (8, 0x02) => (
            Semantic::Tangent,
            Stream::F32x4(
                bytes
                    .as_chunks::<16>()
                    .0
                    .iter()
                    .map(|c| {
                        let t = le::vec4(c, 0);
                        let d = GTA.direction([t[0], t[1], t[2]]);
                        [d[0], d[1], d[2], t[3]]
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

/// A model list as a mesh, each geometry a primitive with its shader's material.
fn mesh(models: &[Model], materials: &[Entity]) -> Result<Mesh> {
    let mut primitives = Vec::new();
    for g in models.iter().flat_map(|m| &m.geometries) {
        let mut streams = BTreeMap::new();
        for &e in &g.elements {
            let (sem, s) = stream(g, e)?;
            streams.insert(sem, s);
        }
        primitives.push(Primitive {
            topology: Topology::Triangles,
            streams,
            indices: Some(g.indices.iter().map(|&i| u32::from(i)).collect()),
            material: materials.get(usize::from(g.shader)).copied(),
        });
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
        report: AssetReport::default(),
    };
    let used: BTreeSet<Entity> = w.iter::<Instance>().map(|(_, i)| i.prototype).collect();
    for proto in used {
        let Some(a) = w.get::<GtaArchetype>(proto).cloned() else {
            continue;
        };
        match a.asset_type {
            2 => {}
            3 => {
                ctx.report.dictionary_pending += 1;
                continue;
            }
            1 => {
                ctx.report.fragment_pending += 1;
                continue;
            }
            _ => continue,
        }
        let Some(f) = set.tables.get("ydr").and_then(|t| t.get(&a.asset_name)).copied() else {
            ctx.report.asset_missing += 1;
            continue;
        };
        let path = set.path(f).to_string();
        let d = Drawable::read_resource(&ctx.decode(f)?)?;
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
        let high = mesh(&d.lods[0], &materials)?;
        ctx.report.geometries += high.primitives.len();
        w.insert(proto, high);
        let lod = |i: usize| -> Result<Option<Mesh>> {
            if d.lods[i].is_empty() {
                Ok(None)
            } else {
                mesh(&d.lods[i], &materials).map(Some)
            }
        };
        w.insert(
            proto,
            GtaDrawable {
                path,
                bbox: [d.bbox_min, d.bbox_max],
                bounding_sphere: (d.bounding_centre, d.bounding_radius),
                lod_dist: d.lod_dist,
                render_masks: d.render_masks,
                models: std::array::from_fn(|i| {
                    d.lods[i].iter().map(|m| m.geometries.len() as u32).collect()
                }),
                lods: [lod(1)?, lod(2)?, lod(3)?],
            },
        );
        ctx.report.drawables += 1;
    }
    Ok(ctx.report)
}
