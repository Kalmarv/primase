//! A map read as the game builds it ([`TrackmaniaReader`]): its block data's blocks placed from their models.
//!
//! A block names its model (`CGameCtnBlockInfo*`, `<collection>\ConstructionBlockInfo\...\<name>.TMED<kind>.Gbx`), whose
//! body lists the mobils (`CSceneMobil`) of each variant: the ground list when the block's flag bit 12 is set, else the
//! air list, indexed by the flags' variant (bits 0 to 5) and sub-variant (bits 6 to 11). The mobil's item shows a solid
//! ([`read_solid`]); each solid read once is a prototype, every block showing it an instance.
//!
//! A block's placement: its model's local space spans its footprint from the origin, `square_size` per unit in X and Z
//! (the units' offsets give the footprint); its direction turns it about +Y, each step taking +X to +Z; the turned
//! footprint's least corner is put at the block's coordinate times the collection's square size and height. Block data
//! version 6 and later store the coordinate plus (1, 0, 1), taken off here.
//!
//! Every cell of the map's area no ground block covers is filled with the collection's default zone's block (Stadium:
//! `StadiumGrass`) at height 0, as the game draws its terrain. A ground block covers the cells of its ground units at
//! height 0 (its mobil draws their terrain), not the rest of its footprint: a bridge arch stands on two of the six
//! cells its footprint spans, the default zone's grass around its feet.
use std::collections::{BTreeMap, BTreeSet};

use gbx_formats::game::blockinfo::{BlockInfo, Body};
use gbx_formats::game::challenge::BlockData;
use gbx_formats::{Gbx, Id, Node};
use primase_core::convert::{self, Notice, NoticeKind, Reader, Report};
use primase_core::frame::SourceFrame;
use primase_core::{
    Collision, Entity, Instance, LodForm, Lods, Mesh, Name, Primitive, Semantic, Source, Stream, Surfaces,
    Topology, Transform, World,
};

use crate::components::{
    TrackmaniaBallProperties, TrackmaniaBlock, TrackmaniaLight, TrackmaniaLightKind,
    TrackmaniaLightProperties, TrackmaniaLights, TrackmaniaMap, TrackmaniaSolid, TrackmaniaSpawn,
    TrackmaniaSpotProperties,
};
use crate::read::files::GameFiles;
use crate::read::library::{At, Library};
use crate::read::material::Materials;
use crate::read::solid::{Affine, GxLight, LightPart, Part, SolidGeometry, read_solid};
use crate::{Error, Result};

const STAGE: &str = "trackmania reader";

/// Flag bit 12: the block stands on the ground.
const FLAG_GROUND: u32 = 1 << 12;
/// Flag bit 13: the block is a clip (GBX.NET's `IsClip`), its sub-variant 63.
const FLAG_CLIP: u32 = 1 << 13;

/// The block info classes' folders and file kinds under `ConstructionBlockInfo`: a block model no collector desc names
/// (a pack file stored under a name hash) is looked for under its name as its file name in each.
const KINDS: [&str; 9] = [
    "Classic",
    "Road",
    "Clip",
    "Flat",
    "Frontier",
    "Pylon",
    "RectAsym",
    "Slope",
    "Transition",
];

/// What a read found and placed, and what it could not.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapReport {
    /// Blocks the map stores.
    pub blocks: usize,
    /// Blocks placed with a prototype.
    pub placed: usize,
    /// Default-zone cells filled.
    pub filled: usize,
    pub prototypes: usize,
    pub spawns: usize,
    /// Block names with no model in the game, counted.
    pub no_model: BTreeMap<String, u32>,
    /// Blocks whose variant's mobil list is empty: their model shows nothing there (a pillar's variant 0).
    pub empty: usize,
    /// Blocks whose variant and sub-variant name no mobil (`name ground|air variant/sub`), counted.
    pub no_mobil: BTreeMap<String, u32>,
    /// Blocks whose mobil's solid is not read, counted by the same label.
    pub unshown: BTreeMap<String, u32>,
    /// Blocks whose direction is not 0 to 3, counted by name: not placed.
    pub bad_direction: BTreeMap<String, u32>,
    /// Solids not read (`path`: why).
    pub no_solid: BTreeMap<String, String>,
    /// GameBox files the game names that could not be read.
    pub unread_files: BTreeMap<String, String>,
    pub materials_unread: BTreeMap<String, String>,
    pub images_unread: BTreeMap<String, String>,
    /// What the solids' walks met and did not read, counted.
    pub skipped: BTreeMap<String, u32>,
}

