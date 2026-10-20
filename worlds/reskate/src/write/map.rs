//! A skate-scene world -> a ReSkate Studio normalized map folder (the reference `scene-skate build`).
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "writer I/O: writes the map folder"
)]

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use primase_core::World;

use crate::assembly::{
    EncodingOptions, PartRead, Records, RecordsBasis, SceneWarnings, cell_layout, group_cell, hulls_numbered,
    lay_part, read_part,
};
use crate::collision::{Collision, CollisionResult};
use crate::collision_pack::{Files, Pack, check_packed};
use crate::components::{
    OverlayPiece, OverlayTotals, SkateCellLayout, SkateCollisionPlan, SkateLayoutSummary, StackedPiece,
};
use crate::map::{CellKey, Chains, Extras, MapWriter, Stack, WriterOptions, lay_cell, sum_totals};
use crate::model::CollisionMode;
use crate::records::{CollisionDropped, OverlayStack, SoupStats, StatsJson, Totals};
use crate::scene::{GrindCurve, Marker, PlacedLight, SceneNotes, WaterQuad};

use crate::{Error, Result};

/// What the build did (the log line's numbers).
#[derive(Debug)]
pub struct BuildReport {
    pub out_dir: PathBuf,
    /// The parts the map came in (1: whole).
    pub parts: usize,
    /// The collision cells laid out from their gathered shares (a map in parts), each whole, and their triangles as
    /// filed: the triangles each part brought to those cells as it was written, which the sink dropped (the cells
    /// laid them out already), are these, cell by cell.
    pub cells: usize,
    pub cell_triangles: usize,
    /// The bytes the sink kept of the loaded content it wrote, to compare a later part's copy with (a map in parts): a
    /// digest and a part tag per record, however big the record.
    pub identity_bytes: usize,
    pub tiles: usize,
    pub merged_entities: i64,
    pub merged_tris: i64,
    pub instances: i64,
    pub prototypes: usize,
    pub object_records: usize,
    pub materials: usize,
    pub textures: usize,
    pub collision_tris: i64,
    /// Lights whose basis came from the fallback axis (no usable tangent).
    pub lights_without_tangent: usize,
    /// The most source-mesh bytes the run held at once (loaded band by band): a measure of the run, not of the map,
    /// so the run's report and not `stats.json`.
    pub peak_mesh_bytes: i64,
    /// The overlay stack's lift record (see `WriteResult::lifts`).
    pub lifts: BTreeMap<String, Vec<Vec<f32>>>,
    /// The blend-chain overlays as objects of their own.
    pub blend: crate::blend_order::Stats,
    /// What the writer noticed (`map.json`'s warnings, then `stats.json`'s).
    pub warnings: Vec<String>,
    /// What the writer left out of the map, by what, with counts (a conversion reports each as not written).
    pub omitted: Vec<(String, u64)>,
    /// What it wrote, but not as the world says, by what, with counts (a conversion reports each as approximated).
    pub approximated: Vec<(String, u64)>,
    /// What the world holds that the target does not take ([`target`]'s check of the world written): names with the
    /// entities holding them, a covered name's uncovered entities as `<name> not <its cover>`, and resources (counted
    /// once). A conversion checks the target itself before writing, so this is for direct callers of [`write_map`].
    pub unsupported_names: Vec<(String, u64)>,
}

impl BuildReport {
    /// What a conversion reports of the map: its log lines (the map's numbers, the blend order, how the parts came, the
    /// warnings), what it left out as unsupported notices and what it wrote approximately as approximation notices.
    /// The names the target does not take the conversion reported itself before writing (`unsupported_names` is for
    /// direct callers of [`write_map`]). The blend budget's approximation is the pass's that decided it, reported once.
    pub fn finished(self) -> primase_core::convert::Finished {
        use primase_core::convert::{Finished, Notice, NoticeKind};
        let mut report: primase_core::convert::Report = vec![
            (
                "reskate".into(),
                format!(
                    "{}: tiles={} merged_tris={} instances={} prototypes={} materials={} textures={} collision_tris={}                      peak_mesh_bytes={}",
                    self.out_dir.display(),
                    self.tiles,
                    self.merged_tris,
                    self.instances,
                    self.prototypes,
                    self.materials,
                    self.textures,
                    self.collision_tris,
                    self.peak_mesh_bytes
                ),
            ),
            ("blend order".into(), format!("{:?}", self.blend)),
            (
                "reskate parts".into(),
                format!(
                    "parts={} cells_gathered={} cell_triangles={}",
                    self.parts, self.cells, self.cell_triangles
                ),
            ),
        ];
        // the map's warnings (map.json's), each counted below when it is a loss
        for w in self.warnings {
            report.push(("reskate warning".into(), w));
        }
        let omitted = self.omitted.into_iter().map(|(what, n)| {
            Notice::new(
                "reskate",
                NoticeKind::Unsupported,
                format!("{what}: not written"),
                n,
            )
        });
        let approximated = self
            .approximated
            .into_iter()
            .map(|(what, n)| Notice::new("reskate", NoticeKind::Approximation, what, n));
        Finished {
            report,
            notices: omitted.chain(approximated).collect(),
        }
    }
}

