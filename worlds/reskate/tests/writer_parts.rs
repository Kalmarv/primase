//! The map sink over parts (`convert_parts_gathered` with a partition plan's parts): a map written in parts is the
//! map written whole, byte for byte, whatever order the parts come in. Its render tiles are drawn by the part holding
//! their placements (the partition aligned to the tile); its collision cells are gathered from every part's shares
//! (`share-reskate-collision`, the cell spool: a ground soup in a part of its own reaches every cell), each planned on
//! its own and laid out before the parts; its prototypes, textures and materials written once, its markers, lights and
//! water in the map's order. What the spool holds in memory does not grow with the map. A partition not aligned to the
//! tile, and a part bringing collision the gathering did not lay out, are refused.

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use primase_core::convert::{
    ConvertReport, Finished, Gathered, NoticeKind, PartitionPlan, PlannedParts, Reader, Report, Result,
    Summarized, Writer, convert_parts_gathered,
};
use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, Pipeline, Target};
use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::summary::SummaryRegistry;
use primase_core::{
    Collision, ColorSpace, Component, DrawLayer, DrawOrder, Entity, Instance, Layer, LayerWeights,
    LayeredMaterial, Light, LightKind, Mesh, Name, Primitive, Semantic, Stream, Surfaces, TexFormat, TexRole,
    Texture, Topology, Transform, WeightMapping, World,
};
use primase_meshops::PartitionGrid;
use primase_reskate::components::{
    BlendBudgetMode, CollisionSurfaces, CollisionWeld, SkateBlendBudget, SkateCollisionPlan,
    SkateOverlayStack,
};
use primase_reskate::components::{MarkerKind, Other, SkateMarker, SkateWater};
use primase_reskate::passes::layered::{LowerLayeredReskate, OverlayLimit};
use primase_reskate::passes::{
    FinalizeReskateBlendBudget, PlanReskateBlendBudget, PrepareReskateBlendOrder,
    PrepareReskateCellCollision, PrepareReskateCollision, PrepareReskateOverlays, RaiseOptions,
    ShareReskateCollision, SummarizeReskateContent, SummarizeReskateInstances, SummarizeReskateLayout,
    SummarizeReskateMap,
};
use primase_reskate::spool::{CellSpool, SpoolReport};
use primase_reskate::{BuildReport, EncodingOptions, MapSink};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join(format!("primase-reskate-writer-parts-{}", std::process::id()))
        .join(name);
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    d
}

/// A flat grid mesh `n` x `n` quads of `size` metres at height `y`, from the origin east and north.
fn grid(n: u32, size: f32, y: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let (mut pos, mut tris) = (Vec::new(), Vec::new());
    let step = size / n as f32;
    for i in 0..n {
        for j in 0..n {
            let (x, z) = (step * i as f32, -step * j as f32);
            let v = pos.len() as u32;
            pos.extend([
                [x, y, z],
                [x + step, y, z],
                [x, y, z - step],
                [x + step, y, z - step],
            ]);
            tris.extend([[v, v + 1, v + 2], [v + 2, v + 1, v + 3]]);
        }
    }
    (pos, tris)
}

/// A prototype of a grid mesh with its collision, drawn with `material`, and a 1 m decal `decal` lying on it.
#[allow(clippy::too_many_arguments)]
fn prototype(
    w: &mut World,
    name: &str,
    n: u32,
    size: f32,
    y: f32,
    material: Entity,
    surface: u16,
    decal: Entity,
) -> Entity {
    let (pos, tris) = grid(n, size, y);
    let e = w.spawn();
    w.insert(e, Name(name.into()));
    let (dpos, dtris) = grid(1, 1.0, y);
    w.insert(
        e,
        Mesh::new(vec![
            Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(pos.clone()))]),
                indices: Some(tris.iter().flatten().copied().collect()),
                material: Some(material),
            },
            Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([(
                    Semantic::Position,
                    Stream::F32x3(dpos.iter().map(|p| [p[0] + 0.5, p[1], p[2] - 0.5]).collect()),
                )]),
                indices: Some(dtris.iter().flatten().copied().collect()),
                material: Some(decal),
            },
        ]),
    );
    let mut c = Collision::default();
    c.extend(&pos, &tris, surface, |p| p);
    w.insert(e, c);
    e
}

fn place(w: &mut World, prototype: Entity, east: f32, north: f32) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        Instance {
            prototype,
            variant: 0,
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[3] = [east, 0.0, -north];
    w.insert(e, t);
    e
}

/// Where the decks (instanced: 200 triangles, placed five times) and the rocks (drawn into their tiles) stand, by
/// (east, north): around the 400 m partition borders and 200 m tile borders, a rock alone in its cell at the far east.
const DECKS: [(f32, f32); 5] = [
    (150.0, 30.0),
    (370.0, 230.0),
    (410.0, 30.0),
    (790.0, 230.0),
    (805.0, 150.0),
];
const ROCKS: [(f32, f32); 6] = [
    (20.0, 20.0),
    (199.0, 199.0),
    (201.0, 201.0),
    (399.0, 50.0),
    (620.0, 380.0),
    (1450.0, 60.0),
];

/// Where the blended squares stand (merged into their tiles): in three parts.
const BLENDS: [(f32, f32); 3] = [(60.0, 60.0), (500.0, 300.0), (850.0, 60.0)];

/// A 20 m square of stone blended with moss (by its second colour), as a prototype: lowered, its moss layer is a chain
/// overlay of its tile.
fn blended(w: &mut World) -> Entity {
    let layer = |w: &mut World, name: &str, base: [f32; 4]| {
        let m = w.spawn();
        w.insert(m, Name(name.into()));
        w.insert(
            m,
            PbrMaterial {
                base_color: base,
                metallic: 0.0,
                ..PbrMaterial::default()
            },
        );
        Some(Layer {
            material: m,
            texcoord: None,
            transform: None,
        })
    };
    let layers = vec![
        layer(w, "stone layer", [0.5, 0.5, 0.5, 1.0]),
        layer(w, "moss", [0.2, 0.5, 0.2, 1.0]),
    ];
    let blend = w.spawn();
    w.insert(blend, Name("stone and moss".into()));
    w.insert(
        blend,
        LayeredMaterial {
            layers,
            weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
        },
    );
    let proto = w.spawn();
    w.insert(proto, Name("blended".into()));
    w.insert(
        proto,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            material: Some(blend),
            indices: Some(vec![0, 1, 2, 2, 1, 3]),
            streams: BTreeMap::from([
                (
                    Semantic::Position,
                    Stream::F32x3(vec![
                        [0., 0.5, 0.],
                        [20., 0.5, 0.],
                        [0., 0.5, -20.],
                        [20., 0.5, -20.],
                    ]),
                ),
                (
                    Semantic::TexCoord(0),
                    Stream::F32x2(vec![[0., 0.], [1., 0.], [0., 1.], [1., 1.]]),
                ),
                (
                    Semantic::Color(1),
                    Stream::F32x4(vec![
                        [1., 0., 0., 0.],
                        [0.5, 0.5, 0., 0.],
                        [1., 0., 0., 0.],
                        [0.5, 0.5, 0., 0.],
                    ]),
                ),
            ]),
        }]),
    );
    proto
}