impl MapReport {
    pub fn lines(&self) -> Report {
        vec![(
            "trackmania map".into(),
            format!(
                "{} blocks, {} placed, {} showing nothing, {} default-zone cells, {} prototypes, {} spawns; {} block \
                 names without a model, {} without a mobil, {} solids, {} files, {} materials and {} images not read",
                self.blocks,
                self.placed,
                self.empty,
                self.filled,
                self.prototypes,
                self.spawns,
                self.no_model.len(),
                self.no_mobil.len(),
                self.no_solid.len(),
                self.unread_files.len(),
                self.materials_unread.len(),
                self.images_unread.len()
            ),
        )]
    }

    fn notices(&self) -> Vec<Notice> {
        let mut out = Vec::new();
        let mut counted = |reason: &str, map: &BTreeMap<String, u32>| {
            if map.is_empty() {
                return;
            }
            let mut n = Notice::new(
                STAGE,
                NoticeKind::Unsupported,
                reason,
                map.values().map(|&c| u64::from(c)).sum(),
            );
            for k in map.keys() {
                n = n.example(k.clone());
            }
            out.push(n);
        };
        counted(
            "blocks whose model the game has no file for, not placed",
            &self.no_model,
        );
        counted(
            "blocks whose variant names no mobil of their model, not placed",
            &self.no_mobil,
        );
        counted(
            "blocks with a direction other than 0 to 3, not placed",
            &self.bad_direction,
        );
        counted(
            "parts of the solids not read (a visual or collision shape of a class not read)",
            &self.skipped,
        );
        let mut listed = |reason: &str, map: &BTreeMap<String, String>| {
            if map.is_empty() {
                return;
            }
            let mut n = Notice::new(STAGE, NoticeKind::Unsupported, reason, map.len() as u64);
            for (k, why) in map {
                n = n.example(format!("{k}: {why}"));
            }
            out.push(n);
        };
        listed("solids not read: what shows them is not placed", &self.no_solid);
        listed("files the game names that could not be read", &self.unread_files);
        listed(
            "materials not read: their parts draw without a material",
            &self.materials_unread,
        );
        listed(
            "images not read: their texture slots have no texture",
            &self.images_unread,
        );
        out
    }
}

/// Reads a map (the bytes of a `.Challenge.Gbx`) with the game's files: every block, its prototype's meshes, materials,
/// textures and collision, the default zone's terrain, and where the car spawns. Everything is read with the map, so a
/// pass that runs on the read world sees the geometry; `load` keeps what the selected world holds.
pub struct TrackmaniaReader<'a> {
    files: &'a GameFiles,
    map: &'a [u8],
    label: String,
    frame: SourceFrame,
    pub report: MapReport,
    noticed: bool,
}

impl std::fmt::Debug for TrackmaniaReader<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackmaniaReader")
            .field("label", &self.label)
            .field("frame", &self.frame)
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

impl<'a> TrackmaniaReader<'a> {
    /// The map `map`, named `label` in reports, read with `files` and converted by `frame` ([`crate::FRAME`]).
    pub fn new(files: &'a GameFiles, map: &'a [u8], label: impl Into<String>, frame: SourceFrame) -> Self {
        TrackmaniaReader {
            files,
            map,
            label: label.into(),
            frame,
            report: MapReport::default(),
            noticed: false,
        }
    }
}