/// What [`write_map`] writes, as a conversion's target: the skate dialect (the `skate.` and `reskate.` names), the
/// core names the writer reads (names, placements, meshes and textures loaded, collision, material variants,
/// priorities, the surface table) and those `raise-skate` raises into the dialect, covered by their raised form
/// (`pbr.material`, `core.material_usage` and `core.draw_order` by `skate.material`, `core.light` by `skate.light`,
/// `core.marker` by `skate.marker`: one without it, as a directional light raise-skate does not raise, is reported as
/// not written). A `core.mesh_file` or `core.texture_file` only beside its loaded mesh or texture;
/// a blend (`core.layered`) is lowered first (`lower-layered-reskate`), one form per prototype chosen first
/// (`select-lod`). Every other name is reported as not written: the map has no place for it.
pub fn target() -> primase_core::pass::Target {
    use crate::components::{
        SkateBaseFactor, SkateBlendBudget, SkateBlendChains, SkateCollision, SkateCollisionPlan,
        SkateFloorPlan, SkateForm, SkateGrindCurve, SkateGrindPrism, SkateLight, SkateMapSummary,
        SkateMarker, SkateMarkerSnaps, SkateMaterial, SkateOverlayStack, SkateParts, SkatePartsOff,
        SkatePlacement, SkatePrototype, SkateRegionClip, SkateRock, SkateScene, SkateSoup, SkateTexture,
        SkateWater,
    };
    use crate::passes::layered::{BlendOpacity, VertexBlend};
    use crate::passes::pause_map::{SkatePauseImage, SkatePausePicture};
    use primase_core::pbr::PbrMaterial;
    use primase_core::{
        Collision, Component, DrawOrder, Instance, LayeredMaterial, Light, Lods, Marker, MaterialUsage,
        MaterialVariants, Mesh, MeshFile, Name, Priority, Resource, Surfaces, Texture, TextureFile,
        Transform,
    };
    primase_core::pass::Target::new("reskate")
        .legal(Name::NAME)
        .legal(Transform::NAME)
        .legal(Instance::NAME)
        .legal(Mesh::NAME)
        .legal(Texture::NAME)
        .legal(Collision::NAME)
        .legal(MaterialVariants::NAME)
        .legal(Priority::NAME)
        .legal(Surfaces::NAME)
        .covered(PbrMaterial::NAME, "covered by skate.material", |w, e| {
            w.has::<SkateMaterial>(e)
        })
        .covered(MaterialUsage::NAME, "covered by skate.material", |w, e| {
            w.has::<SkateMaterial>(e)
        })
        .covered(DrawOrder::NAME, "covered by skate.material", |w, e| {
            w.has::<SkateMaterial>(e)
        })
        .covered(Light::NAME, "covered by skate.light", |w, e| {
            w.has::<SkateLight>(e)
        })
        .covered(Marker::NAME, "covered by skate.marker", |w, e| {
            w.has::<SkateMarker>(e)
        })
        .dynamic(MeshFile::NAME, |w, e| w.has::<Mesh>(e))
        .dynamic(TextureFile::NAME, |w, e| w.has::<Texture>(e))
        .illegal(LayeredMaterial::NAME)
        .illegal(Lods::NAME)
        .legal(SkateScene::NAME)
        .legal(SkateTexture::NAME)
        .legal(SkateMaterial::NAME)
        .legal(SkatePrototype::NAME)
        .legal(SkateParts::NAME)
        .legal(SkatePartsOff::NAME)
        .legal(SkateForm::NAME)
        .legal(SkatePlacement::NAME)
        .legal(SkateCollision::NAME)
        .legal(SkateSoup::NAME)
        .legal(SkateRock::NAME)
        .legal(SkateLight::NAME)
        .legal(SkateGrindCurve::NAME)
        .legal(SkateGrindPrism::NAME)
        .legal(SkateWater::NAME)
        .legal(SkateMarker::NAME)
        .legal(SkateBaseFactor::NAME)
        .legal(SkateFloorPlan::NAME)
        // a map in parts: the floor's summary and decision, applied by prepare-reskate-floor
        .legal(crate::components::SkateFloorSummary::NAME)
        .legal(crate::components::SkateFloorDecision::NAME)
        .legal(crate::components::SkateLightsSummary::NAME)
        .legal(crate::components::SkateTextureWeights::NAME)
        .legal(crate::components::SkateContentBounds::NAME)
        .legal(crate::components::SkateSpawnSummary::NAME)
        .legal(crate::components::SkateInstanceCounts::NAME)
        .legal(crate::components::SkateWaterSurfaces::NAME)
        .legal(crate::components::SkateLayoutSummary::NAME)
        .legal(SkateMarkerSnaps::NAME)
        .legal(SkateOverlayStack::NAME)
        .legal(SkateBlendChains::NAME)
        .legal(SkateBlendBudget::NAME)
        // the map's summary is taken in through the budget planned from it (its basis is checked on writing)
        .legal(SkateMapSummary::NAME)
        .legal(SkateCollisionPlan::NAME)
        // a collision cell of a map in parts: every part's shares of it, its layout and what it is written as
        .legal(crate::components::SkateCellShare::NAME)
        .legal(crate::components::SkateCellLayout::NAME)
        .legal(crate::components::SkateCellCounts::NAME)
        .legal(SkateRegionClip::NAME)
        .legal(SkatePauseImage::NAME)
        .legal(SkatePausePicture::NAME)
        // what the scene loader kept of the files it read: the payloads beside it are what is written
        .legal(crate::components::LoadedFile::NAME)
        .legal(VertexBlend::NAME)
        .legal(BlendOpacity::NAME)
}

/// The map folder of the prepared world `world` (`dir`: where its scene came from, recorded in the stats), encoded
/// as `a` says: the [`MapSink`] given the whole map as its one part.
pub fn write_map(world: &World, dir: &Path, a: &EncodingOptions) -> Result<BuildReport> {
    let mut sink = MapSink::new(dir, a);
    sink.part(world)?;
    sink.finish()
}

/// The map writer as a sink of a map's parts: [`MapSink::part`] lays a part out and writes its payloads at once (its
/// tiles' meshes, the prototypes and textures it is the first to use, the collision cells it completes), keeping only
/// what the map's index files and the report need; [`MapSink::finish`] lays out what is left (the cells still waiting,
/// the water, the grind prisms), writes the index files (`map.json`, the records, `stats.json`) and reports.
///
/// A map in several parts is written as the whole map is, byte for byte, whatever order the parts come in. Its parts
/// carry the map's layout summary (`summarize-reskate-layout`), instancing counts (`summarize-reskate-instances`) and,
/// without a stated region, content extent (`summarize-reskate-content`); a part without the layout summary is the
/// whole map, and a second one is refused. A render tile is drawn by the one part holding all of its placements (else
/// refused, with the tile and the placements of each part: partition aligned to the tile, `partition-grid{tile=T}`).
/// Its collision cells come before its parts, each gathered from every part's shares and planned
/// (`share-reskate-collision`, the cell spool, `prepare-reskate-collision` on the cell): the sink lays each out as its
/// plan says and writes it at once, holding only its objects. A part then brings no collision cell but those laid out
/// (its own cells' triangles are dropped, a cell the gathering did not lay out refused); its hulls it writes whole.
pub struct MapSink {
    dir: PathBuf,
    encoding: EncodingOptions,
    taken: Option<Taken>,
    cells: GatheredCells,
}

/// The collision cells of a map in parts, laid out as they came (before its parts): the map's folder, the cells'
/// layout, which cells with their triangles as filed and the triangles the parts brought to them (dropped), their
/// objects and counts, and the bytes of each file written.
#[derive(Default)]
struct GatheredCells {
    out_dir: Option<PathBuf>,
    layout: Option<SkateCellLayout>,
    keys: BTreeSet<CellKey>,
    triangles: BTreeMap<CellKey, usize>,
    brought: BTreeMap<CellKey, usize>,
    /// The loaded soups brought to each cell (by planning id): a soup two parts both reach counted once.
    brought_soups: BTreeSet<(CellKey, u32)>,
    col: CollisionResult,
    water_col: CollisionResult,
    mesh_sizes: BTreeMap<String, u64>,
}

impl std::fmt::Debug for MapSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapSink")
            .field("dir", &self.dir)
            .field("name", &self.encoding.name)
            .field("taken", &self.taken.as_ref().map(|t| (&t.out_dir, t.parts)))
            .finish()
    }
}