/// A raised map: decks and rocks sharing one textured material, flat ground over east -100..1500, north -100..500 (a
/// soup of its own, no transform: a part of its own in a partition), four lights, a spawn, two unnamed travel points and
/// an unnamed trainer spot, one water surface.
fn map() -> World {
    let mut w = World::new();
    let mut surfaces = Surfaces::default();
    let concrete = surfaces.index("concrete").unwrap();
    let wood = surfaces.index("wood").unwrap();
    let metal = surfaces.index("metal").unwrap();
    w.insert_resource(surfaces);
    let tex = w.spawn();
    w.insert(tex, Name("stone".into()));
    w.insert(
        tex,
        Texture {
            width: 2,
            height: 2,
            format: TexFormat::Rgba8Unorm,
            levels: vec![[120, 110, 90, 255].repeat(4)],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        },
    );
    let material = w.spawn();
    w.insert(material, Name("stone".into()));
    w.insert(
        material,
        PbrMaterial {
            base_color_texture: Some(TextureRef::new(tex)),
            ..PbrMaterial::default()
        },
    );
    let paint = w.spawn();
    w.insert(paint, Name("paint".into()));
    w.insert(paint, PbrMaterial::default());
    w.insert(paint, DrawOrder::conventional(DrawLayer::Decal, 2500));
    let deck = prototype(&mut w, "deck", 10, 40.0, 2.0, material, wood, paint);
    let rock = prototype(&mut w, "rock", 1, 3.0, 1.0, material, metal, paint);
    let blended = blended(&mut w);
    for (e, n) in BLENDS {
        place(&mut w, blended, e, n);
    }
    for (e, n) in DECKS {
        place(&mut w, deck, e, n);
    }
    for (e, n) in ROCKS {
        place(&mut w, rock, e, n);
    }
    let ground = w.spawn();
    let mut c = Collision::default();
    for i in 0..16 {
        for j in 0..6 {
            let (x, z) = (-100.0 + 100.0 * i as f32, 100.0 - 100.0 * j as f32);
            c.extend(
                &[
                    [x, 0.0, z],
                    [x + 100.0, 0.0, z],
                    [x, 0.0, z - 100.0],
                    [x + 100.0, 0.0, z - 100.0],
                ],
                &[[0, 1, 2], [2, 1, 3]],
                concrete,
                |p| p,
            );
        }
    }
    w.insert(ground, c);
    for (i, (e, n)) in [(50.0, 50.0), (390.0, 210.0), (420.0, 20.0), (900.0, 300.0)]
        .into_iter()
        .enumerate()
    {
        let l = w.spawn();
        w.insert(
            l,
            Light {
                kind: LightKind::Point,
                color: [1.0; 3],
                intensity: 100.0 * (i + 1) as f32,
                range: Some(20.0),
                cast_shadows: None,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [e, 4.0, -n];
        w.insert(l, t);
    }
    primase_reskate::passes::raise_skate(
        &mut w,
        &RaiseOptions::new(
            "parts",
            primase_reskate::passes::LightCalibration::Standard,
            primase_reskate::passes::Unclassified::LeaveOut,
        )
        .unwrap(),
    )
    .unwrap();
    LowerLayeredReskate::new(OverlayLimit::All)
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    let marker = |w: &mut World, kind: MarkerKind, at: [f32; 2], name: Option<&str>| {
        let e = w.spawn();
        w.insert(
            e,
            SkateMarker {
                kind,
                position: Some((at, Some(1.0))),
                yaw: None,
                m: None,
                blueprint: None,
                snap: None,
                lift: None,
                shelter: None,
                trainer: None,
                other: Other::new(),
            },
        );
        if let Some(n) = name {
            w.insert(e, Name(n.into()));
        }
    };
    marker(&mut w, MarkerKind::Travel, [810.0, -100.0], None);
    marker(&mut w, MarkerKind::Spawn, [100.0, -100.0], Some("start"));
    marker(&mut w, MarkerKind::Trainer, [300.0, -60.0], None);
    marker(&mut w, MarkerKind::Travel, [500.0, -300.0], None);
    let water = w.spawn();
    w.insert(
        water,
        SkateWater {
            corners: vec![[1000.0, 300.0], [1100.0, 300.0], [1100.0, 400.0], [1000.0, 400.0]],
            height: Some(0.5),
            other: Default::default(),
        },
    );
    w
}

/// Removes the far-east rock: everything the part of cell 3,0 holds (`partition-grid{cell=400}`).
#[derive(Debug)]
struct DropFarRock;
impl Pass for DropFarRock {
    fn name(&self) -> &'static str {
        "drop-far-rock"
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> std::result::Result<(), String> {
        let far: Vec<Entity> = w
            .iter::<Instance>()
            .filter(|(e, _)| w.get::<Transform>(*e).is_some_and(|t| t.cols[3][0] > 1200.0))
            .map(|(e, _)| e)
            .collect();
        for e in far {
            w.despawn(e);
        }
        Ok(())
    }
}

/// Adds a collision soup: content a pass adds after the part's collision was shared (the gather traversal ran
/// without it).
#[derive(Debug)]
struct AddSoup;
impl Pass for AddSoup {
    fn name(&self) -> &'static str {
        "add-soup"
    }
    fn provides(&self) -> &[&'static str] {
        &["core.collision"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let e = w.spawn();
        let mut c = Collision::default();
        c.extend(
            &[
                [3000.0, 0.0, -3000.0],
                [3010.0, 0.0, -3000.0],
                [3000.0, 0.0, -3010.0],
            ],
            &[[0, 1, 2]],
            0,
            |p| p,
        );
        w.insert(e, c);
        log.place(e);
        Ok(())
    }
}

fn encoding(out: &Path) -> EncodingOptions {
    encoding_with(out, EncodingOptions::default().cull_ids)
}

/// The map's encoding with `cull_ids` cull IDs.
fn encoding_with(out: &Path, cull_ids: usize) -> EncodingOptions {
    EncodingOptions {
        out_root: out.to_path_buf(),
        name: Some("parts".into()),
        cull_ids,
        ..EncodingOptions::default()
    }
}

/// The map written whole, after `passes`; and the collision plan they recorded.
fn whole(out: &Path, passes: Vec<Box<dyn Pass>>) -> BuildReport {
    whole_planned(out, passes).0
}

fn whole_planned(out: &Path, passes: Vec<Box<dyn Pass>>) -> (BuildReport, Option<SkateCollisionPlan>) {
    let (r, w) = whole_with(&encoding(out), passes);
    (r, w.resource::<SkateCollisionPlan>().cloned())
}

/// The map written whole as `e` says, after `passes`; and the world written.
fn whole_with(e: &EncodingOptions, passes: Vec<Box<dyn Pass>>) -> (BuildReport, World) {
    let mut w = map();
    for p in passes {
        p.run(&mut w, &mut PassLog::default()).unwrap();
    }
    (primase_reskate::write_map(&w, Path::new(""), e).unwrap(), w)
}

fn plan() -> PrepareReskateCollision {
    PrepareReskateCollision::new(
        CollisionSurfaces::Coalesce,
        CollisionWeld::Millimetre,
        &EncodingOptions::default(),
    )
    .unwrap()
}

/// Loads each part's closure under the same ids; any prototype's box is the deck's.
struct Planned(World);
impl Reader for Planned {
    fn read(&mut self) -> Result<(World, Report)> {
        Ok((self.0.clone(), Vec::new()))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        Ok((world.subset(&world.closure(roots.iter().copied())), Vec::new()))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "skate", "reskate", "pbr", "test"]
    }
    fn prototype_box(&self, _world: &World, _prototype: Entity) -> Option<[[f64; 3]; 2]> {
        Some([[0.0, 0.0, -40.0], [40.0, 2.0, 0.0]])
    }
}

/// The map sink as a conversion's writer.
struct Sink(Option<MapSink>, Rc<RefCell<Option<BuildReport>>>);
impl Writer for Sink {
    fn target(&self) -> Target {
        primase_reskate::target()
    }
    fn write(&mut self, part: &World) -> Result<()> {
        self.0.as_mut().unwrap().part(part)?;
        Ok(())
    }
    fn finish(mut self: Box<Self>) -> Result<Finished> {
        *self.1.borrow_mut() = Some(self.0.take().unwrap().finish()?);
        Ok(Finished::default())
    }
}

/// A conversion in parts' passes: `passes` in each part as it is written; `gather` in each part before its collision
/// is shared (the passes that change it; `share-reskate-collision` follows them); `group` on each collision cell;
/// `finalize` once, on every summary.
#[derive(Default)]
struct Stages {
    passes: Vec<Box<dyn Pass>>,
    gather: Vec<Box<dyn Pass>>,
    group: Vec<Box<dyn Pass>>,
    finalize: Vec<Box<dyn Pass>>,
}

impl Stages {
    /// `passes` in each part, and in the gather traversal too.
    fn both(passes: impl Fn() -> Vec<Box<dyn Pass>>) -> Stages {
        Stages {
            passes: passes(),
            gather: passes(),
            ..Stages::default()
        }
    }

    /// The collision cells planned (`prepare-reskate-cell-collision` on each), as [`plan`] plans the whole map.
    fn planned(mut self) -> Stages {
        self.group.push(Box::new(PrepareReskateCellCollision::new(
            CollisionSurfaces::Coalesce,
            CollisionWeld::Millimetre,
        )));
        self
    }
}

/// What a conversion in parts did: the sink's report, the conversion's, the spool's, and how many parts there were.
struct InParts {
    built: BuildReport,
    converted: ConvertReport,
    spool: SpoolReport,
    parts: usize,
}

/// The map written in the parts of `grid`, their groups in the order `order` gives (`None`: the plan's), with `passes`
/// in each part (and in the gather traversal).
fn in_parts(
    out: &Path,
    grid: &PartitionGrid,
    order: Option<&[usize]>,
    passes: impl Fn() -> Vec<Box<dyn Pass>>,
) -> Result<InParts> {
    in_parts_staged(&encoding(out), grid, order, Stages::both(passes), 200.0)
}

/// The map `map` written as `e` says in the parts of `grid`, their groups in the order `order` gives (`None`: the
/// plan's), through `stages`, the layout summarized for `layout_tile` metre tiles; the cell spool in `e`'s output
/// root.
fn in_parts_staged(
    e: &EncodingOptions,
    grid: &PartitionGrid,
    order: Option<&[usize]>,
    stages: Stages,
    layout_tile: f32,
) -> Result<InParts> {
    in_parts_of(map(), e, grid, order, stages, layout_tile)
}

/// [`in_parts_staged`] of the map `w`.
fn in_parts_of(
    mut w: World,
    e: &EncodingOptions,
    grid: &PartitionGrid,
    order: Option<&[usize]>,
    stages: Stages,
    layout_tile: f32,
) -> Result<InParts> {
    grid.run(&mut w, &mut PassLog::default()).unwrap();
    let mut plan = w.resource::<PartitionPlan>().unwrap().clone();
    if let Some(order) = order {
        plan.groups = order.iter().map(|&i| plan.groups[i].clone()).collect();
    }
    let parts = plan.groups.len();
    w.insert_resource(plan);
    let mut registry = SummaryRegistry::new();
    primase_reskate::register_summaries(&mut registry);
    let report = Rc::new(RefCell::new(None));
    let summarize: Vec<Box<dyn Pass>> = vec![
        Box::new(SummarizeReskateLayout::new(layout_tile).unwrap()),
        Box::new(SummarizeReskateInstances),
        Box::new(SummarizeReskateContent),
    ];
    let mut gather = stages.gather;
    gather.push(Box::new(
        ShareReskateCollision::new(&EncodingOptions::default()).unwrap(),
    ));
    let mut spool = CellSpool::new(&e.out_root.join("spool"))?;
    let converted = convert_parts_gathered(
        &mut Planned(w),
        &mut PlannedParts::default(),
        Box::new(Sink(Some(MapSink::new(Path::new(""), e)), report.clone())),
        &PassManager::new(PassRegistry::new()),
        &Pipeline { passes: Vec::new() },
        &Summarized {
            registry: &registry,
            summarize: &Pipeline { passes: summarize },
            decide: &Pipeline { passes: Vec::new() },
        },
        Gathered {
            spool: &mut spool,
            gather: &Pipeline { passes: gather },
            group: &Pipeline { passes: stages.group },
            finalize: &Pipeline {
                passes: stages.finalize,
            },
        },
        &Pipeline {
            passes: stages.passes,
        },
    )?;
    let built = report.take().unwrap();
    Ok(InParts {
        built,
        converted,
        spool: spool.report(),
        parts,
    })
}

/// Every file of a folder, by its path within it.
fn files(d: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![d.to_path_buf()];
    while let Some(p) = stack.pop() {
        for e in std::fs::read_dir(&p).unwrap() {
            let e = e.unwrap().path();
            if e.is_dir() {
                stack.push(e);
            } else {
                let rel = e.strip_prefix(d).unwrap().to_string_lossy().replace('\\', "/");
                out.insert(rel, std::fs::read(&e).unwrap());
            }
        }
    }
    out
}

fn same_maps(a: &Path, b: &Path, what: &str) {
    let (fa, fb) = (files(a), files(b));
    assert_eq!(
        fa.keys().collect::<Vec<_>>(),
        fb.keys().collect::<Vec<_>>(),
        "{what}: the files"
    );
    for (k, v) in &fa {
        assert!(fb[k] == *v, "{what}: {k} differs");
    }
}

fn aligned() -> PartitionGrid {
    PartitionGrid::new(400.0)
        .unwrap()
        .with_tile(200.0)
        .unwrap()
        .with_halo(50.0)
        .unwrap()
}

#[test]
fn a_map_written_in_parts_is_the_map_written_whole_in_any_order() {
    let reference = tmp("whole");
    let r = whole(&reference, Vec::new());
    assert_eq!(r.parts, 1);
    assert!(r.instances >= 5 && r.tiles >= 4 && r.collision_tris > 0, "{r:?}");
    let reference = reference.join("parts");
    let mut orders: Vec<Option<Vec<usize>>> = vec![None];
    let n = in_parts(&tmp("count"), &aligned(), None, Vec::new).unwrap().parts;
    assert!(n >= 5, "{n} parts");
    orders.push(Some((0..n).rev().collect()));
    orders.push(Some((0..n).map(|i| (i * 3 + 1) % n).collect()));
    for (k, order) in orders.iter().enumerate() {
        let out = tmp(&format!("parts{k}"));
        let r = in_parts(&out, &aligned(), order.as_deref(), Vec::new).unwrap();
        assert_eq!(r.built.parts, r.parts, "{order:?}");
        // every cell the ground and the props reach gathered from every part, each laid out once, the spool emptied
        assert!(r.built.cells >= 6, "{order:?}: {} cells", r.built.cells);
        assert_eq!(r.spool.cells, r.built.cells, "{order:?}");
        assert!(
            r.spool.shares > r.spool.cells,
            "a cell shared by several parts: {:?}",
            r.spool
        );
        // the triangles the parts brought to the gathered cells as written, dropped: what they shared
        assert!(r.spool.triangles > 0);
        assert_eq!(r.built.cell_triangles, r.spool.triangles, "{order:?}");
        assert!(
            !out.join("spool").exists(),
            "{order:?}: the spool's folder is removed"
        );
        same_maps(&reference, &out.join("parts"), &format!("{order:?}"));
    }
}

#[test]
fn a_part_left_without_its_roots_changes_nothing() {
    let reference = tmp("whole-dropped");
    whole(&reference, vec![Box::new(DropFarRock)]);
    let out = tmp("parts-dropped");
    let drop = || -> Vec<Box<dyn Pass>> { vec![Box::new(DropFarRock)] };
    in_parts(&out, &aligned(), None, drop).unwrap();
    same_maps(&reference.join("parts"), &out.join("parts"), "dropped");
}

/// A pause picture leaving out placements by a prefix no part's placement matches: each part only counts it (it may
/// hold none of them), and the sink, which sees every part, refuses it at the end.
#[test]
fn a_pause_prefix_no_part_matches_is_refused_by_the_sink() {
    use primase_reskate::passes::{
        PauseColour, PauseDrawing, PauseExtent, PauseLeaveOut, PauseSource, PrepareReskatePauseMap,
    };
    let pause = || -> Vec<Box<dyn Pass>> {
        vec![Box::new(
            PrepareReskatePauseMap::new(PauseSource::Geometry {
                height: 90,
                extent: PauseExtent::Content,
                drawing: PauseDrawing {
                    leave_out: PauseLeaveOut::Placements(["Level Bounds/Cieling".to_owned()].into()),
                    colour: PauseColour::Mean,
                },
            })
            .unwrap(),
        )]
    };
    let e = in_parts(&tmp("pause-prefix"), &aligned(), None, pause)
        .err()
        .unwrap()
        .to_string();
    assert!(
        e.contains("no placement's name starts with Level Bounds/Cieling"),
        "{e}"
    );
}

#[test]
fn a_misaligned_partition_and_collision_not_gathered_are_refused() {
    // 300 m cells, not aligned to the 200 m tile: a tile's placements in two parts
    let e = in_parts(
        &tmp("misaligned"),
        &PartitionGrid::new(300.0).unwrap().with_halo(50.0).unwrap(),
        None,
        Vec::new,
    )
    .err()
    .unwrap()
    .to_string();
    assert!(
        e.contains("render tile t_1_")
            && e.contains("does not hold all of its placements")
            && e.contains("tile=200"),
        "{e}"
    );
    // a soup a part's pass adds after its collision was shared, in a cell no gathered cell laid out
    let stages = Stages {
        passes: vec![Box::new(AddSoup)],
        ..Stages::default()
    };
    let e = in_parts_staged(&encoding(&tmp("ungathered")), &aligned(), None, stages, 200.0)
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("no gathered cell laid out"), "{e}");
    // a part's collision removed after it was shared: the cell was laid out with triangles the part no longer brings
    let stages = Stages {
        passes: vec![Box::new(DropFarRock)],
        ..Stages::default()
    };
    let e = in_parts_staged(&encoding(&tmp("changed")), &aligned(), None, stages, 200.0)
        .err()
        .unwrap()
        .to_string();
    assert!(
        e.contains("a pass changed a part's collision after it was shared"),
        "{e}"
    );
}

