//! Prototypes' drawables and textures into the world, for the prototypes placements still use.
//!
//! - Each prototype's files are looked up in the archive set and texture parents of a map state it is resolved in
//!   ([`Lookups::of`], [`GtaResolvedIn`]): a prototype is one resolved payload ([`Payload`]), so Story's and GTA
//!   Online's files never stand in for each other.
//! - A prototype whose archetype is a drawable (asset type 2: the live ydr), a fragment (1: the live yft's main
//!   drawable) or a drawable dictionary's entry (3: the archetype's live ydd, entry by asset name) gets `core.mesh` from
//!   that drawable's high LOD: every model's geometries in order, one primitive each, streams as stored (frame
//!   converted), 16-bit indices widened; the blend streams (`BLENDWEIGHT0`, `BLENDINDICES0`) not in it but as stored in
//!   [`GtaBlendStreams`] with the geometry's bone palette (the bone ids they index), as stored;
//!   [`GtaSources`] (LOD 0, each primitive's model and geometry), [`GtaBones`] when a model is bound to a bone (its
//!   bones the prototype's [`GtaSkeleton`], as stored; `gta-pose-fragments` places it), and [`GtaDrawable`] with the drawable's box, sphere, LOD
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
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;

use primase_core::{
    Entity, Instance, Mesh, Name, Primitive, Semantic, Source, Stream, TexRole, Texture as CoreTexture,
    Topology, World,
};
use primase_core::{LodForm, Lods, Origin, Provenance};

use crate::components::GTA;
use crate::components::GtaResolvedIn;
use crate::components::collision::GtaEmbeddedBounds;
use crate::components::{
    BlendStreams, GtaArchetype, GtaBlendStreams, GtaBones, GtaDrawable, GtaLights, GtaParam, GtaShader,
    GtaSkeleton, GtaSources,
};
use crate::rage::drawable::{
    Bones, Drawable, DrawableDictionary, Fragment, Geometry, Model, ParamValue, resource_lights,
};
use crate::rage::le;
use crate::rage::map::ArchetypeDef;
use crate::rage::texture::{Texture, TextureDictionary};
use crate::read::archive::archive_set::{ArchiveSet, FileRef, Install};
use crate::read::archive::keys::Keys;
use crate::read::gtxd::TextureParents;
use crate::{Result, invalid};

/// The resident detail dictionary searched last (`x64a.rpf\textures\mapdetail.ytd`, the global textures folder): detail
/// maps (`env_*`) are bound by name and no parent chain reaches it. In Legion it is the only dictionary holding all four
/// detail textures the chains missed (188 others hold scattered copies); CodeWalker also keeps it resident.
const RESIDENT: &str = "mapdetail";

/// One map state's asset lookup: its archive set (the live files) and its texture dictionary parents.
#[derive(Clone, Copy, Debug)]
pub struct Lookup<'a> {
    pub set: &'a ArchiveSet,
    pub parents: &'a TextureParents,
}

/// What of an archetype names its payload: its asset type and name, drawable dictionary and texture dictionary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssetNames {
    pub asset_type: i32,
    pub asset_name: u32,
    pub drawable_dictionary: u32,
    pub texture_dictionary: u32,
}

impl From<&ArchetypeDef> for AssetNames {
    fn from(a: &ArchetypeDef) -> AssetNames {
        AssetNames {
            asset_type: a.asset_type,
            asset_name: a.asset_name,
            drawable_dictionary: a.drawable_dictionary,
            texture_dictionary: a.texture_dictionary,
        }
    }
}

impl From<&GtaArchetype> for AssetNames {
    fn from(a: &GtaArchetype) -> AssetNames {
        AssetNames {
            asset_type: a.asset_type,
            asset_name: a.asset_name,
            drawable_dictionary: a.drawable_dictionary,
            texture_dictionary: a.texture_dictionary,
        }
    }
}

/// The files a map state resolves an archetype's payload to: its drawable's ([`Lookup::drawable`]; `None`: no live
/// file, or an asset type without one) and each texture dictionary's of its lookup ([`Lookup::texture_dictionaries`],
/// in lookup order; `None`: no live ytd of that name). Its light records are its drawable's. Two states load the same
/// payload exactly when these are equal.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Payload {
    pub drawable: Option<FileRef>,
    pub textures: Vec<Option<FileRef>>,
}