impl Reader for TrackmaniaReader<'_> {
    fn read(&mut self) -> convert::Result<(World, Report)> {
        let (world, report) = Build::run(self.files, self.map, &self.label, self.frame)?;
        self.report = report;
        self.noticed = false;
        Ok((world, self.report.lines()))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> convert::Result<(World, Report)> {
        Ok((world.subset(&world.closure(roots.iter().copied())), vec![]))
    }
    fn load_all(&mut self, world: World) -> convert::Result<(World, Report)> {
        Ok((world, vec![]))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "trackmania"]
    }
    fn prototype_box(&self, world: &World, e: Entity) -> Option<[[f64; 3]; 2]> {
        let mesh = world.get::<Mesh>(e)?;
        let mut bounds = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
        let mut any = false;
        for p in mesh.primitives() {
            for v in p.positions()? {
                any = true;
                for k in 0..3 {
                    bounds[0][k] = bounds[0][k].min(f64::from(v[k]));
                    bounds[1][k] = bounds[1][k].max(f64::from(v[k]));
                }
            }
        }
        any.then_some(bounds)
    }
    fn notices(&mut self) -> Vec<Notice> {
        if std::mem::replace(&mut self.noticed, true) {
            return Vec::new();
        }
        self.report.notices()
    }
}

/// An id's text: its name, or its number.
fn id_text(id: &Id) -> String {
    match id {
        Id::Name(n) => n.clone(),
        Id::Number(n) => collection_name(*n).map_or_else(|| n.to_string(), str::to_owned),
        Id::Empty => String::new(),
    }
}

/// A collection's name by its number, as GBX.NET's collection table gives it (the ones of the games before
/// ManiaPlanet).
fn collection_name(n: u32) -> Option<&'static str> {
    Some(match n {
        0 => "Speed",
        1 => "Alpine",
        2 => "Rally",
        3 => "Island",
        4 => "Bay",
        5 => "Coast",
        6 => "Stadium",
        _ => return None,
    })
}

/// The block info chunks of a block model node.
fn block_info(n: &Node) -> Option<&BlockInfo> {
    Some(match n {
        Node::BlockInfoClassic(b) => &b.info,
        Node::BlockInfoFlat(b) => &b.info,
        Node::BlockInfoRectAsym(b) => &b.info,
        Node::BlockInfoRoad(b) => &b.info,
        Node::BlockInfoClip(b) => &b.info,
        Node::BlockInfoFrontier(b) => &b.info,
        Node::BlockInfoPylon(b) => &b.info,
        _ => return None,
    })
}

/// An affine map as a transform (columns: the images of +X, +Y, +Z, then the translation).
fn transform(a: &Affine) -> Transform {
    let m = &a.rows;
    Transform {
        cols: [
            [m[0][0], m[1][0], m[2][0]],
            [m[0][1], m[1][1], m[2][1]],
            [m[0][2], m[1][2], m[2][2]],
            a.translation,
        ],
    }
}

/// `v` turned `steps` quarter turns about +Y, each taking +X to +Z.
fn turn(v: [f32; 3], steps: u8) -> [f32; 3] {
    (0..steps).fold(v, |[x, y, z], _| [-z, y, x])
}

/// What a block shows.
#[derive(Clone, Copy, Debug)]
enum Shown {
    Prototype(Entity),
    /// Its variant's list is empty: the model shows nothing in that variant (a pillar's variant 0, an air clip).
    Nothing,
    /// Its variant or sub-variant is past its model's lists.
    NoMobil,
    /// Its mobil's solid is not read ([`MapReport::no_solid`]).
    NoSolid,
}

/// A block model, read once.
#[derive(Clone, Debug)]
struct Model {
    path: String,
    at: At,
    /// The footprint in units (X, Z): one past the units' greatest offsets.
    footprint: [i32; 2],
    /// The cells (X, Z offsets in units) of the ground units at height 0: those a ground placement covers.
    ground_cells: Vec<[i32; 2]>,
    waypoint: Option<i32>,
    /// The spawn location on the ground and in the air.
    spawn: Option<(Affine, Affine)>,
}

/// One read's state.
struct Build<'f> {
    files: &'f GameFiles,
    frame: SourceFrame,
    lib: Library<'f>,
    materials: Materials,
    world: World,
    surfaces: Surfaces,
    report: MapReport,
    /// Block model files by lowercased block name.
    model_paths: BTreeMap<String, String>,
    /// `<collection>\ConstructionBlockInfo\`.
    model_root: String,
    models: BTreeMap<String, Option<Model>>,
    /// The prototype of each solid, by the solid node's key.
    prototypes: BTreeMap<(String, u32), Option<Entity>>,
    /// The prototype of each mobil: (model path, ground, variant, sub-variant).
    mobils: BTreeMap<(String, bool, u8, u8), Shown>,
    square: [f32; 2],
}

