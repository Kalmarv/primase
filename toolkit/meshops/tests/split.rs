//! split-by-collision on a synthetic two-surface scene, run by the pass manager (declarations, conservation and the
//! verifier checked).
#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};

use primase_core::ledger::Conservation;
use primase_core::pass::{Options, Pass, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::pbr::PbrMaterial;
use primase_core::{
    Collision, Component, Entity, Instance, Mesh, MeshFile, Name, Primitive, Semantic, Stream, Surface,
    Surfaces, Topology, Transform, World, remap_list,
};
use primase_meshops::SplitByCollision;

fn run(w: &mut World, pass: SplitByCollision) -> Result<RunReport, String> {
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .with_conservation(Conservation::Enforce)
        .run(
            w,
            &Pipeline {
                passes: vec![Box::new(pass)],
            },
            &mut report,
        )
        .map_err(|e| e.to_string())?;
    Ok(report)
}

fn moved(by: [f32; 3]) -> Transform {
    let mut t = Transform::IDENTITY;
    t.cols[3] = by;
    t
}

/// A triangle soup of one surface.
fn soup(triangles: &[[[f32; 3]; 3]]) -> Collision {
    let mut c = Collision::default();
    for t in triangles {
        c.extend(t, &[[0, 1, 2]], 0, |p| p);
    }
    c
}

/// One indexed triangle primitive of these triangles, with texture coordinates, drawn with `material`.
fn primitive(triangles: &[[[f32; 3]; 3]], material: Entity) -> Primitive {
    let positions: Vec<[f32; 3]> = triangles.iter().flatten().copied().collect();
    let uv = positions.iter().map(|p| [p[0], p[2]]).collect();
    Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (Semantic::Position, Stream::F32x3(positions.clone())),
            (Semantic::TexCoord(0), Stream::F32x2(uv)),
        ]),
        indices: Some((0..positions.len() as u32).collect()),
        material: Some(material),
    }
}

const FLOOR: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
const WALL: [[f32; 3]; 3] = [[5.0, 0.0, 0.0], [5.0, 1.0, 0.0], [5.0, 0.0, 1.0]];
const FAR: [[f32; 3]; 3] = [[0.0, 10.0, 0.0], [1.0, 10.0, 0.0], [0.0, 10.0, 1.0]];

struct Scene {
    w: World,
    placement: Entity,
    prototype: Entity,
    material: Entity,
}

/// One baked placement moved by (100, 0, 0), drawing a floor triangle, a wall triangle and one far from both in one
/// primitive; the floor's collision placed through a prototype (`Floor_<`), the wall's on an entity of its own
/// (`Wall_<`), both where the placement draws them.
fn scene() -> Scene {
    let mut w = World::new();
    w.insert_resource(Surfaces(vec![Surface::new("Default")]));
    let material = w.spawn();
    w.insert(material, PbrMaterial::default());
    let prototype = w.spawn();
    w.insert(prototype, Name("bake_0_object".into()));
    w.insert(
        prototype,
        Mesh::new(vec![primitive(&[FLOOR, WALL, FAR], material)]),
    );
    w.insert(
        prototype,
        MeshFile {
            path: "level.gltf".into(),
            mesh: 0,
            materials: vec![Some(material)],
        },
    );
    let placement = w.spawn();
    w.insert(
        placement,
        Instance {
            prototype,
            variant: 0,
        },
    );
    w.insert(placement, moved([100.0, 0.0, 0.0]));
    w.insert(placement, Name("bake_0_object".into()));
    // the floor's collision, larger than the drawn triangle, placed through a prototype
    let floor = w.spawn();
    w.insert(
        floor,
        soup(&[[[-1.0, 0.0, -1.0], [3.0, 0.0, -1.0], [-1.0, 0.0, 3.0]]]),
    );
    let floor_at = w.spawn();
    w.insert(
        floor_at,
        Instance {
            prototype: floor,
            variant: 0,
        },
    );
    w.insert(floor_at, moved([100.0, 0.0, 0.0]));
    w.insert(floor_at, Name("Floor_<".into()));
    // the wall's, on an entity of its own, placed in the world frame
    let wall = w.spawn();
    w.insert(
        wall,
        soup(&[[[105.0, -1.0, -1.0], [105.0, 3.0, -1.0], [105.0, -1.0, 3.0]]]),
    );
    w.insert(wall, Transform::IDENTITY);
    w.insert(wall, Name("Wall_<".into()));
    Scene {
        w,
        placement,
        prototype,
        material,
    }
}