/// What the stats need of a collision soup of the scene: its place in the scene's order, its file, mode, triangles
/// read, cut by the region and kept.
#[derive(Clone, Debug)]
struct SoupFacts {
    order: (bool, primase_core::Entity),
    file: String,
    mode: CollisionMode,
    read: usize,
    cut: i64,
    kept: i64,
}

/// What the map's index files and report need of its scene, over every part.
struct SceneFacts {
    dir: PathBuf,
    soups: Vec<SoupFacts>,
    stats: Option<serde_json::Value>,
    floor_plan: Option<crate::components::SkateFloorPlan>,
    stated_warnings: Vec<String>,
    notes: SceneNotes,
    /// The scene reader's warnings: given by the encoding (the same for every part), else found, merged.
    warnings: Result<Vec<String>, SceneWarnings>,
}

/// The map's markers, water, lights and grind curves, from every part: markers and water by entity (the scene's
/// order), each marker with whether it has a name of its own.
#[derive(Default)]
struct Content {
    markers: Vec<(primase_core::Entity, bool, Marker)>,
    water: Vec<(primase_core::Entity, WaterQuad)>,
    lights: Vec<PlacedLight>,
    grind: Vec<GrindCurve>,
}

/// Whether the sink has not written `e` (of kind `kind`) yet, marking it written: content loaded (below `first_made`,
/// the layout summary's: one id in every part) is written once, however many parts reach it; content a part made, and
/// everything of a map written whole (`first_made` none), is its own.
///
/// A later part's copy of loaded content must be the first's, equal or refused by the SHA-256 of its canonical
/// identity ([`Written::digest`]): one prepared differently by two parts (a light of another energy) is refused, naming
/// both parts by their tags (`part`: this one's; `None`: it holds none of its roots). Content of different identities
/// stays apart, whatever its values. The sink keeps the digest and the tag of the part that wrote it, not the record
/// (written and dropped with its part), so what it holds is fixed per record however big the record is.
fn fresh(
    written: &mut Identities,
    first_made: Option<u32>,
    part: Option<u64>,
    e: primase_core::Entity,
    payload: Written<'_>,
) -> Result<bool> {
    let loaded = first_made.is_some_and(|f| e.index() < f);
    if !loaded {
        return Ok(true);
    }
    let digest = payload.digest();
    match written.entry((payload.kind(), e.index())) {
        std::collections::btree_map::Entry::Vacant(v) => {
            v.insert((part, digest));
            Ok(true)
        }
        std::collections::btree_map::Entry::Occupied(o) if o.get().1 == digest => Ok(false),
        std::collections::btree_map::Entry::Occupied(o) => {
            let name = |p: Option<u64>| {
                p.map_or_else(
                    || "a part holding none of its roots".to_owned(),
                    |t| format!("part {t}"),
                )
            };
            Err(Error::Scene(format!(
                "the {:?} {e:?}, which two parts both reach, is prepared differently by {} and {}: every part of a \
                 map prepares what it shares alike",
                payload.kind(),
                name(o.get().0),
                name(part)
            )))
        }
    }
}