/// The collision plan over parts: each collision cell, gathered from every part's shares, is planned on its own by the
/// pass's rule, the plan of every cell together the one the pass records for the whole map (leaf by leaf), the map
/// byte for byte the same; the rocks' few metal triangles are written as their leaf's wood or concrete, reported by
/// the cells' passes among the conversion's notices; a part of a map in several plans none.
#[test]
fn the_collision_plan_over_parts_is_the_whole_maps_leaf_by_leaf() {
    let reference = tmp("whole-planned");
    let (_, recorded) = whole_planned(&reference, vec![Box::new(plan())]);
    let recorded = recorded.unwrap();
    assert!(
        recorded.leaves.values().any(|l| !l.into.is_empty()),
        "the plan rewrites a minor surface somewhere: {:?}",
        recorded.leaves
    );
    let mut order: Option<Vec<usize>> = None;
    for _ in 0..2 {
        let out = tmp("parts-planned");
        let r = in_parts_staged(
            &encoding(&out),
            &aligned(),
            order.as_deref(),
            Stages::default().planned(),
            200.0,
        )
        .unwrap();
        assert!(r.parts >= 2);
        // the cells' passes report what they rewrote, as the whole map's pass does: the same triangles
        let rewritten = |what: &str| -> u64 {
            r.converted
                .notices()
                .iter()
                .filter(|n| n.kind == NoticeKind::Approximation && n.reason.contains(what))
                .map(|n| n.count)
                .sum()
        };
        let whole_rewritten: usize = recorded
            .leaves
            .values()
            .flat_map(|l| l.into.keys().map(|s| l.counts[s]))
            .sum();
        assert!(
            rewritten("collision triangles of surface metal") > 0,
            "{:?}",
            r.converted.notices()
        );
        assert_eq!(
            rewritten("collision triangles of surface") as usize,
            whole_rewritten
        );
        same_maps(
            &reference.join("parts"),
            &out.join("parts"),
            &format!("planned {order:?}"),
        );
        order = Some((0..r.parts).rev().collect());
    }
    // a part of a map in several plans no collision of its own
    let e = in_parts(
        &tmp("part-planned"),
        &aligned(),
        None,
        || -> Vec<Box<dyn Pass>> { vec![Box::new(plan())] },
    )
    .err()
    .unwrap()
    .to_string();
    assert!(e.contains("a part of a map in several plans no collision"), "{e}");
}