fn pass(groups: &[(&str, &str)], tolerance: f64) -> SplitByCollision {
    SplitByCollision::new(
        BTreeSet::from(["bake_0".to_owned()]),
        groups
            .iter()
            .map(|(p, g)| ((*p).to_owned(), (*g).to_owned()))
            .collect(),
        tolerance,
        "room".into(),
    )
    .unwrap()
}

/// The placements named `name` and the triangles their prototype draws, as placed corners.
fn drawn(w: &World, name: &str) -> Vec<(Entity, Vec<[[f32; 3]; 3]>)> {
    w.iter::<Instance>()
        .filter(|(e, _)| w.get::<Name>(*e).is_some_and(|n| n.0 == name))
        .map(|(e, i)| {
            let t = w.get::<Transform>(e).unwrap().cols[3];
            let mut tris = Vec::new();
            for p in w.get::<Mesh>(i.prototype).unwrap().primitives() {
                let pos = p.positions().unwrap();
                for c in p.indices.as_ref().unwrap().as_chunks::<3>().0 {
                    tris.push(c.map(|k| {
                        let v = pos[k as usize];
                        [v[0] + t[0], v[1] + t[1], v[2] + t[2]]
                    }));
                }
            }
            (e, tris)
        })
        .collect()
}

fn at(t: [[f32; 3]; 3], by: f32) -> [[f32; 3]; 3] {
    t.map(|v| [v[0] + by, v[1], v[2]])
}

/// The floor triangle goes to the floor's group; the wall triangle to the wall's; the far one to the rest, which is
/// the wall's group here, so it shares the wall's placement. The placement itself becomes the first group's, a copy
/// the other's; each places a prototype of its own, named alike, made from the source, materials and coordinates
/// unchanged, without the file link; the source, placed no longer, goes.
#[test]
fn each_triangle_goes_to_the_group_of_the_collision_it_lies_on() {
    let Scene {
        mut w,
        placement,
        prototype,
        material,
    } = scene();
    let report = run(&mut w, pass(&[("Floor_", "park"), ("Wall_", "room")], 0.01)).unwrap();
    let log = &report.steps[0].log;
    let park = drawn(&w, "bake_0_object~park");
    let room = drawn(&w, "bake_0_object~room");
    assert_eq!(park.len(), 1);
    assert_eq!(room.len(), 1);
    assert_eq!(
        park[0].0, placement,
        "the placement itself is the first group's (park before room)"
    );
    assert_eq!(park[0].1, [at(FLOOR, 100.0)]);
    assert_eq!(room[0].1, [at(WALL, 100.0), at(FAR, 100.0)]);
    for (e, _) in park.iter().chain(&room) {
        let i = w.get::<Instance>(*e).unwrap();
        let p = i.prototype;
        assert_eq!(w.get::<Name>(p), w.get::<Name>(*e));
        assert_eq!(log.placed_from.get(&p), Some(&prototype));
        assert!(!w.has::<MeshFile>(p), "the mesh is no longer its file's");
        let prim = &w.get::<Mesh>(p).unwrap().primitives()[0];
        assert_eq!(prim.material, Some(material));
        // compacted: each piece keeps only its own vertices, every stream with them
        assert_eq!(prim.vertex_count(), prim.indices.as_ref().unwrap().len());
        assert_eq!(prim.streams.len(), 2);
    }
    assert_eq!(log.placed_from.get(&room[0].0), Some(&placement));
    assert!(
        !w.is_alive(prototype),
        "no placement places the source any longer"
    );
    assert_eq!(log.counts["triangles in group park"], 1);
    assert_eq!(log.counts["triangles in group room"], 2);
    assert_eq!(
        log.counts["triangles near no group's collision, put in the rest group"],
        1
    );
    assert_eq!(log.counts["placements split"], 1);
    assert_eq!(
        log.counts["prototypes made, one per group of a split placement"],
        2
    );
    assert_eq!(log.counts["collision placements in no group"], 0);
}