impl<'f> Build<'f> {
    fn run(files: &'f GameFiles, map: &[u8], label: &str, frame: SourceFrame) -> Result<(World, MapReport)> {
        let gbx = Gbx::parse(map)?;
        let nodes = gbx.nodes()?;
        let Node::Challenge(challenge) = nodes.main() else {
            return Err(Error::Format(format!(
                "{label}: not a map ({})",
                nodes.main().class_name()
            )));
        };
        let data: &BlockData = challenge
            .block_data
            .as_ref()
            .or(challenge.legacy_block_data.as_ref())
            .ok_or_else(|| Error::Format(format!("{label}: the map stores no block data")))?;
        let collection = id_text(&data.map_info.collection);
        let mut b = Build {
            files,
            frame,
            lib: Library::new(files),
            materials: Materials::default(),
            world: World::new(),
            surfaces: Surfaces::default(),
            report: MapReport::default(),
            model_paths: BTreeMap::new(),
            model_root: String::new(),
            models: BTreeMap::new(),
            prototypes: BTreeMap::new(),
            mobils: BTreeMap::new(),
            square: [0.0; 2],
        };
        b.index_models(&collection)?;
        let collection_path = format!("Collections\\{collection}.TMCollection.Gbx");
        let zones = match b.lib.main(&collection_path)?.as_ref().map(At::node) {
            Some(Node::Collection(c)) => c.zones.clone(),
            _ => None,
        }
        .ok_or_else(|| {
            Error::Format(format!(
                "{label}: collection {collection} ({collection_path}) can't be read for its squares"
            ))
        })?;
        b.square = [zones.square_size, zones.square_height];
        b.world.insert_resource(TrackmaniaMap {
            name: data.name.clone(),
            uid: id_text(&data.map_info.id),
            author: id_text(&data.map_info.author),
            decoration: id_text(&data.decoration.id),
            collection: collection.clone(),
            size: data.size,
            square: b.square,
            block_data_version: data.version,
        });

        let mut covered = BTreeSet::new();
        b.report.blocks = data.blocks.len();
        for block in &data.blocks {
            let name = id_text(&block.name);
            let stored = block.coord.map(i32::from);
            let coord = if data.version >= 6 {
                [stored[0] - 1, stored[1], stored[2] - 1]
            } else {
                stored
            };
            let ground = block.flags & FLAG_GROUND != 0;
            let record = TrackmaniaBlock {
                name: name.clone(),
                coord,
                direction: block.direction,
                flags: block.flags,
                ground,
                variant: (block.flags & 63) as u8,
                sub_variant: ((block.flags >> 6) & 63) as u8,
                info: None,
                waypoint: None,
                fill: false,
            };
            if let Some(cells) = b.place(record)?
                && ground
            {
                covered.extend(cells);
            }
        }

        b.fill(&zones, &collection_path, data.size, &covered)?;

        b.world.insert_resource(std::mem::take(&mut b.surfaces));
        b.report.unread_files = std::mem::take(&mut b.lib.unread);
        b.report.materials_unread = std::mem::take(&mut b.materials.materials_unread);
        b.report.images_unread = std::mem::take(&mut b.materials.images_unread);
        b.report.prototypes = b.prototypes.values().flatten().count();
        Ok((b.world, b.report))
    }

    /// Index the collection's block models by their names: each collector of the game's files whose ident is of the
    /// collection, by its ident's name (a model's file name need not be its name: `StadiumRoadMainStartLine` is
    /// `StadiumRoadMainStart.TMEDClassic.Gbx`).
    fn index_models(&mut self, collection: &str) -> Result<()> {
        for (ident, path) in self.files.collectors()? {
            if id_text(&ident.collection).eq_ignore_ascii_case(collection) {
                self.model_paths
                    .entry(id_text(&ident.id).to_lowercase())
                    .or_insert(path);
            }
        }
        self.model_root = format!("{collection}\\ConstructionBlockInfo\\");
        Ok(())
    }