/// Rocks in pairs, a pair's two in one 256 m collision cell (east 150 and 210 m, north 1000 m apart pair from pair) but
/// in two parts (`partition-grid{cell=200,tile=200}`: the 200 m columns 0 and 1), and a spawn.
fn pairs(n: usize) -> World {
    let mut w = World::new();
    let mut surfaces = Surfaces::default();
    let metal = surfaces.index("metal").unwrap();
    w.insert_resource(surfaces);
    let material = w.spawn();
    w.insert(material, Name("stone".into()));
    w.insert(material, PbrMaterial::default());
    let rock = prototype(&mut w, "rock", 4, 3.0, 1.0, material, metal, material);
    for k in 0..n {
        let north = 1000.0 * k as f32 + 50.0;
        place(&mut w, rock, 150.0, north);
        place(&mut w, rock, 210.0, north);
    }
    primase_reskate::passes::raise_skate(
        &mut w,
        &RaiseOptions::new(
            "parts",
            primase_reskate::passes::LightCalibration::Standard,
            primase_reskate::passes::Unclassified::LeaveOut,
        )
        .unwrap(),
    )
    .unwrap();
    let e = w.spawn();
    w.insert(
        e,
        SkateMarker {
            kind: MarkerKind::Spawn,
            position: Some(([100.0, -100.0], Some(1.0))),
            yaw: None,
            m: None,
            blueprint: None,
            snap: None,
            lift: None,
            shelter: None,
            trainer: None,
            other: Other::new(),
        },
    );
    w
}