/// Loaded content a map in parts writes once, as the part that holds it gives it (borrowed: compared by its digest).
#[derive(Clone, Copy, Debug)]
enum Written<'a> {
    Light(&'a PlacedLight),
    Marker(bool, &'a Marker),
    Water(&'a WaterQuad),
    Grind(&'a GrindCurve),
    Soup(&'a SoupFacts),
    Hull(&'a crate::model::Hull),
}

/// A record's canonical identity, streamed into SHA-256 as it is read (nothing copied): the record's kind first, then
/// each field after a one-byte tag naming its type, numbers little-endian (every float by its bits: -0.0 and 0.0
/// apart, a NaN equal to itself), text and lists after their length; the fields in a fixed order per kind. Two
/// different records never stream the same bytes.
struct Identity(sha2::Sha256);

impl Identity {
    fn new(kind: Kind) -> Identity {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        h.update([b'R', kind as u8]);
        Identity(h)
    }
    fn put(&mut self, tag: u8, bytes: &[u8]) -> &mut Identity {
        use sha2::Digest;
        self.0.update([tag]);
        self.0.update(bytes);
        self
    }
    fn f32(&mut self, v: f32) -> &mut Identity {
        self.put(b'f', &v.to_bits().to_le_bytes())
    }
    fn f64(&mut self, v: f64) -> &mut Identity {
        self.put(b'd', &v.to_bits().to_le_bytes())
    }
    fn int(&mut self, v: i64) -> &mut Identity {
        self.put(b'i', &v.to_le_bytes())
    }
    /// A list's (or a text's) length, before its items.
    fn len(&mut self, n: usize) -> &mut Identity {
        self.put(b'n', &(n as u64).to_le_bytes())
    }
    fn vec3(&mut self, v: primase_meshops::vector::Vec3) -> &mut Identity {
        self.f32(v.x).f32(v.y).f32(v.z)
    }
    fn text(&mut self, s: &str) -> &mut Identity {
        self.len(s.len());
        self.put(b't', s.as_bytes())
    }
    fn option(&mut self, present: bool) -> &mut Identity {
        self.put(b'o', &[u8::from(present)])
    }
    fn finish(self) -> [u8; 32] {
        use sha2::Digest;
        self.0.finalize().into()
    }
}

impl Written<'_> {
    /// The SHA-256 of its canonical identity ([`Identity`]), compared between the parts that hold it. A marker without
    /// a name of its own is compared without the name its part numbered it by (the map numbers markers over every
    /// part).
    fn digest(&self) -> [u8; 32] {
        let mut i = Identity::new(self.kind());
        match *self {
            Written::Light(l) => {
                i.int(l.entity.index().into()).int(l.spot.into());
                i.vec3(l.pos).vec3(l.dir).vec3(l.tan);
                i.f32(l.r)
                    .f32(l.g)
                    .f32(l.b)
                    .f32(l.energy)
                    .f32(l.range)
                    .f32(l.radius);
                i.f32(l.spot_size)
                    .f32(l.spot_blend)
                    .int(l.time_of_day.into())
                    .int(l.cast_shadow.into());
                i.f32(l.priority).text(&l.group).text(&l.source);
            }
            Written::Marker(named, m) => {
                i.int(named.into()).text(m.kind);
                match (&m.name, named) {
                    (Some(n), true) => {
                        i.option(true).text(n);
                    }
                    _ => {
                        i.option(false);
                    }
                }
                i.f32(m.x).f32(m.y).f32(m.z).f64(m.yaw);
                let s = &m.snap;
                i.int(s.mode as i64)
                    .f32(s.above)
                    .int(s.from_all.into())
                    .f32(s.min_up);
                i.len(s.rings.len());
                for &r in &s.rings {
                    i.f32(r);
                }
                i.int(s.directions.into()).f32(s.max_above_ground).f32(s.headroom);
                i.f64(m.lift).int(m.shelter.into()).int(m.trainer.into());
                i.option(m.blueprint.is_some());
                if let Some(b) = &m.blueprint {
                    i.text(b);
                }
                i.option(m.transform.is_some());
                for &v in m.transform.iter().flatten() {
                    i.f32(v);
                }
            }
            Written::Water(q) => {
                i.f32(q.x0)
                    .f32(q.y0)
                    .f32(q.x1)
                    .f32(q.y1)
                    .f32(q.z)
                    .len(q.corners.len());
                for &(x, y) in &q.corners {
                    i.f32(x).f32(y);
                }
            }
            Written::Grind(g) => {
                i.int(g.entity.index().into())
                    .int(g.surface as i64)
                    .option(g.prism.is_some());
                if let Some(p) = &g.prism {
                    i.len(p.positions.len());
                    for &v in &p.positions {
                        i.vec3(v);
                    }
                    i.len(p.indices.len());
                    for &x in &p.indices {
                        i.int(x.into());
                    }
                    i.int(p.pieces.into()).f32(p.length);
                }
            }
            Written::Soup(f) => {
                i.int(f.order.0.into())
                    .int(f.order.1.index().into())
                    .text(&f.file);
                i.text(&format!("{:?}", f.mode))
                    .int(f.read as i64)
                    .int(f.cut)
                    .int(f.kept);
            }
            Written::Hull(h) => {
                i.int(h.entity.index().into())
                    .text(&h.name)
                    .int(h.surface as i64)
                    .int(h.ground.into());
                i.len(h.pos.len());
                for &v in &h.pos {
                    i.f32(v);
                }
                i.len(h.idx.len());
                for &x in &h.idx {
                    i.int(x.into());
                }
            }
        }
        i.finish()
    }

    fn kind(&self) -> Kind {
        match self {
            Written::Light(_) => Kind::Light,
            Written::Marker(..) => Kind::Marker,
            Written::Water(_) => Kind::Water,
            Written::Grind(_) => Kind::Grind,
            Written::Soup(_) => Kind::Soup,
            Written::Hull(_) => Kind::Hull,
        }
    }
}

/// The SHA-256 of a record's canonical identity ([`Written::digest`]).
type Digest = [u8; 32];

/// The loaded content a map in parts wrote, by kind and planning id: the tag of the part that wrote it and its digest.
type Identities = BTreeMap<(Kind, u32), (Option<u64>, Digest)>;

/// What standalone content of a map in parts the sink writes once ([`Taken::written`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Light,
    Marker,
    Water,
    Grind,
    /// A soup's facts (stats.json).
    Soup,
    /// A hull soup laid out.
    Hull,
}

/// A map in parts: the layout summary, which part each root is in.
struct Layout {
    summary: SkateLayoutSummary,
    tag_of: HashMap<u64, u64>,
}

impl Layout {
    fn new(summary: &SkateLayoutSummary) -> Layout {
        Layout {
            tag_of: summary
                .parts
                .iter()
                .flat_map(|(&t, roots)| roots.iter().map(move |&r| (r, t)))
                .collect(),
            summary: summary.clone(),
        }
    }

    /// Which part `world` is, by the roots it holds; `None`: it holds none of its roots any more.
    fn part_of(&self, world: &World) -> Result<Option<u64>> {
        let mut tag = None;
        for e in world.entities() {
            if let Some(&t) = self.tag_of.get(&u64::from(e.index())) {
                match tag {
                    Some(seen) if seen != t => {
                        return Err(Error::Scene(format!(
                            "one part holds roots of the parts {seen} and {t} of the layout summary: the parts are \
                             not the ones it was made for"
                        )));
                    }
                    _ => tag = Some(t),
                }
            }
        }
        Ok(tag)
    }
}

/// What the sink keeps between parts: the writer (what it wrote, not its payloads), the first part's writer options
/// (every part's must be the same, the scene's notes aside) and preparation records, the scene's facts, the map's
/// markers, water, lights and grind curves, how many placements there were and how many prototypes failed, the names
/// the target does not take; for a map in parts its layout.
struct Taken {
    out_dir: PathBuf,
    first: WriterOptions,
    records: RecordsBasis,
    writer: MapWriter,
    scene: SceneFacts,
    content: Content,
    placements: usize,
    failed: i64,
    unsupported_names: BTreeMap<String, u64>,
    layout: Option<Layout>,
    parts: usize,
    /// The loaded content written so far, by kind and planning id, as the tag of the part that wrote it and the
    /// SHA-256 of its identity: a light, marker, water surface, grind curve, soup or hull soup two parts both reach is
    /// written once, equal or refused ([`fresh`]); a fixed size per record.
    written: Identities,
    /// The bytes of each mesh file written (the map's mesh bytes; a file a later decision removed taken out).
    mesh_sizes: BTreeMap<String, u64>,
    /// Every stacked mesh's overlay totals, and the stacked prototypes' pieces, over every part (a prototype once).
    stack_meshes: BTreeMap<String, OverlayTotals>,
    proto_pieces: BTreeMap<OverlayPiece, StackedPiece>,
    /// The tiles whose blend chains a part recorded, over every part (each tile's chains are one part's).
    chain_tiles: BTreeSet<String>,
}

/// The folder `d` made afresh (empty).
fn fresh_folder(d: &Path) -> Result<()> {
    if d.exists() {
        std::fs::remove_dir_all(d).map_err(|source| Error::Io {
            path: d.to_owned(),
            source,
        })?;
    }
    std::fs::create_dir_all(d).map_err(|source| Error::Io {
        path: d.to_owned(),
        source,
    })
}

/// The files `files` written into the folder `d`.
fn write_files(d: &Path, files: impl IntoIterator<Item = (String, Vec<u8>)>) -> Result<()> {
    for (name, bytes) in files {
        let p = d.join(name);
        std::fs::write(&p, bytes).map_err(|source| Error::Io { path: p, source })?;
    }
    Ok(())
}

impl MapSink {
    /// A sink for the map encoded as `encoding`, its scene from `dir` (recorded in the stats).
    pub fn new(dir: &Path, encoding: &EncodingOptions) -> MapSink {
        MapSink {
            dir: dir.to_owned(),
            encoding: encoding.clone(),
            taken: None,
            cells: GatheredCells::default(),
        }
    }

    /// Take the prepared world `world`: a collision cell of a map in parts (one holding `skate.cell_layout`), laid out
    /// and written now; else a part, laid out and its payloads written (see [`MapSink`]).
    pub fn part(&mut self, world: &World) -> Result<()> {
        if world.resource::<SkateCellLayout>().is_some() {
            return self.cell(world);
        }
        let a = self.encoding.clone();
        let summary = world.resource::<SkateLayoutSummary>();
        if self.taken.as_ref().is_some_and(|t| t.layout.is_none()) {
            return Err(Error::Unsupported(
                "a second part of a map taken as whole (no layout summary, or one naming a single part): the parts \
                 of a map carry its layout summary (summarize-reskate-layout)"
                    .into(),
            ));
        }
        let prepared = target().check_prepared(world);
        // the part as the writer reads it: its options every part's the same (the region stated, else the map's
        // content over every part), its instancing the map's over every part; the scene's notes are the map's, added at
        // the end
        let PartRead {
            scene: mut s,
            mut o,
            cut,
            placements,
            records,
            counts,
            ..
        } = read_part(world, &self.dir, &a, true)?;
        let warnings = match &a.scene_warnings {
            Some(w) => Ok(w.clone()),
            None => Err(SceneWarnings::of(&s, world)?),
        };
        o.warnings = s.stated_warnings.clone();
        let out_dir = a.out_root.join(&o.name);
        if let Some(l) = &self.cells.layout
            && *l != cell_layout(&s, &o)
        {
            return Err(Error::Scene(
                "the map's collision cells were laid out for another map name, surface table, limit or offset than \
                 its parts' (skate.cell_layout): share the collision with the writer's settings"
                    .into(),
            ));
        }
        if self.taken.is_none() {
            // the gathered cells' folder is the map's, made with the first cell
            if self.cells.out_dir.is_none() {
                fresh_folder(&out_dir.join("meshes"))?;
                fresh_folder(&out_dir.join("textures"))?;
            }
            // the writer replays each part's own stack and chains as the part comes (below): made without them, so
            // no copy of the first part's is kept
            let writer = Records {
                stack: None,
                chains: None,
                budget: records.budget.clone(),
                plan: records.plan.clone(),
            }
            .writer(o.clone(), s.surfaces.clone())?;
            self.taken = Some(Taken {
                out_dir: out_dir.clone(),
                first: o.clone(),
                records: records.basis(),
                writer,
                scene: SceneFacts {
                    dir: s.dir.clone(),
                    soups: Vec::new(),
                    stats: s.stats.clone(),
                    floor_plan: s.floor_plan.clone(),
                    stated_warnings: s.stated_warnings.clone(),
                    notes: SceneNotes::default(),
                    warnings: match &warnings {
                        Ok(w) => Ok(w.clone()),
                        Err(_) => Err(SceneWarnings::default()),
                    },
                },
                content: Content::default(),
                placements: 0,
                failed: 0,
                unsupported_names: BTreeMap::new(),
                layout: summary.filter(|l| l.parts.len() > 1).map(Layout::new),
                parts: 0,
                mesh_sizes: BTreeMap::new(),
                stack_meshes: BTreeMap::new(),
                proto_pieces: BTreeMap::new(),
                chain_tiles: BTreeSet::new(),
                written: BTreeMap::new(),
            });
        }
        let t = self.taken.as_mut().expect("taken");
        if o != t.first || records.basis() != t.records || s.surfaces != *t.writer.surfaces() {
            return Err(Error::Scene(
                "the parts of one map disagree on the writer's settings, the preparation records or the surface \
                 table: every part of a map is prepared and written alike"
                    .into(),
            ));
        }
        if let Some(l) = &t.layout
            && summary != Some(&l.summary)
        {
            return Err(Error::Scene(
                "the parts of one map carry different layout summaries".into(),
            ));
        }
        // the layout's basis: it filed the placements in the writer's tiles
        if let Some(s) = summary
            && s.tile != o.tile
        {
            return Err(Error::Scene(format!(
                "the layout summary was made for {} m tiles, the writer's are {} m: summarize-reskate-layout{{tile={}}}",
                s.tile, o.tile, o.tile
            )));
        }
        t.parts += 1;
        t.writer.set_counts(counts);
        t.writer.set_pause_left(crate::map::pause_left_of(world, &o));
        // the part's own overlay stack and blend chains, replayed on its meshes and tiles; a prototype stacked by
        // several parts must be stacked alike (counted once), a tile chained by one
        if let Some(st) = records.stack {
            for (m, totals) in &st.meshes {
                match t.stack_meshes.get(m) {
                    Some(seen) if seen != totals => {
                        return Err(Error::Scene(format!(
                            "{m} is stacked differently by two parts of the map (skate.overlay_stack)"
                        )));
                    }
                    Some(_) => {}
                    None => {
                        t.stack_meshes.insert(m.clone(), *totals);
                    }
                }
            }
            for (p, piece) in st
                .pieces
                .iter()
                .filter(|(p, _)| matches!(p, OverlayPiece::Proto { .. }))
            {
                match t.proto_pieces.get(p) {
                    Some(seen) if seen != piece => {
                        return Err(Error::Scene(format!(
                            "{} is stacked differently by two parts of the map (skate.overlay_stack)",
                            piece.mesh
                        )));
                    }
                    Some(_) => {}
                    None => {
                        t.proto_pieces.insert(*p, piece.clone());
                    }
                }
            }
            t.writer.set_stack(Stack::Replay(Box::new(st)));
        }
        if let Some(ch) = records.chains {
            for name in ch.tiles.keys() {
                if !t.chain_tiles.insert(name.clone()) {
                    return Err(Error::Scene(format!(
                        "{name}'s blend chains are recorded by two parts of the map (skate.blend_chains)"
                    )));
                }
            }
            t.writer.set_chains(Chains::Replay(Box::new(ch)));
        }
        // in parts: which part this is, and every tile it draws held whole by it
        let tiles: BTreeSet<(i32, i32)> = placements.iter().map(|p| (p.tx, p.ty)).collect();
        if let Some(l) = &t.layout {
            let tag = l.part_of(world)?;
            for tile in &tiles {
                let by = l.summary.tiles.get(tile);
                if tag.is_none() || by.is_none_or(|by| by.len() != 1 || !by.contains_key(&tag.expect("told")))
                {
                    return Err(Error::Scene(format!(
                        "render tile t_{}_{} is drawn by a part that does not hold all of its placements (by part, \
                         their planning ids: {:?}): partition the map aligned to the {} m tile \
                         (partition-grid{{tile={}}}, its cell a whole multiple)",
                        tile.0,
                        tile.1,
                        by.cloned().unwrap_or_default(),
                        o.tile,
                        o.tile
                    )));
                }
            }
        }
        // the part laid out: its world soups and tiles' props held, its tiles and prototypes assembled; its hull soups
        // each numbered among the map's
        t.failed += lay_part(&mut s, cut, &placements, &mut t.writer)?;
        let ranks: Option<&BTreeSet<(bool, u64)>> = t.layout.as_ref().map(|l| &l.summary.hulls);
        // loaded content two parts both reach (a dependency of both) is written by the first; what a part made is its
        // own (a whole map writes everything)
        let first_made = t.layout.as_ref().map(|l| l.summary.first_made);
        let world_hulls = hulls_numbered(&mut s, cut, f32::NEG_INFINITY, &|sd, n| match ranks {
            None => Ok(n),
            Some(r) => {
                let key = (sd.raw, u64::from(sd.entity.index()));
                if r.contains(&key) {
                    Ok(r.range(..key).count())
                } else {
                    Err(Error::Scene(format!(
                        "hull soup {:?} is not in the map's layout summary",
                        sd.entity
                    )))
                }
            }
        })?;
        // the part, by its tag: named when two parts prepare what they share differently
        let tag = match &t.layout {
            Some(l) => l.part_of(world)?,
            None => None,
        };
        let mut hulls = Vec::with_capacity(world_hulls.len());
        for h in world_hulls {
            if fresh(&mut t.written, first_made, tag, h.entity, Written::Hull(&h))? {
                hulls.push(h);
            }
        }
        let world_hulls = hulls;
        t.writer.lay_hulls(&world_hulls)?;
        // a placement is in one part (its tile's), so its prototype's hulls are
        let prop_hulls = t.writer.take_prop_hulls();
        t.writer.lay_hulls(&prop_hulls)?;
        // its collision cells: a map written whole lays them out at the end; a map in parts laid each out whole from
        // every part's shares (the gathered cells, recorded as they came), so the triangles a part brings to them are
        // dropped and counted, checked against the cells at the end; a cell the gathering did not lay out is refused
        if self.cells.layout.is_some() || t.layout.is_some() {
            for (c, content) in t.writer.take_cells() {
                if !self.cells.keys.contains(&c) {
                    return Err(Error::Scene(format!(
                        "a part of the map has collision in the cell {c:?}, which no gathered cell laid out: a map in \
                         parts shares its collision per cell (share-reskate-collision in the gather pipeline, after \
                         every pass that changes it)"
                    )));
                }
                // a soup two parts both reach brought to the cell once
                let brought = content.props.iter().map(|(_, _, p)| p.surf.len()).sum::<usize>()
                    + content
                        .segs
                        .iter()
                        .filter(|g| match first_made {
                            Some(f) if g.order.1.index() < f => {
                                self.cells.brought_soups.insert((c, g.order.1.index()))
                            }
                            _ => true,
                        })
                        .map(|g| g.s.len())
                        .sum::<usize>();
                *self.cells.brought.entry(c).or_default() += brought;
            }
        }
        // the part's payloads, written now and not kept
        let meshes = std::mem::take(&mut t.writer.mesh_files);
        t.mesh_sizes
            .extend(meshes.iter().map(|(f, b)| (f.clone(), b.len() as u64)));
        write_files(&out_dir.join("meshes"), meshes)?;
        let used = t.writer.used_textures();
        write_files(&out_dir.join("textures"), t.writer.ctx.tex.take_used(&used)?)?;
        // what the map's index files need of it
        for (name, n) in prepared.unsupported.into_iter().chain(
            prepared
                .unsupported_resources
                .into_iter()
                .map(|r| (format!("resource {r}"), 1)),
        ) {
            *t.unsupported_names.entry(name).or_default() += n;
        }
        t.placements += placements.len();
        t.scene.notes.merge(&s.notes);
        if let (Err(mine), Err(found)) = (&mut t.scene.warnings, warnings) {
            mine.merge(found);
        }
        let w = &mut t.written;
        for sd in &s.soups {
            let f = SoupFacts {
                order: (sd.raw, sd.entity),
                file: sd.file.clone(),
                mode: sd.mode,
                read: sd.surf.len(),
                cut: sd.cut,
                kept: sd.kept,
            };
            if fresh(w, first_made, tag, sd.entity, Written::Soup(&f))? {
                t.scene.soups.push(f);
            }
        }
        let c = &mut t.content;
        let markers = world
            .iter::<crate::components::SkateMarker>()
            .map(|(e, _)| (e, world.has::<primase_core::Name>(e)))
            .zip(std::mem::take(&mut s.markers));
        for ((e, named), m) in markers {
            // an unnamed marker's name is the part's numbering, renamed over the map at the end: not compared
            if fresh(w, first_made, tag, e, Written::Marker(named, &m))? {
                c.markers.push((e, named, m));
            }
        }
        let water = world
            .iter::<crate::components::SkateWater>()
            .map(|(e, _)| e)
            .zip(std::mem::take(&mut s.water));
        for (e, q) in water {
            if fresh(w, first_made, tag, e, Written::Water(&q))? {
                c.water.push((e, q));
            }
        }
        for l in std::mem::take(&mut s.lights) {
            if fresh(w, first_made, tag, l.entity, Written::Light(&l))? {
                c.lights.push(l);
            }
        }
        for g in std::mem::take(&mut s.curves) {
            if fresh(w, first_made, tag, g.entity, Written::Grind(&g))? {
                c.grind.push(g);
            }
        }
        Ok(())
    }

    /// Take a collision cell of a map in parts (a group of its gathered cells: every part's shares of the cell and
    /// their plan): laid out alone as the plan says (exactly without one) and its pieces written now. The cells come
    /// before the parts.
    fn cell(&mut self, world: &World) -> Result<()> {
        if self.taken.is_some() {
            return Err(Error::Scene(
                "a collision cell after a part of the map: the gathered cells come before the parts".into(),
            ));
        }
        let (key, content, surf, layout) = group_cell(world)?;
        let c = &mut self.cells;
        match &c.layout {
            Some(l) if *l != layout => {
                return Err(Error::Scene(
                    "the collision cells of one map disagree on their layout (skate.cell_layout): every part shares \
                     its collision alike"
                        .into(),
                ));
            }
            Some(_) => {}
            None => {
                let out_dir = self.encoding.out_root.join(&layout.name);
                fresh_folder(&out_dir.join("meshes"))?;
                fresh_folder(&out_dir.join("textures"))?;
                c.out_dir = Some(out_dir);
                c.layout = Some(layout.clone());
            }
        }
        if !c.keys.insert(key) {
            return Err(Error::Scene(format!("the collision cell {key:?} is given twice")));
        }
        c.triangles.insert(key, content.triangles());
        let plan = world.resource::<SkateCollisionPlan>();
        let off = primase_meshops::vector::Vec3::new(layout.offset[0], layout.offset[1], layout.offset[2]);
        let mut files = Files::new();
        {
            let mut pack = Pack::new(&surf, off, plan, &mut files)?;
            let mut collision = Collision::new(&surf, off.x, off.y, off.z, layout.limit);
            let res = if key.0 { &mut c.water_col } else { &mut c.col };
            lay_cell(&mut collision, key, content, res, &mut |leaf, r| {
                pack.leaf(leaf, r)
            })?;
            check_packed(plan, pack.seen())?;
        }
        c.mesh_sizes
            .extend(files.iter().map(|(f, b)| (f.clone(), b.len() as u64)));
        write_files(
            &c.out_dir
                .as_ref()
                .expect("made with the first cell")
                .join("meshes"),
            files,
        )?;
        Ok(())
    }

    /// The map laid out to the end (the cells still waiting, the water, the grind prisms, the lights and markers in
    /// the map's order) and its index files written; what the map writer did.
    pub fn finish(self) -> Result<BuildReport> {
        let Some(Taken {
            out_dir,
            mut writer,
            scene: mut s,
            content,
            placements,
            failed,
            unsupported_names,
            parts,
            mut mesh_sizes,
            written,
            records,
            stack_meshes,
            ..
        }) = self.taken
        else {
            return Err(Error::Scene(
                "the map sink was given no part: a map needs its whole".into(),
            ));
        };
        // the markers in the scene's order, those without a name of their own named as the scene names them
        let Content {
            mut markers,
            mut water,
            lights,
            grind,
        } = content;
        markers.sort_by_key(|(e, _, _)| *e);
        let mut spots = 0;
        for (_, named, m) in &mut markers {
            if !*named && m.kind != "spawn" {
                m.name = Some(format!("Spot {spots}"));
            }
            if matches!(m.kind, "travel" | "trainer") {
                spots += 1;
            }
        }
        water.sort_by_key(|(e, _)| *e);
        // the markers still asking for a snap and the curves without a prism, counted over the map's content as
        // written (one two parts both reach once), not summed over the parts
        s.notes.unsnapped = markers
            .iter()
            .filter(|(_, _, m)| m.snap.mode != crate::scene::SnapMode::None)
            .count();
        s.notes.unprepared_curves = grind.iter().filter(|g| g.prism.is_none()).count();
        let (notes, omitted, approximated) = s.notes.render();
        // the overlay totals over every stacked mesh, a prototype once
        if records.stack.is_some() {
            writer.set_overlay_totals(&sum_totals(stack_meshes.values()));
        }
        // the gathered cells laid out already: their objects and counts among the map's
        let GatheredCells {
            keys: cells,
            triangles: cell_triangles,
            brought,
            col,
            water_col,
            mesh_sizes: cell_sizes,
            ..
        } = self.cells;
        // what the parts brought to the gathered cells is what the cells laid out: their collision unchanged since it
        // was shared
        if !cells.is_empty()
            && let Some((c, n)) = cell_triangles
                .iter()
                .find(|(c, n)| brought.get(*c).copied().unwrap_or(0) != **n)
        {
            return Err(Error::Scene(format!(
                "the collision cell {c:?} was laid out from {n} shared triangles, but the parts as written bring {} to \
                 it: a pass changed a part's collision after it was shared (run it in the gather pipeline too)",
                brought.get(c).copied().unwrap_or(0)
            )));
        }
        writer.add_laid_cells(col, water_col);
        mesh_sizes.extend(cell_sizes);
        writer.o.warnings = s.stated_warnings.iter().chain(&notes).cloned().collect();
        writer.finish(Extras {
            hulls: Vec::new(),
            water: water.into_iter().map(|(_, q)| q).collect(),
            lights,
            markers: markers.into_iter().map(|(_, _, m)| m).collect(),
            grind,
        })?;
        match &s.warnings {
            Ok(w) => writer.r.warnings.extend(w.iter().cloned()),
            Err(found) => writer.r.warnings.extend(found.render()),
        }
        // the payloads left (the last cells, water, grind prisms, textures not used before); files given out under a
        // name the texture no longer has renamed, files of a form not taken removed
        let meshes = std::mem::take(&mut writer.mesh_files);
        mesh_sizes.extend(meshes.iter().map(|(f, b)| (f.clone(), b.len() as u64)));
        write_files(&out_dir.join("meshes"), meshes)?;
        for f in &writer.discarded {
            mesh_sizes.remove(f);
            let p = out_dir.join("meshes").join(f);
            if p.exists() {
                std::fs::remove_file(&p).map_err(|source| Error::Io { path: p, source })?;
            }
        }
        let used = writer.used_textures();
        write_files(&out_dir.join("textures"), writer.ctx.tex.take_used(&used)?)?;
        for (given, now) in writer.ctx.tex.renamed() {
            let d = out_dir.join("textures");
            std::fs::rename(d.join(&given), d.join(&now)).map_err(|source| Error::Io {
                path: d.join(&given),
                source,
            })?;
        }
        // the soups in the scene's order, those placed by an entity of their own numbered among them
        s.soups.sort_by_key(|f| f.order);
        for (i, f) in s.soups.iter_mut().enumerate() {
            if f.order.0 {
                f.file = format!("collision/world_{i}.bin");
            }
        }
        writer.r.mesh_bytes = mesh_sizes.values().sum();
        let o = writer.o.clone();
        for (name, bytes) in &writer.files {
            let p = out_dir.join(name);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                    path: parent.to_owned(),
                    source,
                })?;
            }
            std::fs::write(&p, bytes).map_err(|source| Error::Io { path: p, source })?;
        }
        if !writer.files.contains_key("trainer.json") {
            let p = out_dir.join("trainer.json");
            if p.exists() {
                std::fs::remove_file(&p).map_err(|source| Error::Io { path: p, source })?;
            }
        }
        let stats = stats_json(&writer, &s, &o, placements, failed)?;
        let p = out_dir.join("stats.json");
        std::fs::write(&p, stats).map_err(|source| Error::Io { path: p, source })?;
        let r = &writer.r;
        Ok(BuildReport {
            out_dir,
            parts,
            cells: cells.len(),
            cell_triangles: cell_triangles.values().sum(),
            identity_bytes: written.len() * std::mem::size_of::<((Kind, u32), (Option<u64>, Digest))>(),
            tiles: r.tiles.len(),
            merged_entities: r.merged_entities,
            merged_tris: r.merged_tris,
            instances: r.instances,
            prototypes: r.proto_list.len(),
            object_records: r.object_records,
            materials: r.materials,
            textures: writer.ctx.tex.all().iter().filter(|t| !t.removed).count(),
            collision_tris: r.collision.as_ref().map_or(0, |c| c.triangles),
            lights_without_tangent: r.lights_without_tangent,
            peak_mesh_bytes: r.peak_mesh_bytes,
            lifts: r.lifts.clone(),
            blend: r.blend.clone(),
            warnings: o.warnings.iter().chain(&r.warnings).cloned().collect(),
            omitted: omitted
                .into_iter()
                .chain(
                    [
                        ("water outlines with no area", r.water_without_area as i64),
                        (
                            "clip-reskate-region: placements outside the region with nothing within its apron",
                            r.far_dropped,
                        ),
                        (
                            "instanced placements reaching past the map's limit (Studio's extent)",
                            r.limit_instances,
                        ),
                        ("triangles past the map's limit", r.limit_triangles),
                        (
                            "collision triangles past the map's limit less 20 m",
                            r.collision
                                .as_ref()
                                .map_or(0, |c| c.cut_beyond_limit + c.props_beyond_limit),
                        ),
                        ("placements with a degenerate matrix", r.degenerate),
                        (
                            "trainer spots without a height or past the 64 trainer.json takes",
                            r.trainer_left_out as i64,
                        ),
                        ("grind curves with no usable points", r.grind_unusable as i64),
                        (
                            "pause-map picture: triangles of materials casting no shadow left out (leave-out=shadowless)",
                            r.pause_shadowless,
                        ),
                        (
                            "pause-map picture: triangles of the named placements left out (leave-out=placements)",
                            r.pause_placements,
                        ),
                    ]
                    .into_iter()
                    .filter(|(_, n)| *n > 0)
                    .map(|(what, n)| (what.to_owned(), n as u64)),
                )
                .collect(),
            approximated: approximated
                .into_iter()
                .chain(match writer.stack() {
                    Stack::Off { unstacked } if *unstacked > 0 => Some((
                        "overlay pieces drawn where authored (prepare-reskate-overlays stacks them)".to_owned(),
                        *unstacked,
                    )),
                    _ => None,
                })
                .chain(
                    [
                        (
                            "water outlines cut at the map's extent limit",
                            r.water_cut_at_limit as i64,
                        ),
                        (
                            "clip-reskate-region: placements outside the region trimmed to its apron",
                            r.apron_trimmed,
                        ),
                    ]
                    .into_iter()
                    .filter(|(_, n)| *n > 0)
                    .map(|(what, n)| (what.to_owned(), n as u64)),
                )
                .chain(r.pause_untextured.iter().map(|(name, n)| {
                    (
                        format!(
                            "pause-map picture: triangles drawn in their material's mean colour, no texture to sample \
                             (colour=texture): {name}"
                        ),
                        *n as u64,
                    )
                }))
                .chain(match writer.chains() {
                    Chains::Off { left } if *left > 0 => Some((
                        "tiles' vertex-blend chain overlays drawn in the tile, in an unstable order \
                         (prepare-reskate-blend-order splits them)"
                            .to_owned(),
                        *left,
                    )),
                    _ => None,
                })
                .collect(),
            unsupported_names: unsupported_names.into_iter().collect(),
        })
    }
}