    /// The block model named `name`, read on first use.
    fn model(&mut self, name: &str) -> Result<Option<Model>> {
        let key = name.to_lowercase();
        if let Some(m) = self.models.get(&key) {
            return Ok(m.clone());
        }
        let path = match self.model_paths.get(&key) {
            Some(p) => Some(p.clone()),
            None => KINDS
                .iter()
                .map(|k| format!("{}ConstructionBlockInfo{k}\\{name}.TMED{k}.Gbx", self.model_root))
                .find(|p| self.files.find(p).is_some()),
        };
        let mut model = None;
        if let Some(path) = path
            && let Some(at) = self.lib.main(&path)?
            && let Some(info) = block_info(at.node())
        {
            let info = info.clone();
            let (footprint, ground_cells) = match &info.body {
                Some(body) => self.footprint(&at, body)?,
                None => ([1, 1], vec![[0, 0]]),
            };
            model = Some(Model {
                path,
                waypoint: info.waypoint.as_ref().map(|w| w.kind),
                spawn: info.spawn.map(|(g, a)| {
                    let affine = |i: gbx_formats::plug::Iso4| Affine {
                        rows: i.matrix,
                        translation: i.translation,
                    };
                    (affine(g), affine(a))
                }),
                at,
                footprint,
                ground_cells,
            });
        }
        self.models.insert(key, model.clone());
        Ok(model)
    }

    /// A model's footprint in units: its units' offsets (the ground units, else the air units), one past the greatest;
    /// and the cells of its ground units at height 0.
    fn footprint(&mut self, at: &At, body: &Body) -> Result<([i32; 2], Vec<[i32; 2]>)> {
        let ground = !body.ground_units.is_empty();
        let units = if ground {
            &body.ground_units
        } else {
            &body.air_units
        };
        let mut size = [1, 1];
        let mut cells = Vec::new();
        for unit in units {
            if let Some(u) = self.lib.follow(&at.doc, *unit)?
                && let Node::BlockUnitInfo(info) = u.node()
                && let Some(place) = &info.place
            {
                let [x, y, z] = place.relative_offset;
                size = [size[0].max(x + 1), size[1].max(z + 1)];
                if ground && y == 0 {
                    cells.push([x, z]);
                }
            }
        }
        Ok((size, cells))
    }

    /// Place one block: its entity with its record and placement, an instance of its mobil's prototype. The cells
    /// (X, Z) its ground units at height 0 cover, `None` when it is not placed.
    fn place(&mut self, mut record: TrackmaniaBlock) -> Result<Option<Vec<[i32; 2]>>> {
        let label = format!(
            "{} {} {}/{}",
            record.name,
            if record.ground { "ground" } else { "air" },
            record.variant,
            record.sub_variant
        );
        let Some(model) = self.model(&record.name)? else {
            *self.report.no_model.entry(record.name.clone()).or_default() += 1;
            return Ok(None);
        };
        if record.direction > 3 {
            *self.report.bad_direction.entry(record.name.clone()).or_default() += 1;
            return Ok(None);
        }
        record.info = Some(model.path.clone());
        record.waypoint = model.waypoint;
        let steps = record.direction;
        let [sq, h] = self.square;
        // the turned footprint's least corner, put at the coordinate
        let [fx, fz] = model.footprint.map(|n| n as f32 * sq);
        let corners =
            [[0.0, 0.0, 0.0], [fx, 0.0, 0.0], [0.0, 0.0, fz], [fx, 0.0, fz]].map(|c| turn(c, steps));
        let least = [0, 2].map(|k| corners.iter().map(|c| c[k]).fold(f32::INFINITY, f32::min));
        let [cx, cy, cz] = record.coord;
        // the matrix's columns are the turned axes
        let axes = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].map(|a| turn(a, steps));
        let placement = Affine {
            rows: std::array::from_fn(|r| std::array::from_fn(|c| axes[c][r])),
            translation: [
                cx as f32 * sq - least[0],
                cy as f32 * h,
                cz as f32 * sq - least[1],
            ],
        };
        // each ground cell's centre turned with the block, from the turned footprint's least corner
        let cells = model
            .ground_cells
            .iter()
            .map(|&[ux, uz]| {
                let [x, _, z] = turn([(ux as f32 + 0.5) * sq, 0.0, (uz as f32 + 0.5) * sq], steps);
                [
                    cx + ((x - least[0]) / sq).floor() as i32,
                    cz + ((z - least[1]) / sq).floor() as i32,
                ]
            })
            .collect();