/// The order that gives every pair's first part before any pair's second: the parts of column 0 (east 0..200 m), then
/// those of column 1, by the east of their roots.
fn firsts_then_seconds(w: &World, grid: &PartitionGrid) -> Vec<usize> {
    let mut w = w.clone();
    grid.run(&mut w, &mut PassLog::default()).unwrap();
    let plan = w.resource::<PartitionPlan>().unwrap();
    let east = |i: usize| {
        plan.groups[i]
            .roots
            .iter()
            .filter_map(|&e| w.get::<Transform>(e).map(|t| t.cols[3][0]))
            .fold(f32::INFINITY, f32::min)
    };
    let mut order: Vec<usize> = (0..plan.groups.len()).collect();
    order.sort_by(|&a, &b| east(a).total_cmp(&east(b)).then(a.cmp(&b)));
    order
}

/// The cell spool's memory does not grow with the map: rocks in pairs sharing a collision cell across two parts, given
/// every pair's first part before any second, the most share payload the spool holds at once is the same for 2, 8 and
/// 32 pairs (one cell's, or one part's), while what it writes grows with them; each map is the map written whole, the
/// cells planned the same.
#[test]
fn the_spools_memory_does_not_grow_with_the_map() {
    let grid = PartitionGrid::new(200.0)
        .unwrap()
        .with_tile(200.0)
        .unwrap()
        .with_halo(20.0)
        .unwrap();
    let mut peaks = Vec::new();
    let mut written = Vec::new();
    for n in [2usize, 8, 32] {
        let reference = tmp(&format!("whole-pairs-{n}"));
        let mut w = pairs(n);
        plan().run(&mut w, &mut PassLog::default()).unwrap();
        primase_reskate::write_map(&w, Path::new(""), &encoding(&reference)).unwrap();
        let w = pairs(n);
        let order = firsts_then_seconds(&w, &grid);
        let out = tmp(&format!("parts-pairs-{n}"));
        let r = in_parts_of(
            w,
            &encoding(&out),
            &grid,
            Some(&order),
            Stages::default().planned(),
            200.0,
        )
        .unwrap();
        // a part per rock, and the spawn's
        assert_eq!(r.parts, 2 * n + 1, "{n} pairs");
        assert_eq!(r.built.cells, n, "a cell per pair");
        assert_eq!(r.spool.shares, 2 * n, "two shares per cell");
        same_maps(
            &reference.join("parts"),
            &out.join("parts"),
            &format!("{n} pairs"),
        );
        peaks.push(r.spool.peak_payload_bytes);
        written.push(r.spool.bytes_written);
    }
    assert!(peaks[0] > 0, "{peaks:?}");
    assert!(
        peaks.iter().all(|&p| p == peaks[0]),
        "the peak grows with the map: {peaks:?}"
    );
    assert!(written[0] < written[1] && written[1] < written[2], "{written:?}");
}

/// The routes' preparation of each part, for a map encoded with `cull_ids`: the overlays stacked, the blend chains
/// split.
fn part_prepared(cull_ids: usize) -> Vec<Box<dyn Pass>> {
    let e = EncodingOptions {
        cull_ids,
        ..EncodingOptions::default()
    };
    vec![
        Box::new(
            PrepareReskateOverlays::new(0.004)
                .unwrap()
                .for_encoding(&e)
                .unwrap(),
        ),
        Box::new(
            PrepareReskateBlendOrder::new(200.0)
                .unwrap()
                .for_encoding(&e)
                .unwrap(),
        ),
    ]
}

/// The routes' preparation of the whole map, for a map encoded with `cull_ids`: each part's, the collision planned, the
/// map summarized, the chain objects budgeted from the cull-ID pool.
fn prepared(cull_ids: usize) -> Vec<Box<dyn Pass>> {
    let e = EncodingOptions {
        cull_ids,
        ..EncodingOptions::default()
    };
    let mut passes = part_prepared(cull_ids);
    passes.push(Box::new(plan()));
    passes.push(Box::new(SummarizeReskateMap::for_encoding(&e).unwrap()));
    passes.push(Box::new(PlanReskateBlendBudget::pool()));
    passes
}

/// The whole map's recorded budget for `cull_ids`: how many tiles it ranks and gives the chain form.
fn budget_for(cull_ids: usize) -> SkateBlendBudget {
    let mut w = map();
    for p in prepared(cull_ids) {
        p.run(&mut w, &mut PassLog::default()).unwrap();
    }
    w.resource::<SkateBlendBudget>().unwrap().clone()
}

/// Keeps the blend budget of the world it runs on (the last it saw).
#[derive(Default)]
struct KeepBudget(Arc<Mutex<Option<SkateBlendBudget>>>);
impl Pass for KeepBudget {
    fn name(&self) -> &'static str {
        "keep-budget"
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> std::result::Result<(), String> {
        *self.0.lock().unwrap() = w.resource::<SkateBlendBudget>().cloned();
        Ok(())
    }
}

/// The notices a conversion reports of the blend budget: kind, reason and count (which pass decided it, the whole
/// map's or the finalize stage's, apart).
fn budget_notices(r: &ConvertReport) -> Vec<(NoticeKind, String, u64)> {
    r.notices()
        .into_iter()
        .filter(|n| n.stage.contains("reskate-blend-budget"))
        .map(|n| (n.kind, n.reason, n.count))
        .collect()
}

/// The map converted whole as `e` says, after `passes`: the sink's report and the conversion's.
fn converted_whole(e: &EncodingOptions, passes: Vec<Box<dyn Pass>>) -> (BuildReport, ConvertReport) {
    converted_whole_of(map(), e, passes)
}

/// [`converted_whole`] of the map `w`.
fn converted_whole_of(
    w: World,
    e: &EncodingOptions,
    passes: Vec<Box<dyn Pass>>,
) -> (BuildReport, ConvertReport) {
    let report = Rc::new(RefCell::new(None));
    let converted = primase_core::convert::convert(
        &mut Planned(w),
        Box::new(Sink(Some(MapSink::new(Path::new(""), e)), report.clone())),
        &PassManager::new(PassRegistry::new()),
        &Pipeline { passes: Vec::new() },
        &Pipeline { passes },
    )
    .unwrap();
    (report.take().unwrap(), converted)
}

