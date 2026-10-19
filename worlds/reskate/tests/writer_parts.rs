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
    Collision, ColorSpace, Entity, Instance, Light, LightKind, Mesh, Name, Primitive, Semantic, Stream,
    Surfaces, TexFormat, TexRole, Texture, Topology, Transform, World,
};
use primase_meshops::PartitionGrid;
use primase_reskate::components::{MarkerKind, Other, SkateMarker, SkateWater};
use primase_reskate::passes::{
    RaiseOptions, SummarizeReskateContent, SummarizeReskateInstances, SummarizeReskateLayout,
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

/// A prototype of a grid mesh with its collision, drawn with `material`.
fn prototype(w: &mut World, name: &str, n: u32, size: f32, y: f32, material: Entity, surface: u16) -> Entity {
    let (pos, tris) = grid(n, size, y);
    let e = w.spawn();
    w.insert(e, Name(name.into()));
    w.insert(
        e,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(pos.clone()))]),
            indices: Some(tris.iter().flatten().copied().collect()),
            material: Some(material),
        }]),
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

/// A raised map: decks and rocks sharing one textured material, flat ground over east -100..1500, north -100..500 (a
/// soup of its own, no transform: a part of its own in a partition), four lights, a spawn, two unnamed travel points and
/// an unnamed trainer spot, one water surface.
fn map() -> World {
    let mut w = World::new();
    let mut surfaces = Surfaces::default();
    let concrete = surfaces.index("concrete").unwrap();
    let wood = surfaces.index("wood").unwrap();
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
    let deck = prototype(&mut w, "deck", 10, 40.0, 2.0, material, wood);
    let rock = prototype(&mut w, "rock", 1, 3.0, 1.0, material, concrete);
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
    EncodingOptions {
        out_root: out.to_path_buf(),
        name: Some("parts".into()),
        ..EncodingOptions::default()
    }
}

/// The map written whole, after `passes`.
fn whole(out: &Path, passes: Vec<Box<dyn Pass>>) -> BuildReport {
    let mut w = map();
    for p in passes {
        p.run(&mut w, &mut PassLog::default()).unwrap();
    }
    primase_reskate::write_map(&w, Path::new(""), &encoding(out)).unwrap()
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
        &["core", "skate", "pbr"]
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
        Box::new(SummarizeReskateLayout::new(200.0).unwrap()),
        Box::new(SummarizeReskateInstances),
        Box::new(SummarizeReskateContent),
    ];
    convert_parts_summarized(
        &mut Planned(w),
        &mut PlannedParts::default(),
        Box::new(Sink(
            Some(MapSink::new(Path::new(""), &encoding(out))),
            report.clone(),
        )),
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