/// A triangle within the tolerance of two groups' collision goes to the one whose farthest corner is nearest; the
/// longest prefix chooses a collision placement's group; a collision placement no prefix matches is counted.
#[test]
fn the_nearest_group_wins_and_the_longest_prefix_names_it() {
    let Scene { mut w, .. } = scene();
    // a second floor 4 mm below the first, under a longer prefix of the same start
    let under = w.spawn();
    w.insert(
        under,
        soup(&[[[99.0, -0.004, -1.0], [103.0, -0.004, -1.0], [99.0, -0.004, 3.0]]]),
    );
    w.insert(under, Transform::IDENTITY);
    w.insert(under, Name("Floor_<_under".into()));
    let stray = w.spawn();
    w.insert(stray, soup(&[FAR]));
    w.insert(stray, Transform::IDENTITY);
    w.insert(stray, Name("Lamp_<".into()));
    let report = run(
        &mut w,
        pass(
            &[("Floor_", "park"), ("Floor_<_under", "cellar"), ("Wall_", "room")],
            0.01,
        ),
    )
    .unwrap();
    let log = &report.steps[0].log;
    // the floor triangle lies on the park floor, 4 mm above the cellar's
    assert_eq!(drawn(&w, "bake_0_object~park")[0].1, [at(FLOOR, 100.0)]);
    assert!(drawn(&w, "bake_0_object~cellar").is_empty());
    assert_eq!(log.counts["collision placements in no group"], 1);
}

/// The tolerance is the reach: a floor 4 mm below the drawn one is out of a 3 mm reach (the triangle goes to the
/// rest) and within a 5 mm one.
#[test]
fn the_tolerance_is_the_reach() {
    let Scene { mut w, .. } = scene();
    let under = w.spawn();
    w.insert(
        under,
        soup(&[[[99.0, -0.004, -1.0], [103.0, -0.004, -1.0], [99.0, -0.004, 3.0]]]),
    );
    w.insert(under, Transform::IDENTITY);
    w.insert(under, Name("Cellar_<".into()));
    let mut short = w.clone();
    run(
        &mut short,
        pass(&[("Cellar_", "cellar"), ("Wall_", "room")], 0.003),
    )
    .unwrap();
    assert_eq!(
        drawn(&short, "bake_0_object~room")[0].1,
        [at(FLOOR, 100.0), at(WALL, 100.0), at(FAR, 100.0)]
    );
    let report = run(&mut w, pass(&[("Cellar_", "cellar"), ("Wall_", "room")], 0.005)).unwrap();
    assert_eq!(drawn(&w, "bake_0_object~cellar")[0].1, [at(FLOOR, 100.0)]);
    // the floor of the scene, named by no prefix here
    assert_eq!(report.steps[0].log.counts["collision placements in no group"], 1);
}

/// A prefix that matches nothing is refused before anything changes; a placement whose prototype collides too is
/// refused.
#[test]
fn a_selection_that_selects_nothing_is_refused() {
    let Scene { mut w, prototype, .. } = scene();
    let e = run(&mut w, pass(&[("Ceiling_", "room")], 0.01)).unwrap_err();
    assert!(
        e.contains("no collision placement's name starts with Ceiling_"),
        "{e}"
    );
    let e = run(
        &mut w,
        SplitByCollision::new(
            BTreeSet::from(["nothing".to_owned()]),
            BTreeMap::from([("Wall_".to_owned(), "room".to_owned())]),
            0.01,
            "room".into(),
        )
        .unwrap(),
    )
    .unwrap_err();
    assert!(e.contains("no placement's name starts with nothing"), "{e}");
    assert!(w.is_alive(prototype));
    w.insert(prototype, soup(&[FLOOR]));
    let e = run(&mut w, pass(&[("Wall_", "room")], 0.01)).unwrap_err();
    assert!(e.contains("a collision of its own"), "{e}");
}

/// Every option is required, checked, and printed so that it makes the pass again.
#[test]
fn the_options_are_required_and_round_trip() {
    let p = pass(&[("Floor_", "park"), ("Wall_<=x", "room")], 0.05);
    assert_eq!(SplitByCollision::from_options(&p.options()).unwrap(), p);
    assert_eq!(p.groups()["Wall_<=x"], "room", "the group follows the last '='");
    let full = p.options();
    for k in ["placements", "groups", "tolerance", "rest"] {
        let mut o: Options = full.clone();
        o.remove(k);
        assert!(SplitByCollision::from_options(&o).is_err(), "{k}");
    }
    let mut o = full.clone();
    o.insert("tolerance".into(), "0".into());
    assert!(SplitByCollision::from_options(&o).is_err());
    let mut o = full;
    o.insert("rest".into(), "a=b".into());
    assert!(SplitByCollision::from_options(&o).is_err());
}