/// The overlays, blend chains, collision plan and pool budget over parts: each part stacks its own meshes (the deck,
/// drawn by several parts, stacked alike by each) and chains its own tiles; the gather traversal summarizes each part's
/// content with its collision cells apart, the cells' groups count theirs, and the finalize stage decides the pool
/// budget once from the two (`finalize-reskate-blend-budget`) by the whole map's rule. The decision with its basis is
/// the whole map's, as are the writer's blend record, the budget's structured approximation notice (the tiles left
/// plain, reported once) and the maps byte for byte: at the pool's boundary (room for every tile's chain objects), one
/// ID short of it, and saturated (no tile's chains fit).
#[test]
fn the_preparation_and_the_pool_budget_over_parts_are_the_whole_maps() {
    // the boundary: the fewest cull IDs with which every ranked tile takes the chain form
    let ranked = |b: &SkateBlendBudget| b.decision.as_ref().unwrap().ranked.len();
    let chosen = |b: &SkateBlendBudget| b.decision.as_ref().unwrap().chosen.len();
    let roomy = budget_for(1_000_000);
    assert_eq!(ranked(&roomy), BLENDS.len(), "{roomy:?}");
    assert_eq!(chosen(&roomy), BLENDS.len());
    let other = roomy.decision.as_ref().unwrap().pool.unwrap().other_cull_ids;
    let mut boundary = other;
    while chosen(&budget_for(boundary)) < BLENDS.len() {
        boundary += 1;
    }
    assert!(chosen(&budget_for(boundary - 1)) < BLENDS.len());
    // saturated: the most cull IDs with which no tile takes it
    let mut saturated = boundary;
    while chosen(&budget_for(saturated)) > 0 {
        saturated -= 1;
    }
    let reason = "tiles whose blend chains are written plain, past the chain-object budget (drawn in an unstable order)";
    for cull_ids in [boundary, boundary - 1, saturated] {
        let reference = tmp(&format!("whole-pool-{cull_ids}"));
        let kept = KeepBudget::default();
        let recorded = kept.0.clone();
        let mut passes = prepared(cull_ids);
        passes.push(Box::new(kept));
        let (r, whole) = converted_whole(&encoding_with(&reference, cull_ids), passes);
        let recorded = recorded.lock().unwrap().clone().unwrap();
        let plain = ranked(&recorded) - chosen(&recorded);
        assert_eq!(
            r.blend.tiles_split + r.blend.tiles_shared,
            BLENDS.len(),
            "{:?}",
            r.blend
        );
        assert_eq!(r.blend.tiles_shared, plain, "{:?}", r.blend);
        let expected_notices = budget_notices(&whole);
        if plain > 0 {
            assert_eq!(
                expected_notices,
                [(NoticeKind::Approximation, reason.to_owned(), plain as u64)],
                "{cull_ids}"
            );
        } else {
            assert!(expected_notices.is_empty(), "{expected_notices:?}");
        }
        // in parts: each part's content summarized with its cells apart in the gather traversal, the collision planned
        // and counted cell by cell, the budget decided once in the finalize stage and handed to every part
        let out = tmp(&format!("parts-pool-{cull_ids}"));
        let kept = KeepBudget::default();
        let made = kept.0.clone();
        let e = encoding_with(&out, cull_ids);
        let mut passes = part_prepared(cull_ids);
        passes.push(Box::new(kept));
        let mut gather = part_prepared(cull_ids);
        gather.push(Box::new(SummarizeReskateMap::for_encoding(&e).unwrap()));
        let stages = Stages {
            passes,
            gather,
            finalize: vec![Box::new(FinalizeReskateBlendBudget::pool())],
            ..Stages::default()
        }
        .planned();
        let p = in_parts_staged(&e, &aligned(), None, stages, 200.0).unwrap();
        assert!(p.parts >= 3);
        let made = made.lock().unwrap().clone().unwrap();
        assert_eq!(made, recorded, "{cull_ids}: the decision and its basis");
        assert_eq!(p.built.blend, r.blend, "{cull_ids}");
        assert_eq!(budget_notices(&p.converted), expected_notices, "{cull_ids}");
        same_maps(
            &reference.join("parts"),
            &out.join("parts"),
            &format!("pool {cull_ids}"),
        );
    }
    // the saturated map leaves every tile plain
    assert_eq!(chosen(&budget_for(saturated)), 0);
}

/// The pool budget of the map `w()` decided whole (`plan-reskate-blend-budget`) and in the parts of [`aligned`]
/// (`finalize-reskate-blend-budget` on the gather traversal's summaries and the cells' counts), each written as
/// `cull_ids` say: the two decisions, with their bases; the maps compared byte for byte.
fn pool_budgets(w: &dyn Fn() -> World, cull_ids: usize, tag: &str) -> (SkateBlendBudget, SkateBlendBudget) {
    pool_budgets_with(w, cull_ids, tag, &Vec::new)
}

/// [`pool_budgets`], `extra` first in the whole map's passes and in each part's (both traversals).
fn pool_budgets_with(
    w: &dyn Fn() -> World,
    cull_ids: usize,
    tag: &str,
    extra: &dyn Fn() -> Vec<Box<dyn Pass>>,
) -> (SkateBlendBudget, SkateBlendBudget) {
    let reference = tmp(&format!("whole-{tag}"));
    let kept = KeepBudget::default();
    let recorded = kept.0.clone();
    let mut passes = extra();
    passes.extend(prepared(cull_ids));
    passes.push(Box::new(kept));
    converted_whole_of(w(), &encoding_with(&reference, cull_ids), passes);
    let out = tmp(&format!("parts-{tag}"));
    let kept = KeepBudget::default();
    let made = kept.0.clone();
    let e = encoding_with(&out, cull_ids);
    let mut passes = extra();
    passes.extend(part_prepared(cull_ids));
    passes.push(Box::new(kept));
    let mut gather = extra();
    gather.extend(part_prepared(cull_ids));
    gather.push(Box::new(SummarizeReskateMap::for_encoding(&e).unwrap()));
    let stages = Stages {
        passes,
        gather,
        finalize: vec![Box::new(FinalizeReskateBlendBudget::pool())],
        ..Stages::default()
    }
    .planned();
    in_parts_of(w(), &e, &aligned(), None, stages, 200.0).unwrap();
    same_maps(&reference.join("parts"), &out.join("parts"), tag);
    let recorded = recorded.lock().unwrap().clone().unwrap();
    let made = made.lock().unwrap().clone().unwrap();
    (recorded, made)
}

/// [`map`] with a second water surface, at the far west: two parts write a water surface each, the map one water
/// object.
fn two_waters() -> World {
    let mut w = map();
    let water = w.spawn();
    w.insert(
        water,
        SkateWater {
            corners: vec![[0.0, 300.0], [100.0, 300.0], [100.0, 400.0], [0.0, 400.0]],
            height: Some(0.5),
            other: Default::default(),
        },
    );
    w
}