impl Lookup<'_> {
    fn live(&self, ext: &str, hash: u32) -> Option<FileRef> {
        self.set.tables.get(ext).and_then(|t| t.get(&hash)).copied()
    }

    /// The live file holding an archetype's drawable, and its extension: a drawable's ydr (asset type 2), a fragment's
    /// yft (1), the ydd holding a dictionary entry (3); `None` for no live file or another asset type.
    pub fn drawable(&self, a: AssetNames) -> Option<(&'static str, FileRef)> {
        let (ext, hash) = match a.asset_type {
            2 => ("ydr", a.asset_name),
            1 => ("yft", a.asset_name),
            3 => ("ydd", a.drawable_dictionary),
            _ => return None,
        };
        self.live(ext, hash).map(|f| (ext, f))
    }

    /// The texture dictionaries the game searches after a drawable's own, by name hash: the archetype's, that
    /// dictionary's parents ([`TextureParents::chain`]), then the resident detail dictionary ([`RESIDENT`]).
    pub fn texture_dictionaries(&self, txd: u32) -> Vec<u32> {
        std::iter::once(txd)
            .chain(self.parents.chain(txd))
            .chain(std::iter::once(primase_hash::joaat(RESIDENT.as_bytes())))
            .collect()
    }

    /// The files this state resolves an archetype's payload to.
    pub fn payload(&self, a: AssetNames) -> Payload {
        Payload {
            drawable: self.drawable(a).map(|(_, f)| f),
            textures: (self.texture_dictionaries(a.texture_dictionary).into_iter())
                .map(|d| self.live("ytd", d))
                .collect(),
        }
    }
}

/// Both map states' lookups, over one install: a prototype's payload is loaded through the lookup of a state it is
/// resolved in ([`GtaResolvedIn`]).
#[derive(Clone, Copy, Debug)]
pub struct Lookups<'a> {
    story: Lookup<'a>,
    online: Option<Lookup<'a>>,
}

impl<'a> Lookups<'a> {
    /// Story's lookup and, when read, GTA Online's; both archive sets must be over the same install (their file
    /// references index its archives).
    pub fn new(story: Lookup<'a>, online: Option<Lookup<'a>>) -> Result<Lookups<'a>> {
        if let Some(o) = online {
            let (s, o) = (&story.set.install, &o.set.install);
            if s.dir != o.dir || s.archives.len() != o.archives.len() {
                return Err(invalid(
                    "archive sets",
                    0,
                    "Story's and GTA Online's are over different installs",
                ));
            }
        }
        Ok(Lookups { story, online })
    }

    /// A world read from one archive set: every prototype is loaded through it.
    pub fn one(set: Lookup<'a>) -> Lookups<'a> {
        Lookups {
            story: set,
            online: None,
        }
    }

    /// The install both archive sets are over.
    fn install(&self) -> &'a Install {
        &self.story.set.install
    }

    /// The lookup prototype `proto`'s payload is loaded through: Story's when Story resolves it or nothing says (a
    /// world read from one archive set), else GTA Online's.
    pub fn of(&self, w: &World, proto: Entity) -> Result<Lookup<'a>> {
        match w.get::<GtaResolvedIn>(proto) {
            Some(r) if !r.story && r.online => self.online.ok_or_else(|| {
                invalid(
                    "prototype",
                    0,
                    format!("{proto:?} is resolved in GTA Online's archive set, which was not read"),
                )
            }),
            _ => Ok(self.story),
        }
    }
}

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
    /// High-LOD models bound to a bone, to be drawn at its pose (`gta-pose-fragments` places them).
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
    /// Primitives of any LOD whose geometry stores vertex blend streams, kept as [`GtaBlendStreams`] with their bone
    /// palette rather than core joints and weights.
    pub blend_streams: usize,
}