/// A dialect's value per vertex of each primitive, following both remaps (as a reader's per-vertex attributes do).
#[derive(Clone, Debug, PartialEq)]
struct Attributes(Vec<Vec<u32>>);
impl Component for Attributes {
    const NAME: &'static str = "test.attributes";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::PerVertex;
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(remap_list(&self.0, from).map(Attributes))
    }
    fn remap_vertices(&self, primitive: usize, from: &[u32]) -> Option<Result<Self, String>> {
        let mut out = self.0.clone();
        let list = self.0.get(primitive)?;
        out[primitive] = from.iter().map(|&i| list[i as usize]).collect();
        Some(Ok(Attributes(out)))
    }
}

/// Per-primitive and per-vertex data follow the pieces: the prototype draws the far triangle as primitive 0 and the
/// floor and wall triangles as primitive 1; the floor's copy keeps primitive 1 only, its first three vertices; the
/// wall's (the rest group's too) keeps primitive 0 whole and primitive 1's last three vertices.
#[test]
fn per_primitive_and_per_vertex_data_follow_the_pieces() {
    let Scene {
        mut w,
        prototype,
        material,
        ..
    } = scene();
    w.insert(
        prototype,
        Mesh::new(vec![
            primitive(&[FAR], material),
            primitive(&[FLOOR, WALL], material),
        ]),
    );
    w.insert(
        prototype,
        MeshFile {
            path: "level.gltf".into(),
            mesh: 0,
            materials: vec![Some(material); 2],
        },
    );
    w.insert(
        prototype,
        Attributes(vec![vec![0, 1, 2], vec![10, 11, 12, 13, 14, 15]]),
    );
    run(&mut w, pass(&[("Floor_", "park"), ("Wall_", "room")], 0.01)).unwrap();
    let attributes = |name: &str| {
        let (e, _) = drawn(&w, name)[0];
        let p = w.get::<Instance>(e).unwrap().prototype;
        w.get::<Attributes>(p).unwrap().clone()
    };
    assert_eq!(
        attributes("bake_0_object~park"),
        Attributes(vec![vec![10, 11, 12]])
    );
    assert_eq!(
        attributes("bake_0_object~room"),
        Attributes(vec![vec![0, 1, 2], vec![13, 14, 15]])
    );
}

/// A component that can't follow (opaque: whether it describes primitives or vertices can't be known) fails the pass,
/// naming it.
#[test]
fn a_component_that_cannot_follow_is_refused_by_name() {
    use primase_core::world::{Opaque, OpaqueData};
    let Scene { mut w, prototype, .. } = scene();
    w.insert_opaque(
        prototype,
        "game.parts",
        Opaque {
            version: 1,
            data: OpaqueData::Bytes(vec![1]),
        },
    )
    .unwrap();
    let e = run(&mut w, pass(&[("Floor_", "park"), ("Wall_", "room")], 0.01)).unwrap_err();
    assert!(e.contains("game.parts"), "{e}");
}

/// A placement's own collision collides once: it stays on the first group's placement, a copy has none, counted.
#[test]
fn a_placements_own_collision_stays_on_one_copy() {
    let Scene { mut w, placement, .. } = scene();
    // the placement's own soup, far from both surfaces (its name matches no group: counted as no group's)
    w.insert(placement, soup(&[at(FAR, 100.0)]));
    let report = run(&mut w, pass(&[("Floor_", "park"), ("Wall_", "room")], 0.01)).unwrap();
    let park = drawn(&w, "bake_0_object~park");
    let room = drawn(&w, "bake_0_object~room");
    assert_eq!(park[0].0, placement);
    assert!(w.has::<Collision>(placement));
    assert!(!w.has::<Collision>(room[0].0), "a copy collides no second time");
    let log = &report.steps[0].log;
    assert_eq!(
        log.counts["placements' own collision kept on the first group's placement only (left off a copy)"],
        1
    );
    assert_eq!(log.counts["collision placements in no group"], 1);
}

#[test]
fn an_asset_that_listed_the_split_prototype_stops_listing_it() {
    use primase_core::{Asset, Role};
    let Scene {
        mut w,
        prototype,
        placement,
        ..
    } = scene();
    let asset = w.spawn();
    w.insert(
        asset,
        Asset {
            members: vec![(prototype, Role::Part), (placement, Role::Part)],
        },
    );
    run(&mut w, pass(&[("Floor_", "park"), ("Wall_", "room")], 0.01)).unwrap();
    assert!(!w.is_alive(prototype));
    let members = &w.get::<Asset>(asset).unwrap().members;
    assert!(members.iter().all(|(m, _)| w.is_alive(*m)), "{members:?}");
}