/// The map sink as a conversion's writer ([`primase_core::convert::Writer`]): what it writes is [`target`]'s, each part
/// is taken by [`MapSink::part`], and at the end its [`BuildReport`] is the conversion's ([`BuildReport::finished`]).
impl primase_core::convert::Writer for MapSink {
    fn target(&self) -> primase_core::pass::Target {
        target()
    }
    fn write(&mut self, part: &World) -> primase_core::convert::Result<()> {
        Ok(self.part(part)?)
    }
    fn finish(self: Box<Self>) -> primase_core::convert::Result<primase_core::convert::Finished> {
        Ok(MapSink::finish(*self)?.finished())
    }
}

fn stats_json(
    writer: &MapWriter,
    s: &SceneFacts,
    o: &WriterOptions,
    placements: usize,
    failed: i64,
) -> Result<Vec<u8>> {
    let r = &writer.r;
    let col = r.collision.as_ref().expect("finished");
    let full = std::path::absolute(&s.dir).unwrap_or_else(|_| s.dir.clone());
    let kept: Vec<_> = writer.ctx.tex.all().iter().filter(|t| !t.removed).collect();
    let mut formats: BTreeMap<&str, i64> = BTreeMap::new();
    for t in &kept {
        *formats.entry(t.format.as_str()).or_default() += 1;
    }
    let mut top: Vec<(&String, &i64)> = r.prop_col_tris.iter().collect();
    top.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let mut seen = HashSet::new();
    let ov = &r.overlay;
    let stats = StatsJson {
        front_end: s.stats.as_ref(),
        name: &o.name,
        scene: full.to_string_lossy().into_owned(),
        region: o.region,
        tile_size: o.tile,
        lift_m: s.floor_plan.as_ref().map_or(0.0, |f| f.lift),
        totals: Totals {
            placements,
            placements_degenerate: r.degenerate,
            placements_stubs_left_out: s.floor_plan.as_ref().map_or(0, |f| f.placements_removed as i64),
            placed_parts_stubs_left_out: s.floor_plan.as_ref().map_or(0, |f| f.parts_removed as i64),
            placements_clipped_away: r.far_dropped,
            prototypes_placed: r.archetypes,
            prototypes_failed: failed,
            tiles: r.tiles.len(),
            object_records: r.object_records,
            merged_placements: r.merged_entities,
            merged_triangles: r.merged_tris,
            instances: r.instances,
            instanced_triangles_drawn: r.instanced_tris,
            prototypes: r.proto_list.len(),
            prototype_triangles: r.prototype_tris,
            collision_pieces: col.pieces,
            collision_triangles: col.triangles,
            collision_hulls: col.hulls,
            materials: r.materials,
            textures: kept.len(),
            texture_file_bytes: r.tex_bytes,
            texture_pixels: r.tex_pixels,
            mesh_file_bytes: r.mesh_bytes,
            textures_unsupported: writer.ctx.tex.unsupported,
            placements_with_empty_mesh: r.empty_mesh,
            placements_collision_only: r.collision_only,
            lights_written: r.lights_written,
            lights_without_tangent: r.lights_without_tangent,
            grind_curves: r.grind_curves,
            grind_pieces: r.grind_pieces,
            grind_length_m: r.grind_length,
            travel_points: r.travel_points,
            trainer_spots: r.trainer_spots,
            seconds: 0.0,
        },
        soups: s
            .soups
            .iter()
            .map(|sd| SoupStats {
                file: &sd.file,
                mode: match sd.mode {
                    CollisionMode::TriangleMesh => "triangle_mesh",
                    CollisionMode::Water => "water",
                    CollisionMode::Hull => "hull",
                },
                read: sd.read,
                cut_by_region: sd.cut,
                kept_before_cleaning: sd.kept,
            })
            .collect(),
        collision_dropped: CollisionDropped {
            degenerate: col.cut_degenerate,
            beyond_map_edge: col.cut_beyond_limit,
        },
        overlay_stack: OverlayStack {
            step_m: writer.stack().step(),
            overlay_pieces: ov.pieces,
            shifted: ov.shifted,
            water_shifted: ov.water_shifted,
            max_shift_m: ov.max_shift,
            order_cycles_broken: ov.cycles,
        },
        blend_order: {
            let b = &writer.r.blend;
            (b.tiles_split + b.tiles_shared > 0).then_some(b)
        },
        texture_formats: formats,
        prop_collision_top: top
            .into_iter()
            .take(40)
            .map(|(k, v)| (k.clone(), serde_json::Value::from(*v)))
            .collect(),
        warnings: r
            .warnings
            .iter()
            .filter(|x| seen.insert(x.as_str()))
            .map(String::as_str)
            .collect(),
    };
    crate::json_bytes("stats.json", &stats, true)
}