        let shown = self.shown(&model, &record)?;
        let e = self.world.spawn();
        self.world.insert(e, Name(record.name.clone()));
        self.world.insert(e, self.frame.transform(&transform(&placement)));
        match shown {
            Shown::Prototype(prototype) => {
                self.world.insert(
                    e,
                    Instance {
                        prototype,
                        variant: 0,
                    },
                );
                if record.fill {
                    self.report.filled += 1;
                } else {
                    self.report.placed += 1;
                }
            }
            Shown::Nothing => self.report.empty += 1,
            Shown::NoMobil => *self.report.no_mobil.entry(label).or_default() += 1,
            Shown::NoSolid => *self.report.unshown.entry(label).or_default() += 1,
        }
        if !record.fill
            && record.waypoint.is_some_and(|w| w != 3)
            && let Some((on_ground, in_air)) = model.spawn
        {
            let at = if record.ground { on_ground } else { in_air };
            let s = self.world.spawn();
            self.world.insert(s, Name(format!("{} spawn", record.name)));
            self.world
                .insert(s, self.frame.transform(&transform(&placement.then_local(&at))));
            self.world.insert(
                s,
                TrackmaniaSpawn {
                    block: e,
                    waypoint: record.waypoint,
                },
            );
            self.report.spawns += 1;
        }
        self.world.insert(e, record);
        Ok(Some(cells))
    }

    /// What a block shows: the mobil of its variant list (ground or air) at its sub-variant, read on first use. A clip
    /// (flag bit 13) stores sub-variant 63, not an index: it shows its list's first mobil.
    fn shown(&mut self, model: &Model, record: &TrackmaniaBlock) -> Result<Shown> {
        let clip = record.flags & FLAG_CLIP != 0;
        let sub = if clip { 0 } else { record.sub_variant };
        let key = (model.path.to_lowercase(), record.ground, record.variant, sub);
        if let Some(shown) = self.mobils.get(&key) {
            return Ok(*shown);
        }
        let body = block_info(model.at.node()).and_then(|i| i.body.clone());
        let list = body.as_ref().and_then(|b| {
            let lists = if record.ground {
                &b.ground_mobils
            } else {
                &b.air_mobils
            };
            lists.get(usize::from(record.variant)).cloned()
        });
        let shown = match list {
            None => Shown::NoMobil,
            Some(list) if list.is_empty() => Shown::Nothing,
            Some(list) => match list.get(usize::from(sub)) {
                None => Shown::NoMobil,
                Some(&mobil) => {
                    let solid = match self.lib.follow(&model.at.doc, mobil)? {
                        Some(at) => match at.node() {
                            Node::Mobil(m) => {
                                let solid = m.item.as_ref().and_then(|i| i.solid).flatten();
                                self.lib.follow(&at.doc, solid)?
                            }
                            _ => None,
                        },
                        None => None,
                    };
                    match solid {
                        Some(solid) => self.prototype(&solid)?.map_or(Shown::NoSolid, Shown::Prototype),
                        None => Shown::NoSolid,
                    }
                }
            },
        };
        self.mobils.insert(key, shown);
        Ok(shown)
    }

    /// The prototype of the solid at `at`, read on first use.
    fn prototype(&mut self, at: &At) -> Result<Option<Entity>> {
        let key = at.key();
        if let Some(p) = self.prototypes.get(&key) {
            return Ok(*p);
        }
        let read = read_solid(&mut self.lib, at)?;
        let e = match read {
            None => {
                self.report.no_solid.insert(
                    format!("{}#{}", at.doc.path, at.index.0),
                    "not a solid, or a model chain that ends".into(),
                );
                None
            }
            Some((solid, g)) => Some(self.build_prototype(&solid, g)?),
        };
        self.prototypes.insert(key, e);
        Ok(e)
    }

    fn build_prototype(&mut self, solid: &At, g: SolidGeometry) -> Result<Entity> {
        for (what, n) in &g.skipped {
            *self.report.skipped.entry(what.clone()).or_default() += n;
        }
        let name = {
            let file = solid.doc.path.rsplit('\\').next().unwrap_or(&solid.doc.path);
            file.split('.').next().unwrap_or(file).to_owned()
        };
        let e = self.world.spawn();
        let mut levels = g.levels.into_iter();
        let first = levels.next().unwrap_or_default();
        let primitives = self.primitives(first)?;
        if !primitives.is_empty() {
            self.world.insert(e, Mesh::new(primitives));
        }
        let mut forms = Vec::new();
        for (k, parts) in levels.enumerate() {
            let primitives = self.primitives(parts)?;
            let form = self.world.spawn();
            if !primitives.is_empty() {
                self.world.insert(form, Mesh::new(primitives));
            }
            self.world.insert(form, Name(format!("{name} lod{}", k + 1)));
            forms.push(LodForm {
                detail: u8::try_from(k + 1).unwrap_or(u8::MAX),
                form,
            });
        }
        if !forms.is_empty() {
            self.world.insert(
                e,
                Lods {
                    detail: 0,
                    forms,
                    default: None,
                },
            );
        }
        let mut collision = Collision::default();
        for part in &g.collision {
            for (t, surface) in part.triangles.iter().zip(&part.surfaces) {
                let id = self
                    .surfaces
                    .index(surface)
                    .map_err(|e| Error::Format(e.to_string()))?;
                let base = u32::try_from(collision.positions.len()).unwrap_or(u32::MAX);
                let tri = t.map(|i| part.positions[i as usize]);
                collision.positions.extend(tri.map(|p| self.frame.point(p)));
                let tri = if self.frame.flips_winding() {
                    [base, base + 2, base + 1]
                } else {
                    [base, base + 1, base + 2]
                };
                collision.triangles.push(tri);
                collision.surfaces.push(id);
            }
        }
        if !collision.triangles.is_empty() {
            self.world.insert(e, collision);
        }
        if !g.lights.is_empty() {
            let lights = g.lights.iter().map(|l| self.light(l)).collect();
            self.world.insert(e, TrackmaniaLights(lights));
        }
        self.world.insert(e, Name(name));
        self.world
            .insert(e, Source(format!("{}#{}", solid.doc.path, solid.index.0)));
        self.world.insert(
            e,
            TrackmaniaSolid {
                path: solid.doc.path.clone(),
                node: solid.index.0,
                mip_far: g.mip_far,
            },
        );
        Ok(e)
    }

    /// A light tree's light in the world's frame, its Gx light's chunks as stored.
    fn light(&self, part: &LightPart) -> TrackmaniaLight {
        let (kind, base, spot) = match &part.light {
            GxLight::Ball(l) => (TrackmaniaLightKind::Ball, Some(&l.base), None),
            GxLight::Spot(l) => {
                let spot = match (&l.spot, &l.legacy_angles) {
                    (Some(s), _) => Some(TrackmaniaSpotProperties {
                        inner: s.inner,
                        outer: s.outer,
                        falloff_exponent: s.falloff_exponent,
                    }),
                    (None, Some(a)) => Some(TrackmaniaSpotProperties {
                        inner: a.inner,
                        outer: a.outer,
                        falloff_exponent: a.falloff_exponent,
                    }),
                    (None, None) => None,
                };
                (TrackmaniaLightKind::Spot, Some(&l.base), spot)
            }
            GxLight::Frustum(l) => (TrackmaniaLightKind::Frustum, Some(&l.base), None),
            GxLight::Directional(_) => (TrackmaniaLightKind::Directional, None, None),
        };
        let common = match &part.light {
            GxLight::Directional(l) => l.base.common.as_ref(),
            _ => base.and_then(|b| b.base.common.as_ref()),
        };
        TrackmaniaLight {
            tree: part.tree.clone(),
            location: self.frame.transform(&transform(&part.location)),
            kind,
            common: common.map(|c| TrackmaniaLightProperties {
                color: c.color,
                flags: c.flags,
                intensity: c.intensity,
                diffuse_intensity: c.diffuse_intensity,
                specular_intensity: c.specular_intensity,
                specular_power: c.specular_power,
                shadow_intensity: c.shadow_intensity,
                flare_intensity: c.flare_intensity,
                shadow_rgb: c.shadow_rgb,
            }),
            ball: base
                .and_then(|b| b.ball.as_ref())
                .map(|b| TrackmaniaBallProperties {
                    flags: b.flags,
                    radius: b.radius,
                    radius_specular: b.radius_specular,
                    radius_shadow: b.radius_shadow,
                    radius_flare: b.radius_flare,
                    emitting_radius: b.emitting_radius,
                    attenuation1: b.attenuation1,
                    attenuation2: b.attenuation2,
                    ambient_rgb: b.ambient_rgb,
                }),
            spot,
        }
    }

    /// The parts as primitives in the world's frame, each with its material.
    fn primitives(&mut self, parts: Vec<Part>) -> Result<Vec<Primitive>> {
        let mut out = Vec::with_capacity(parts.len());
        for part in parts {
            let material = match &part.material {
                Some(r) => self
                    .materials
                    .material(&mut self.lib, self.files, &mut self.world, r)?,
                None => None,
            };
            let mut streams = BTreeMap::new();
            streams.insert(
                Semantic::Position,
                Stream::F32x3(part.positions.iter().map(|&p| self.frame.point(p)).collect()),
            );
            if let Some(normals) = part.normals {
                streams.insert(
                    Semantic::Normal,
                    Stream::F32x3(normals.into_iter().map(|n| self.frame.direction(n)).collect()),
                );
            }
            if let Some(colors) = part.colors {
                streams.insert(Semantic::Color(0), Stream::F32x4(colors));
            }
            for (k, set) in part.texcoords.into_iter().enumerate() {
                streams.insert(
                    Semantic::TexCoord(u8::try_from(k).unwrap_or(u8::MAX)),
                    Stream::F32x2(set.into_iter().map(|[u, v]| [u, 1.0 - v]).collect()),
                );
            }
            let mut indices = part.indices;
            if self.frame.flips_winding() {
                for t in indices.as_chunks_mut::<3>().0 {
                    t.swap(1, 2);
                }
            }
            out.push(Primitive {
                topology: Topology::Triangles,
                streams,
                indices: Some(indices),
                material,
            });
        }
        Ok(out)
    }

    /// Fill each cell of the map's area that no ground block covers with the default zone's flat block, at height 0.
    fn fill(
        &mut self,
        zones: &gbx_formats::game::collection::CollectionZones,
        collection_path: &str,
        size: [u32; 3],
        covered: &BTreeSet<[i32; 2]>,
    ) -> Result<()> {
        let Some(collection) = self.lib.main(collection_path)? else {
            return Ok(());
        };
        let Some(zone) = self.lib.follow(&collection.doc, zones.default_zone)? else {
            return Ok(());
        };
        let Node::ZoneFlat(flat) = zone.node() else {
            return Ok(());
        };
        let Some(block) = flat.block_infos.and_then(|b| b[0]) else {
            return Ok(());
        };
        let Some(path) = Library::file_of(&zone.doc, Some(block)) else {
            return Ok(());
        };
        let name = {
            let file = path.rsplit('\\').next().unwrap_or(&path);
            file.split('.').next().unwrap_or(file).to_owned()
        };
        let [sx, _, sz] = size.map(|n| i32::try_from(n).unwrap_or(i32::MAX));
        for x in 0..sx {
            for z in 0..sz {
                if covered.contains(&[x, z]) {
                    continue;
                }
                self.place(TrackmaniaBlock {
                    name: name.clone(),
                    coord: [x, 0, z],
                    direction: 0,
                    flags: FLAG_GROUND,
                    ground: true,
                    variant: 0,
                    sub_variant: 0,
                    info: None,
                    waypoint: None,
                    fill: true,
                })?;
            }
        }
        Ok(())
    }
}
