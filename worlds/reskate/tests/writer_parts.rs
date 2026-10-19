//! The map sink over parts (`convert_parts_summarized` with a partition plan's parts): a map written in parts is the
//! map written whole, byte for byte, whatever order the parts come in. Its render tiles are drawn by the part holding
//! their placements (the partition aligned to the tile), its collision cells laid out once every part announced for
//! them has come (a ground soup in a part of its own reaches every cell), its prototypes, textures and materials
//! written once, its markers, lights and water in the map's order. A part a pass leaves with none of its roots is not
//! told, and the cells waiting for it are laid out at the end, with the same result. A partition not aligned to the
//! tile, and a part adding collision where it announced none, are refused.

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use primase_core::convert::{
    Finished, PartitionPlan, PlannedParts, Reader, Report, Result, Summarized, Writer,
    convert_parts_summarized,
};
use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, Pipeline, Target};
use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::summary::SummaryRegistry;
use primase_core::{
    Collision, ColorSpace, DrawLayer, DrawOrder, Entity, Instance, Layer, LayerWeights, LayeredMaterial,
    Light, LightKind, Mesh, Name, Primitive, Semantic, Stream, Surfaces, TexFormat, TexRole, Texture,
    Topology, Transform, WeightMapping, World,
};
use primase_meshops::PartitionGrid;
use primase_reskate::components::{
    BlendBudgetMode, CollisionSurfaces, CollisionWeld, SkateBlendBudget, SkateCollisionPlan,
    SkateOverlayStack,
};
use primase_reskate::components::{MarkerKind, Other, SkateMarker, SkateWater};
use primase_reskate::passes::layered::{LowerLayeredReskate, OverlayLimit};
use primase_reskate::passes::{
    PlanReskateBlendBudget, PrepareReskateBlendOrder, PrepareReskateCollision, PrepareReskateOverlays,
    RaiseOptions, SummarizeReskateContent, SummarizeReskateInstances, SummarizeReskateLayout,
    SummarizeReskateMap,
};
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
        &RaiseOptions::new("parts", primase_reskate::passes::LightCalibration::Standard).unwrap(),
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

/// Adds a collision soup nothing announced: a pass's content after the layout summary was made.
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
        &["core", "skate", "reskate", "pbr"]
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

/// The map written in the parts of `grid`, their groups in the order `order` gives (`None`: the plan's), with `passes`
/// in each part.
fn in_parts(
    out: &Path,
    grid: &PartitionGrid,
    order: Option<&[usize]>,
    passes: Vec<Box<dyn Pass>>,
) -> Result<(BuildReport, usize)> {
    in_parts_with(&encoding(out), grid, order, passes)
}

/// The map written as `e` says in the parts of `grid`, as [`in_parts`].
fn in_parts_with(
    e: &EncodingOptions,
    grid: &PartitionGrid,
    order: Option<&[usize]>,
    passes: Vec<Box<dyn Pass>>,
) -> Result<(BuildReport, usize)> {
    in_parts_tiled(e, grid, order, passes, 200.0)
}

/// [`in_parts_with`], the layout summarized for `layout_tile` metre tiles.
fn in_parts_tiled(
    e: &EncodingOptions,
    grid: &PartitionGrid,
    order: Option<&[usize]>,
    passes: Vec<Box<dyn Pass>>,
    layout_tile: f32,
) -> Result<(BuildReport, usize)> {
    let mut w = map();
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
    convert_parts_summarized(
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
        &Pipeline { passes },
    )?;
    let r = report.take().unwrap();
    Ok((r, parts))
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
    let (_, n) = in_parts(&tmp("count"), &aligned(), None, Vec::new()).unwrap();
    assert!(n >= 5, "{n} parts");
    orders.push(Some((0..n).rev().collect()));
    orders.push(Some((0..n).map(|i| (i * 3 + 1) % n).collect()));
    for (k, order) in orders.iter().enumerate() {
        let out = tmp(&format!("parts{k}"));
        let (r, parts) = in_parts(&out, &aligned(), order.as_deref(), Vec::new()).unwrap();
        assert_eq!(r.parts, parts, "{order:?}");
        // collision waited for the ground's part, then was laid out as it came; none was left for the end
        assert!(r.held.peak_cells > 0, "{:?}", r.held);
        assert_eq!((r.held.at_finish, r.held.parts_untold), (0, 0), "{order:?}");
        same_maps(&reference, &out.join("parts"), &format!("{order:?}"));
    }
}

#[test]
fn a_part_left_without_its_roots_waits_to_the_end_and_changes_nothing() {
    let reference = tmp("whole-dropped");
    whole(&reference, vec![Box::new(DropFarRock)]);
    let out = tmp("parts-dropped");
    let (r, _) = in_parts(&out, &aligned(), None, vec![Box::new(DropFarRock)]).unwrap();
    // the far rock's part is not told: the cell it announced waits for it to the end
    assert_eq!(r.held.parts_untold, 1, "{:?}", r.held);
    assert!(r.held.at_finish >= 1, "{:?}", r.held);
    same_maps(&reference.join("parts"), &out.join("parts"), "dropped");
}