/// The map's other cull IDs over parts count what the writer writes once once: the water, one object however many
/// parts bring a surface of it, and a light, a hull soup or a grind prism two parts both write (reached from both
/// parts' roots), each by its key. The decision of a map in parts, with its basis, is the whole map's.
#[test]
fn shared_content_and_water_are_counted_once_over_parts() {
    for (tag, w) in [
        ("two-waters", &two_waters as &dyn Fn() -> World),
        ("shared", &shared_map),
    ] {
        let roomy = pool_budgets(w, 1_000_000, &format!("{tag}-roomy"));
        assert_eq!(roomy.1, roomy.0, "{tag}");
        let basis = roomy.0.decision.as_ref().unwrap().pool.unwrap();
        // and with little room, a pool a double count would change
        let (whole, parts) = pool_budgets(w, basis.other_cull_ids + BLENDS.len(), &format!("{tag}-tight"));
        assert_eq!(parts, whole, "{tag}");
    }
}

/// Puts in a blend budget with its mode and no decision.
struct Undecided;
impl Pass for Undecided {
    fn name(&self) -> &'static str {
        "undecided"
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.blend_budget"]
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> std::result::Result<(), String> {
        w.insert_resource(SkateBlendBudget {
            mode: BlendBudgetMode::Pool,
            decision: None,
        });
        Ok(())
    }
}

/// The writer of a map in parts replays a decided budget and decides none: parts whose budget states its mode and no
/// decision are refused.
#[test]
fn a_map_in_parts_with_an_undecided_budget_is_refused() {
    let out = tmp("parts-undecided");
    let mut passes = part_prepared(EncodingOptions::default().cull_ids);
    passes.push(Box::new(Undecided));
    let stages = Stages {
        passes,
        ..Stages::default()
    }
    .planned();
    let e = in_parts_staged(&encoding(&out), &aligned(), None, stages, 200.0)
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("states its mode and no decision"), "{e}");
}

/// Changes the deck's recorded stack in the part holding the deck at east 410 m: two parts then disagree on a
/// prototype they both draw.
#[derive(Debug)]
struct TamperDeck;
impl Pass for TamperDeck {
    fn name(&self) -> &'static str {
        "tamper-deck"
    }
    fn modifies(&self) -> &[&'static str] {
        &["skate.overlay_stack"]
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> std::result::Result<(), String> {
        let holds = w.resource::<primase_core::convert::PartScope>().is_some_and(|s| {
            s.roots
                .iter()
                .any(|&e| w.get::<Transform>(e).is_some_and(|t| t.cols[3][0] == 410.0))
        });
        if holds
            && let Some(st) = w.resource_mut::<SkateOverlayStack>()
            && let Some(t) = st.meshes.get_mut("proto deck")
        {
            t.pieces += 1;
        }
        Ok(())
    }
}

/// A prototype two parts stack differently is refused, by its name.
#[test]
fn a_prototype_stacked_differently_by_two_parts_is_refused() {
    let e = EncodingOptions::default();
    let passes: Vec<Box<dyn Pass>> = vec![
        Box::new(
            PrepareReskateOverlays::new(0.004)
                .unwrap()
                .for_encoding(&e)
                .unwrap(),
        ),
        Box::new(TamperDeck),
    ];
    let e = in_parts_staged(
        &encoding(&tmp("tampered")),
        &aligned(),
        None,
        Stages {
            passes,
            ..Stages::default()
        },
        200.0,
    )
    .err()
    .unwrap()
    .to_string();
    assert!(
        e.contains("proto deck is stacked differently by two parts"),
        "{e}"
    );
}

/// A layout summarized for other tiles than the writer's is refused, naming both.
#[test]
fn a_layout_for_other_tiles_is_refused() {
    let e = in_parts_staged(
        &encoding(&tmp("other-tiles")),
        &aligned(),
        None,
        Stages::default(),
        100.0,
    )
    .err()
    .unwrap()
    .to_string();
    assert!(e.contains("made for 100 m tiles, the writer's are 200 m"), "{e}");
}

/// Refers to the entities it holds: what holds them brings them into its part (as a reader's closure does).
#[derive(Clone, Debug, PartialEq)]
struct Holds(Vec<Entity>);
impl Component for Holds {
    const NAME: &'static str = "test.holds";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(&self.0);
    }
}

/// [`map`] with content two parts both reach: a light, a travel marker, a water surface and a world soup (a 20 m
/// square at east 300), each held by two holders standing in different parts (east 150 and east 610), so each part's
/// closure brings them in.
fn shared_map() -> World {
    let mut w = map();
    let l = w.spawn();
    w.insert(
        l,
        primase_reskate::components::SkateLight {
            spot: false,
            position: [300.0, 6.0, -120.0],
            direction: [0.0, -1.0, 0.0],
            tangent: [0.0; 3],
            color: [1.0; 3],
            energy: 40.0,
            range: 15.0,
            radius: 0.1,
            spot_size: 0.0,
            spot_blend: 0.0,
            time_of_day: 127,
            cast_shadow: false,
            priority: None,
            group: String::new(),
            unknown_flags: 0,
        },
    );
    let m = w.spawn();
    w.insert(
        m,
        SkateMarker {
            kind: MarkerKind::Travel,
            position: Some(([300.0, -120.0], Some(1.0))),
            yaw: None,
            m: None,
            blueprint: None,
            // a snap left to the writer: counted once among the map's approximations
            snap: Some(primase_reskate::components::Snap::Mode(
                primase_reskate::components::SnapMode::Ground,
            )),
            lift: None,
            shelter: None,
            trainer: None,
            other: Other::new(),
        },
    );
    let water = w.spawn();
    w.insert(
        water,
        SkateWater {
            corners: vec![[290.0, 110.0], [310.0, 110.0], [310.0, 130.0], [290.0, 130.0]],
            height: Some(0.25),
            other: Default::default(),
        },
    );
    let soup = w.spawn();
    let concrete = w
        .resource::<Surfaces>()
        .unwrap()
        .clone()
        .index("concrete")
        .unwrap();
    let mut c = Collision::default();
    c.extend(
        &[
            [290.0, 0.5, -110.0],
            [310.0, 0.5, -110.0],
            [290.0, 0.5, -130.0],
            [310.0, 0.5, -130.0],
        ],
        &[[0, 1, 2], [2, 1, 3]],
        concrete,
        |p| p,
    );
    w.insert(soup, c);
    for east in [150.0, 610.0] {
        let h = w.spawn();
        w.insert(h, Holds(vec![l, m, water, soup]));
        let mut t = Transform::IDENTITY;
        t.cols[3] = [east, 0.0, -120.0];
        w.insert(h, t);
    }
    w
}