/// What loading drawables and textures keeps between prototypes: the files read once and the textures made once.
pub(crate) struct Ctx<'a> {
    install: &'a Install,
    keys: &'a Keys,
    /// Texture dictionaries read, by file, with their paths.
    dicts: BTreeMap<FileRef, (String, TextureDictionary)>,
    /// Texture entities by (dictionary key, texture name hash).
    textures: BTreeMap<(String, u32), Entity>,
    /// Drawable dictionaries read, by file.
    ydds: BTreeMap<FileRef, DrawableDictionary>,
    /// Texture dictionary files searched first, in order, for every texture a drawable names (a component ped's own
    /// `.ytd`: its drawables name textures that dictionary holds, whatever the archetype's lookup).
    pub(crate) own_dictionaries: Vec<FileRef>,
    pub(crate) report: AssetReport,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(install: &'a Install, keys: &'a Keys) -> Ctx<'a> {
        Ctx {
            install,
            keys,
            dicts: BTreeMap::new(),
            textures: BTreeMap::new(),
            ydds: BTreeMap::new(),
            own_dictionaries: Vec::new(),
            report: AssetReport::default(),
        }
    }
}

/// Where a vertex element goes.
enum Element {
    /// A core stream (frame converted).
    Core(Semantic, Stream),
    /// `BLENDWEIGHT0` as stored.
    BlendWeights(Vec<[u8; 4]>),
    /// `BLENDINDICES0` as stored: entries of the geometry's bone palette.
    BlendIndices(Vec<[u8; 4]>),
}

/// One vertex element: a core stream (frame converted), or a blend stream kept as stored.
fn stream(g: &Geometry, e: crate::rage::drawable::Element) -> Result<Element> {
    let bytes = g.element_bytes(e);
    let f3 = |b: &[u8]| -> Vec<[f32; 3]> { b.as_chunks::<12>().0.iter().map(le::vec3_of).collect() };
    let fmt = e.format.0;
    let (semantic, s) = match (e.slot, fmt) {
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
                        let v = le::vec4_of(c);
                        let d = GTA.direction([v[0], v[1], v[2]]);
                        [d[0], d[1], d[2], -v[3]]
                    })
                    .collect(),
            ),
        ),
        (16, 0x1C) => return Ok(Element::BlendWeights(u8x4(&bytes))),
        (20, 0x1E) => return Ok(Element::BlendIndices(u8x4(&bytes))),
        (24..=27, 0x1C) => (
            Semantic::Color(e.slot.wrapping_sub(24)),
            Stream::U8x4(u8x4(&bytes)),
        ),
        (28..=51, 0x10) => (
            Semantic::TexCoord(e.slot.wrapping_sub(28)),
            Stream::F32x2(bytes.as_chunks::<8>().0.iter().map(le::f32x2_of).collect()),
        ),
        _ => {
            return Err(invalid(
                "drawable",
                0,
                format!("{} in format {fmt:#x} has no core stream", e.semantic()),
            ));
        }
    };
    Ok(Element::Core(semantic, s))
}

/// Count one more.
fn bump(n: &mut usize) {
    *n = n.saturating_add(1);
}

fn u8x4(b: &[u8]) -> Vec<[u8; 4]> {
    b.as_chunks::<4>().0.to_vec()
}