#[test]
fn a_misaligned_partition_and_unannounced_collision_are_refused() {
    // 300 m cells, not aligned to the 200 m tile: a tile's placements in two parts
    let e = in_parts(
        &tmp("misaligned"),
        &PartitionGrid::new(300.0).unwrap().with_halo(50.0).unwrap(),
        None,
        Vec::new(),
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
    // a soup a part's pass added, in a cell the layout summary announced for no part
    let e = in_parts(&tmp("unannounced"), &aligned(), None, vec![Box::new(AddSoup)])
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("does not announce"), "{e}");
}

/// The collision plan over parts: each part records the rule only, the writer plans each leaf as it lays the cell out by
/// the pass's own rule, and the plan it reports is the one the pass records for the whole map (leaf by leaf), the map
/// byte for byte the same; the rocks' few metal triangles are written as their leaf's wood or concrete.
#[test]
fn the_collision_plan_over_parts_is_the_whole_maps_leaf_by_leaf() {
    let reference = tmp("whole-planned");
    let (r, recorded) = whole_planned(&reference, vec![Box::new(plan())]);
    let recorded = recorded.unwrap();
    assert!(
        r.collision_plan.is_none(),
        "a whole map's plan is the pass's record"
    );
    let leaves = recorded.leaves.as_ref().unwrap();
    assert!(
        leaves.values().any(|l| !l.into.is_empty()),
        "the plan rewrites a minor surface somewhere: {leaves:?}"
    );
    let mut order: Option<Vec<usize>> = None;
    for _ in 0..2 {
        let out = tmp("parts-planned");
        let (r, n) = in_parts(&out, &aligned(), order.as_deref(), vec![Box::new(plan())]).unwrap();
        assert!(n >= 2);
        assert_eq!(r.collision_plan.as_ref(), Some(&recorded), "{order:?}");
        assert!(
            r.approximated
                .iter()
                .any(|(what, _)| what.starts_with("collision triangles of surface metal")),
            "{:?}",
            r.approximated
        );
        same_maps(
            &reference.join("parts"),
            &out.join("parts"),
            &format!("planned {order:?}"),
        );
        order = Some((0..n).rev().collect());
    }
}

/// The routes' preparation, for a map encoded with `cull_ids`: the overlays stacked, the blend chains split, the
/// collision planned, the map summarized, the chain objects budgeted from the cull-ID pool.
fn prepared(cull_ids: usize) -> Vec<Box<dyn Pass>> {
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
        Box::new(plan()),
        Box::new(SummarizeReskateMap::for_encoding(&e).unwrap()),
        Box::new(PlanReskateBlendBudget::pool()),
    ]
}

/// The whole map's recorded budget for `cull_ids`: how many tiles it ranks and gives the chain form.
fn budget_for(cull_ids: usize) -> SkateBlendBudget {
    let mut w = map();
    for p in prepared(cull_ids) {
        p.run(&mut w, &mut PassLog::default()).unwrap();
    }
    w.resource::<SkateBlendBudget>().unwrap().clone()
}

/// The overlays, blend chains, collision plan and pool budget over parts: each part stacks its own meshes (the deck,
/// drawn by several parts, stacked alike by each) and chains its own tiles, the writer decides the pool budget over the
/// whole map by the pass's rule on its own count: the decision, with its basis, is the whole map's, at the pool's
/// boundary (room for every tile's chain objects) and one ID short of it, and the maps are the same byte for byte.
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
    for cull_ids in [boundary, boundary - 1] {
        let reference = tmp(&format!("whole-pool-{cull_ids}"));
        let (r, w) = whole_with(&encoding_with(&reference, cull_ids), prepared(cull_ids));
        let recorded = w.resource::<SkateBlendBudget>().unwrap().clone();
        // the deck (drawn by several parts) and tiles were stacked
        let stack = w.resource::<SkateOverlayStack>().unwrap();
        assert!(
            stack.meshes.contains_key("proto deck"),
            "{:?}",
            stack.meshes.keys()
        );
        assert!(
            stack.meshes.keys().any(|m| m.starts_with("t_")),
            "{:?}",
            stack.meshes.keys()
        );
        assert!(
            r.blend_budget.is_none(),
            "a whole map's budget is the pass's record"
        );
        assert!(
            r.blend.tiles_split + r.blend.tiles_shared == BLENDS.len(),
            "{:?}",
            r.blend
        );
        let out = tmp(&format!("parts-pool-{cull_ids}"));
        let (p, n) = in_parts_with(
            &encoding_with(&out, cull_ids),
            &aligned(),
            None,
            prepared(cull_ids),
        )
        .unwrap();
        assert!(n >= 3);
        let made = p.blend_budget.unwrap();
        assert_eq!(made.mode, BlendBudgetMode::Pool);
        assert_eq!(made.decision, recorded.decision, "{cull_ids}");
        same_maps(
            &reference.join("parts"),
            &out.join("parts"),
            &format!("pool {cull_ids}"),
        );
    }
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
    let e = in_parts(&tmp("tampered"), &aligned(), None, passes)
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
    let e = in_parts_tiled(
        &encoding(&tmp("other-tiles")),
        &aligned(),
        None,
        Vec::new(),
        100.0,
    )
    .err()
    .unwrap()
    .to_string();
    assert!(e.contains("made for 100 m tiles, the writer's are 200 m"), "{e}");
}