/// Content two parts both reach (a light, a marker, a water surface, a world soup) is written once: the map in parts
/// is the map written whole, byte for byte, the soup's cell gathered once and its triangles brought once, the marker's
/// unresolved snap counted once.
#[test]
fn content_two_parts_reach_is_written_once() {
    let reference = tmp("whole-shared");
    let mut w = shared_map();
    plan().run(&mut w, &mut PassLog::default()).unwrap();
    primase_reskate::write_map(&w, Path::new(""), &encoding(&reference)).unwrap();
    for order in [None, Some(true)] {
        let out = tmp("parts-shared");
        let w = shared_map();
        let order: Option<Vec<usize>> = order.map(|_| {
            let mut p = w.clone();
            aligned().run(&mut p, &mut PassLog::default()).unwrap();
            (0..p.resource::<PartitionPlan>().unwrap().groups.len())
                .rev()
                .collect()
        });
        let r = in_parts_of(
            w,
            &encoding(&out),
            &aligned(),
            order.as_deref(),
            Stages::default().planned(),
            200.0,
        )
        .unwrap();
        assert_eq!(
            r.built.cell_triangles,
            r.spool.triangles - 2,
            "the shared soup's two triangles, shared twice"
        );
        assert!(
            r.built
                .approximated
                .iter()
                .any(|(what, n)| what.starts_with("markers whose snap is unresolved") && *n == 1),
            "{:?}",
            r.built.approximated
        );
        same_maps(
            &reference.join("parts"),
            &out.join("parts"),
            &format!("shared {order:?}"),
        );
    }
}

/// Puts a light 3 m above each rock the world owns (a part's roots; the whole map's every rock), placed for it: lights
/// each part makes, two parts' under one local index (each part's first made id is the same).
#[derive(Debug)]
struct LightOverRocks;
impl Pass for LightOverRocks {
    fn name(&self) -> &'static str {
        "light-over-rocks"
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.light"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let roots = w
            .resource::<primase_core::convert::PartScope>()
            .map(|s| s.roots.clone());
        let at: Vec<[f32; 3]> = w
            .iter::<Instance>()
            .filter(|(e, i)| {
                roots.as_ref().is_none_or(|r| r.contains(e))
                    && w.get::<Name>(i.prototype).is_some_and(|n| n.0 == "rock")
            })
            .filter_map(|(e, _)| {
                w.get::<Transform>(e)
                    .map(|t| [t.cols[3][0], t.cols[3][1] + 3.0, t.cols[3][2]])
            })
            .collect();
        for position in at {
            let l = w.spawn();
            w.insert(
                l,
                primase_reskate::components::SkateLight {
                    spot: false,
                    position,
                    direction: [0.0, -1.0, 0.0],
                    tangent: [0.0; 3],
                    color: [1.0; 3],
                    energy: 40.0,
                    range: 15.0,
                    radius: 0.1,
                    spot_size: 0.0,
                    spot_blend: 0.0,
                    time_of_day: 127,
                    cast_shadow: false,
                    priority: None,
                    group: String::new(),
                    unknown_flags: 0,
                },
            );
            log.place(l);
        }
        Ok(())
    }
}

/// Lights two parts each make under one local index are two lights, counted apart: the pool budget's basis in parts
/// counts every light the whole map does, and the decision and the map are the whole map's.
#[test]
fn lights_two_parts_make_under_one_index_are_counted_apart() {
    let extra = || -> Vec<Box<dyn Pass>> { vec![Box::new(LightOverRocks)] };
    let (whole, parts) = pool_budgets_with(&map, 1_000_000, "made-lights", &extra);
    let basis = |b: &SkateBlendBudget| b.decision.as_ref().unwrap().pool.unwrap();
    assert_eq!(parts, whole);
    let (_, plain) = pool_budgets_with(&map, 1_000_000, "no-made-lights", &Vec::new);
    // a light per rock: as many cull IDs more as there are rocks
    assert_eq!(
        basis(&whole).other_cull_ids,
        basis(&plain).other_cull_ids + ROCKS.len()
    );
}

/// Brightens the shared light (at east 300) in the part holding the holder at east 610 only: the two parts then
/// prepare one loaded light differently.
#[derive(Debug)]
struct BrightenShared;
impl Pass for BrightenShared {
    fn name(&self) -> &'static str {
        "brighten-shared"
    }
    fn modifies(&self) -> &[&'static str] {
        &["skate.light"]
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> std::result::Result<(), String> {
        let holds = w.resource::<primase_core::convert::PartScope>().is_some_and(|s| {
            s.roots
                .iter()
                .any(|&e| w.has::<Holds>(e) && w.get::<Transform>(e).is_some_and(|t| t.cols[3][0] == 610.0))
        });
        if holds && let Some(mut lights) = w.column_mut::<primase_reskate::components::SkateLight>() {
            for (_, l) in lights.iter_mut().filter(|(_, l)| l.position[0] == 300.0) {
                l.energy *= 2.0;
            }
        }
        Ok(())
    }
}

/// A light two parts both reach, prepared differently by them, is refused, naming both parts; written alike, once.
#[test]
fn shared_content_prepared_differently_is_refused() {
    let stages = Stages {
        passes: vec![Box::new(BrightenShared)],
        ..Stages::default()
    }
    .planned();
    let e = in_parts_of(
        shared_map(),
        &encoding(&tmp("shared-differently")),
        &aligned(),
        None,
        stages,
        200.0,
    )
    .err()
    .unwrap()
    .to_string();
    assert!(
        e.contains("Light") && e.contains("prepared differently by part") && e.matches("part ").count() >= 2,
        "{e}"
    );
}

/// [`shared_map`] with a hull soup of `n` triangles (a fan of thin slivers, its own part: no transform), loaded and
/// written once.
fn with_hull(n: u32) -> World {
    let mut w = shared_map();
    let mut positions = vec![[700.0, 0.5, -20.0]];
    let mut triangles = Vec::new();
    for k in 0..=n {
        positions.push([710.0, 0.5 + k as f32 * 0.01, -20.0 - k as f32 * 0.01]);
        if k > 0 {
            triangles.push([0, k, k + 1]);
        }
    }
    let surfaces = vec![0; triangles.len()];
    let h = w.spawn();
    w.insert(
        h,
        Collision {
            positions,
            triangles,
            surfaces,
        },
    );
    w.insert(
        h,
        primase_reskate::components::SkateCollision(primase_reskate::components::SoupRef {
            table: primase_reskate::components::TableRef::single("collision/hull.bin"),
            mode: Some(primase_reskate::components::CollisionMode::Hull),
            ground: None,
            region_cut: None,
            other: Default::default(),
        }),
    );
    w
}

/// What the sink keeps of the loaded content it writes, to compare a later part's copy with, is a digest per record:
/// a hull soup of 2,000 triangles leaves it holding what one of 2 does.
#[test]
fn the_sink_keeps_a_digest_per_shared_record_not_the_record() {
    let mut kept = Vec::new();
    for n in [2u32, 2000] {
        let r = in_parts_of(
            with_hull(n),
            &encoding(&tmp(&format!("hull-{n}"))),
            &aligned(),
            None,
            Stages::default().planned(),
            200.0,
        )
        .unwrap();
        kept.push(r.built.identity_bytes);
    }
    assert!(kept[0] > 0, "{kept:?}");
    assert_eq!(
        kept[0], kept[1],
        "the sink's records grow with the hull: {kept:?}"
    );
}