/// Each model's bone, from its skeleton binding: none for skinned models (bound to bone weights) or a drawable without
/// a skeleton.
fn model_bones(d: &Drawable, models: &[Model]) -> Vec<Option<u16>> {
    models
        .iter()
        .map(|m| {
            let [bone, _, skinned, _] = m.skeleton_binding.to_be_bytes();
            (d.skeleton.is_some() && skinned == 0).then_some(u16::from(bone))
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

/// A model list as a mesh, each geometry a primitive with its shader's material, and the geometries' vertex blend
/// streams as stored (`None` when no geometry has any). A stream other than the positions holding a value that is
/// not finite is left out and counted in `non_finite`; such positions are an error.
fn mesh(
    path: &str,
    models: &[Model],
    materials: &[Entity],
    non_finite: &mut BTreeMap<String, usize>,
) -> Result<(Mesh, Option<GtaBlendStreams>)> {
    let mut primitives = Vec::new();
    let mut blend = Vec::new();
    for m in models {
        for g in &m.geometries {
            let mut streams = BTreeMap::new();
            let mut b = BlendStreams {
                weights: None,
                indices: None,
                palette: g.bone_ids.clone(),
            };
            for &e in &g.elements {
                let (sem, s) = match stream(g, e)? {
                    Element::Core(sem, s) => (sem, s),
                    Element::BlendWeights(v) => {
                        b.weights = Some(v);
                        continue;
                    }
                    Element::BlendIndices(v) => {
                        b.indices = Some(v);
                        continue;
                    }
                };
                if !finite(&s) {
                    if sem == Semantic::Position {
                        return Err(invalid(
                            "drawable",
                            0,
                            format!("{path}: positions that are not finite"),
                        ));
                    }
                    bump(non_finite.entry(format!("{sem:?}")).or_default());
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
            blend.push((b.weights.is_some() || b.indices.is_some()).then_some(b));
        }
    }
    let blend = blend
        .iter()
        .any(Option::is_some)
        .then_some(GtaBlendStreams(blend));
    Ok((Mesh::new(primitives), blend))
}

/// A resource file of the install, decoded.
pub(crate) fn decode(install: &Install, keys: &Keys, f: FileRef) -> Result<crate::rage::resource::Resource> {
    let a = install.archives.get(f.archive).ok_or_else(|| {
        invalid(
            "archive set",
            0,
            format!("archive {} of {}", f.archive, install.archives.len()),
        )
    })?;
    a.resource(&mut File::open(&a.file)?, f.entry, keys)
}

impl Ctx<'_> {
    fn decode(&self, f: FileRef) -> Result<crate::rage::resource::Resource> {
        decode(self.install, self.keys, f)
    }

    /// The texture dictionary of this name `lookup` resolves, and its path; `None` when no live ytd has the name.
    fn dictionary(&mut self, lookup: Lookup<'_>, txd: u32) -> Result<Option<&(String, TextureDictionary)>> {
        let Some(f) = lookup.live("ytd", txd) else {
            return Ok(None);
        };
        if !self.dicts.contains_key(&f) {
            let d = TextureDictionary::read_resource(&self.decode(f)?)?;
            self.dicts.insert(f, (lookup.set.path(f).to_string(), d));
        }
        Ok(self.dicts.get(&f))
    }

    pub(crate) fn texture_entity(&mut self, w: &mut World, source: &str, t: &Texture) -> Result<Entity> {
        let key = (source.to_string(), primase_hash::joaat(t.name.as_bytes()));
        if let Some(&e) = self.textures.get(&key) {
            return Ok(e);
        }
        let (format, color_space) = t
            .format
            .texture()
            .ok_or_else(|| invalid("texture", 0, format!("{}: format {:#x}", t.name, t.format.0)))?;
        let mut levels = Vec::new();
        let mut rest = t.data.as_slice();
        for level in 0..u32::from(t.levels) {
            let n = format
                .level_size(
                    (u32::from(t.width) >> level).max(1),
                    (u32::from(t.height) >> level).max(1),
                )
                .checked_mul(usize::from(t.depth))
                .filter(|&n| n <= rest.len())
                .ok_or_else(|| {
                    invalid(
                        "texture",
                        0,
                        format!(
                            "{}: level {level} runs past the texture's {} bytes",
                            t.name,
                            t.data.len()
                        ),
                    )
                })?;
            let (this, next) = rest.split_at(n);
            levels.push(this.to_vec());
            rest = next;
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
        w.insert(e, Provenance::new(origin(source, Some(&t.name))));
        self.textures.insert(key, e);
        bump(&mut self.report.textures);
        Ok(e)
    }

    /// The texture of this name the game finds for a drawable of this archetype, in `lookup`'s state.
    fn find_texture(
        &mut self,
        w: &mut World,
        lookup: Lookup<'_>,
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
                .and_then(|i| own.textures.get(i))
                .and_then(Option::as_ref)
        {
            return self.texture_entity(w, ydr, t).map(Some);
        }
        for f in self.own_dictionaries.clone() {
            if !self.dicts.contains_key(&f) {
                let dict = TextureDictionary::read_resource(&self.decode(f)?)?;
                let path = lookup.set.path(f).to_string();
                self.dicts.insert(f, (path, dict));
            }
            let found = self.dicts.get(&f).and_then(|(path, dict)| {
                let i = dict.hashes.iter().position(|&x| x == h)?;
                dict.textures.get(i)?.clone().map(|t| (path.clone(), t))
            });
            if let Some((path, t)) = found {
                return self.texture_entity(w, &path, &t).map(Some);
            }
        }
        for d in lookup.texture_dictionaries(txd) {
            let found = self.dictionary(lookup, d)?.and_then(|(path, dict)| {
                let i = dict.hashes.iter().position(|&x| x == h)?;
                dict.textures.get(i)?.clone().map(|t| (path.clone(), t))
            });
            if let Some((path, t)) = found {
                return self.texture_entity(w, &path, &t).map(Some);
            }
        }
        bump(
            self.report
                .textures_not_found
                .entry(name.to_lowercase())
                .or_default(),
        );
        Ok(None)
    }
}

/// A drawable's levels of detail as CodeWalker names them.
const LOD_NAMES: [&str; 4] = ["high", "med", "low", "vlow"];

/// The provenance origin of `item` in the install file at `path`, as the archive set names it: the archive chain up
/// to the last `\` (`x64i.rpf\levels\...\dt1_03.rpf`), then the file; a dictionary entry's `#<hash>` suffix
/// becomes the start of the item.
pub(crate) fn origin(path: &str, item: Option<&str>) -> Origin {
    let (file, entry) = match path.split_once('#') {
        Some((f, e)) => (f, Some(e)),
        None => (path, None),
    };
    let (archive, name) = match file.rsplit_once('\\') {
        Some((a, n)) => (Some(a), n),
        None => (None, file),
    };
    let item = match (entry, item) {
        (Some(e), Some(i)) => Some(format!("entry {e} {i}")),
        (Some(e), None) => Some(format!("entry {e}")),
        (None, i) => i.map(str::to_owned),
    };
    Origin::new(archive, name, item.as_deref())
}

/// Loads drawables and textures for every prototype a placement uses, from one archive set ([`Lookups::one`]).
pub fn attach_assets(w: &mut World, set: &ArchiveSet, keys: &Keys) -> Result<AssetReport> {
    let parents = TextureParents::read(set, keys)?;
    attach(
        w,
        Lookups::one(Lookup {
            set,
            parents: &parents,
        }),
        keys,
    )
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

/// The light records ([`GtaLights`]) of every prototype in `w`, with the bones they are on ([`GtaSkeleton`], as stored),
/// read from its live drawable, fragment or drawable dictionary entry in the state it is resolved in
/// ([`Lookups::of`]), without its geometry or textures. A writer that plans lights before it loads geometry (the web
/// writer plans scene-wide) has them from the read world.
pub fn attach_lights(w: &mut World, lookups: Lookups<'_>, keys: &Keys) -> Result<LightsReport> {
    let mut report = LightsReport::default();
    let install = lookups.install();
    // per prototype the file to read and, for a dictionary entry, its name; each dictionary file is read once, below
    let mut files: Vec<(Entity, &'static str, FileRef)> = Vec::new();
    let mut entries: BTreeMap<FileRef, Vec<(Entity, u32)>> = BTreeMap::new();
    for (proto, a) in w.iter::<GtaArchetype>() {
        if !matches!(a.asset_type, 1..=3) {
            continue;
        }
        match lookups.of(w, proto)?.drawable(a.into()) {
            Some(("ydd", f)) => entries.entry(f).or_default().push((proto, a.asset_name)),
            Some((ext, f)) => files.push((proto, ext, f)),
            None => bump(&mut report.asset_missing),
        }
    }
    // the reads are independent: spread them over the cores
    let jobs: Vec<Job> = files
        .into_iter()
        .map(|(p, ext, f)| Job::One(p, ext, f))
        .chain(entries.into_iter().map(|(f, e)| Job::Dictionary(f, e)))
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
                            Job::One(p, ext, f) => {
                                out.push((*p, resource_lights(&decode(install, keys, *f)?, ext, 0)?));
                            }
                            Job::Dictionary(f, protos) => {
                                let r = decode(install, keys, *f)?;
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
        put_lights(w, &mut report, proto, lights);
    }
    Ok(report)
}

/// A prototype's light records as read, with their bones: into the world when it has any, counted into `report`.
pub(crate) fn put_lights(
    w: &mut World,
    report: &mut LightsReport,
    proto: Entity,
    lights: (Vec<crate::rage::drawable::LightAttr>, Bones),
) {
    bump(&mut report.prototypes_read);
    if !lights.0.is_empty() {
        bump(&mut report.prototypes_with_lights);
        report.lights = report.lights.saturating_add(lights.0.len());
        let (records, bones) = lights;
        w.insert(proto, GtaLights { records });
        if !bones.is_empty() {
            w.insert(proto, GtaSkeleton(bones));
        }
    }
}

/// A prototype and the light records read for it.
type ProtoLights = (Entity, (Vec<crate::rage::drawable::LightAttr>, Bones));

/// One read of [`attach_lights`]: a drawable or fragment, or a dictionary with the prototypes of its entries.
enum Job {
    One(Entity, &'static str, FileRef),
    Dictionary(FileRef, Vec<(Entity, u32)>),
}

/// [`attach_assets`] through both map states' lookups (their texture dictionary parents already read,
/// [`TextureParents::read`]: a driver that loads a large region cell by cell reads them once): each prototype's
/// drawable and textures from the state it is resolved in ([`Lookups::of`]).
pub fn attach_assets_with(w: &mut World, lookups: Lookups<'_>, keys: &Keys) -> Result<AssetReport> {
    attach(w, lookups, keys)
}

fn attach(w: &mut World, lookups: Lookups<'_>, keys: &Keys) -> Result<AssetReport> {
    let mut ctx = Ctx::new(lookups.install(), keys);
    let used: BTreeSet<Entity> = w.iter::<Instance>().map(|(_, i)| i.prototype).collect();
    for proto in used {
        let Some(a) = w.get::<GtaArchetype>(proto).cloned() else {
            continue;
        };
        if !matches!(a.asset_type, 1..=3) {
            continue;
        }
        let lookup = lookups.of(w, proto)?;
        let Some((_, f)) = lookup.drawable((&a).into()) else {
            bump(&mut ctx.report.asset_missing);
            continue;
        };
        load_prototype(&mut ctx, w, proto, &a, lookup, f)?;
    }
    Ok(ctx.report)
}

/// One file's drawable, a prototype's own: `f` is the file `a` (the prototype's archetype, asset type 1 to 3) resolves
/// to in `lookup`, or any file of that kind the caller means ([`attach_file`]).
pub(crate) fn load_prototype(
    ctx: &mut Ctx<'_>,
    w: &mut World,
    proto: Entity,
    a: &GtaArchetype,
    lookup: Lookup<'_>,
    f: FileRef,
) -> Result<()> {
    {
        let set = lookup.set;
        let mut fragment_pose = Vec::new();
        // a fragment's physics bound (what it collides with); a drawable's own otherwise
        let mut bound = None;
        let (path, d) = match a.asset_type {
            // a drawable
            2 => (set.path(f).to_string(), Drawable::read_resource(&ctx.decode(f)?)?),
            // a fragment: its main drawable
            1 => {
                let mut frag = Fragment::read_resource(&ctx.decode(f)?)?;
                bound = frag.physics.take();
                let Some(d) = frag.drawable else {
                    bump(&mut ctx.report.asset_missing);
                    return Ok(());
                };
                fragment_pose = frag.pose;
                bump(&mut ctx.report.fragments);
                (set.path(f).to_string(), d)
            }
            // an entry of a drawable dictionary
            3 => {
                if !ctx.ydds.contains_key(&f) {
                    let dict = DrawableDictionary::read_resource(&ctx.decode(f)?)?;
                    ctx.ydds.insert(f, dict);
                }
                let Some(d) = ctx.ydds.get(&f).and_then(|dict| dict.get(a.asset_name)).cloned() else {
                    bump(&mut ctx.report.asset_missing);
                    return Ok(());
                };
                bump(&mut ctx.report.dictionary_entries);
                (format!("{}#{:#010x}", set.path(f), a.asset_name), d)
            }
            _ => return Ok(()),
        };
        if let Some(bound) = bound.or_else(|| d.bound.clone()) {
            w.insert(
                proto,
                GtaEmbeddedBounds {
                    path: path.clone(),
                    bound,
                },
            );
            bump(&mut ctx.report.embedded_bounds);
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
                        texture: ctx.find_texture(w, lookup, &d, &path, a.texture_dictionary, n)?,
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
                    index: u32::try_from(i)
                        .map_err(|_| invalid("drawable", 0, format!("{path}: shader {i}")))?,
                    name: s.name,
                    preset: s.preset,
                    render_bucket: s.render_bucket,
                    bucket_mask: s.bucket_mask,
                    params,
                },
            );
            w.insert(m, Provenance::new(origin(&path, Some(&format!("shader {i}")))));
            materials.push(m);
            bump(&mut ctx.report.materials);
        }
        // the bones as stored; a model on a bone is drawn at its pose, which gta-pose-fragments composes
        let bones = Bones {
            skeleton: d.skeleton.clone(),
            fragment_pose,
        };
        if !bones.is_empty() {
            w.insert(proto, GtaSkeleton(bones));
        }
        let on_bones: [Vec<Option<u16>>; 4] = d.lods.each_ref().map(|l| model_bones(&d, l));
        // per LOD that has models: its mesh and its primitives' sources; the high LOD is the prototype's own, the
        // others are its forms
        let mut forms = Vec::new();
        for (lod, ((models, bones), lod_name)) in (0u8..).zip(d.lods.iter().zip(&on_bones).zip(LOD_NAMES)) {
            if lod > 0 && models.is_empty() {
                continue;
            }
            let (m, blend) = mesh(&path, models, &materials, &mut ctx.report.non_finite_streams)?;
            // per primitive its model's bone, for gta-pose-fragments
            let posed = (models.iter().zip(bones))
                .flat_map(|(model, p)| std::iter::repeat_n(*p, model.geometries.len()))
                .collect::<Vec<_>>();
            if lod == 0 {
                ctx.report.posed_models = ctx
                    .report
                    .posed_models
                    .saturating_add(bones.iter().flatten().count());
            }
            let mut primitive_sources = Vec::new();
            for (i, model) in (0u32..).zip(models) {
                let geometries = u32::try_from(model.geometries.len())
                    .map_err(|_| invalid("drawable", 0, format!("{path}: model {i}'s geometries")))?;
                primitive_sources.extend((0..geometries).map(|g| [i, g]));
            }
            let sources = GtaSources {
                lod,
                sources: primitive_sources,
            };
            let holder = if lod == 0 {
                ctx.report.geometries = ctx.report.geometries.saturating_add(m.primitives().len());
                proto
            } else {
                let f = w.spawn();
                forms.push(LodForm { detail: lod, form: f });
                bump(&mut ctx.report.lod_forms);
                f
            };
            // the prototype keeps what the map said of it (its archetype); a form is its drawable's alone
            let mut provenance = if lod == 0 {
                w.get::<Provenance>(holder).cloned().unwrap_or_default()
            } else {
                Provenance::default()
            };
            for (i, model) in models.iter().enumerate() {
                for g in 0..model.geometries.len() {
                    let item = format!("{lod_name} model {i} geometry {g}");
                    provenance.push_part(origin(&path, Some(&item)));
                }
            }
            w.insert(holder, m);
            w.insert(holder, sources);
            w.insert(holder, provenance);
            if posed.iter().any(Option::is_some) {
                w.insert(holder, GtaBones(posed));
            }
            if let Some(blend) = blend {
                let n = blend.0.iter().flatten().count();
                ctx.report.blend_streams = ctx.report.blend_streams.saturating_add(n);
                w.insert(holder, blend);
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
                model_masks: d
                    .lods
                    .each_ref()
                    .map(|l| l.iter().map(|m| m.render_mask).collect()),
            },
        );
        bump(&mut ctx.report.drawables);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{origin, stream};
    use crate::rage::drawable::{Element, Geometry};
    use crate::rage::sga::BufferFormat;
    use primase_core::{Semantic, Stream};

    /// An install path splits into its archive chain and file; a dictionary entry's hash leads the item.
    #[test]
    fn origins_name_the_archive_file_and_item() {
        let o = origin(
            r"x64i.rpf\levels\gta5\_citye\downtown_01\dt1_03.rpf\dt1_03_build.ydr",
            Some("high model 0 geometry 2"),
        );
        assert_eq!(
            o.archive.as_deref(),
            Some(r"x64i.rpf\levels\gta5\_citye\downtown_01\dt1_03.rpf")
        );
        assert_eq!(&*o.file, "dt1_03_build.ydr");
        assert_eq!(o.item.as_deref(), Some("high model 0 geometry 2"));
        let d = origin(r"a.rpf\props.ydd#0x1234abcd", Some("high model 1 geometry 0"));
        assert_eq!(
            (d.archive.as_deref(), &*d.file, d.item.as_deref()),
            (
                Some("a.rpf"),
                "props.ydd",
                Some("entry 0x1234abcd high model 1 geometry 0")
            )
        );
        let bare = origin("loose.ytd", None);
        assert_eq!((bare.archive, &*bare.file, bare.item), (None, "loose.ytd", None));
    }

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
            bone_ids: vec![],
        };
        let super::Element::Core(semantic, s) = stream(&g, e).unwrap() else {
            panic!("a tangent is a core stream")
        };
        assert_eq!(semantic, Semantic::Tangent);
        let d = crate::components::GTA.direction([1.0, 0.0, 0.0]);
        assert_eq!(s, Stream::F32x4(vec![[d[0], d[1], d[2], -1.0]]));
    }

    /// A geometry's BLENDWEIGHT0 and BLENDINDICES0 index its bone palette: they stay out of the core mesh (no joints
    /// or weights without a skin), kept as stored with the palette per primitive in `gta.blend_streams`; a geometry
    /// without them has no entry, and a mesh with none has no component.
    #[test]
    fn blend_streams_stay_in_the_dialect_as_stored() {
        use crate::components::BlendStreams;
        use crate::rage::drawable::Model;
        let element = |slot, format, offset| Element {
            slot,
            format: BufferFormat(format),
            offset,
        };
        let (position, weights, indices) =
            (element(0, 0x06, 0), element(16, 0x1C, 12), element(20, 0x1E, 16));
        let geometry = |elements: Vec<Element>, stride: u16| {
            let mut vertices = Vec::new();
            for v in 0u8..3 {
                vertices.extend([f32::from(v), 0.0, 0.0].iter().flat_map(|x| x.to_le_bytes()));
                if stride == 20 {
                    vertices.extend([255, 0, 0, v]);
                    vertices.extend([v, 1, 2, 3]);
                }
            }
            Geometry {
                shader: 0,
                bounds: [[0.0; 4]; 2],
                vertex_count: 3,
                stride,
                elements,
                vertices,
                indices: vec![0, 1, 2],
                bone_ids: if stride == 20 { vec![5, 9, 33, 2] } else { vec![] },
            }
        };
        let model = |geometries| Model {
            skeleton_binding: 0,
            render_mask: 0xff,
            geometries,
        };
        let skinned = geometry(vec![position, weights, indices], 20);
        let plain = geometry(vec![position], 12);
        let mut non_finite = std::collections::BTreeMap::new();
        let (mesh, blend) = super::mesh(
            "a.ydr",
            &[model(vec![skinned, plain.clone()])],
            &[],
            &mut non_finite,
        )
        .unwrap();
        for p in mesh.primitives() {
            assert!(
                p.streams
                    .keys()
                    .all(|s| !matches!(s, Semantic::Joints(_) | Semantic::Weights(_))),
                "{:?}",
                p.streams.keys()
            );
        }
        assert_eq!(
            blend.unwrap().0,
            [
                Some(BlendStreams {
                    weights: Some(vec![[255, 0, 0, 0], [255, 0, 0, 1], [255, 0, 0, 2]]),
                    indices: Some(vec![[0, 1, 2, 3], [1, 1, 2, 3], [2, 1, 2, 3]]),
                    palette: vec![5, 9, 33, 2],
                }),
                None
            ]
        );
        let (_, none) = super::mesh("b.ydr", &[model(vec![plain])], &[], &mut non_finite).unwrap();
        assert!(none.is_none());
    }
}
