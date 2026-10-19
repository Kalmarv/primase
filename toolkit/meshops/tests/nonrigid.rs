//! `bake-nonrigid` against hand-worked values: one triangle facing +Y (tangents of both handedness signs) under a
//! mirror, a shear and both, each expected position, normal, tangent and index worked out by hand below.
#![allow(clippy::unwrap_used)]
use std::collections::BTreeMap;

use primase_core::pass::{Mode, PassManager, PassOptions, PassRegistry, Pipeline, RunReport, Target};
use primase_core::{
    Collision, Instance, Light, LightKind, LodForm, Lods, Mesh, MeshFile, Name, Opaque, OpaqueData,
    Primitive, Semantic, Stream, Surface, Surfaces, Topology, Transform, TransformKind, World,
};
use primase_meshops::{BakeNonrigid, bake_nonrigid};

/// 1 / sqrt 2 as f32.
const H: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// (0, y, 0), (0, y, 1), (1, y, 0): counter-clockwise seen from +Y; normals +Y; tangents +X, w 1, 1, -1.
fn tri(y: f32) -> Mesh {
    let mut streams = BTreeMap::new();
    streams.insert(
        Semantic::Position,
        Stream::F32x3(vec![[0.0, y, 0.0], [0.0, y, 1.0], [1.0, y, 0.0]]),
    );
    streams.insert(Semantic::Normal, Stream::F32x3(vec![[0.0, 1.0, 0.0]; 3]));
    streams.insert(
        Semantic::Tangent,
        Stream::F32x4(vec![
            [1.0, 0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0, -1.0],
        ]),
    );
    Mesh::new(vec![Primitive {
        topology: Topology::Triangles,
        streams,
        indices: Some(vec![0, 1, 2]),
        material: None,
    }])
}

fn t(cols: [[f32; 3]; 4]) -> Transform {
    Transform { cols }
}

/// A primitive's positions, normals, tangents and indices.
type Streams = (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 4]>, Vec<u32>);

fn streams(m: &Mesh) -> Streams {
    let p = &m.primitives()[0];
    let (Some(Stream::F32x3(pos)), Some(Stream::F32x3(nor)), Some(Stream::F32x4(tan))) = (
        p.streams.get(&Semantic::Position),
        p.streams.get(&Semantic::Normal),
        p.streams.get(&Semantic::Tangent),
    ) else {
        panic!("streams")
    };
    (pos.clone(), nor.clone(), tan.clone(), p.indices.clone().unwrap())
}

fn close3(a: &[[f32; 3]], b: &[[f32; 3]]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| (0..3).all(|k| (x[k] - y[k]).abs() <= 1e-7))
}

fn close4(a: &[[f32; 4]], b: &[[f32; 4]]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| (0..4).all(|k| (x[k] - y[k]).abs() <= 1e-7))
}

fn proto(w: &mut World, name: &str) -> primase_core::Entity {
    let p = w.spawn();
    w.insert(p, tri(0.0));
    w.insert(p, Name(name.into()));
    p
}

fn place(w: &mut World, prototype: primase_core::Entity, tr: Transform) -> primase_core::Entity {
    let e = w.spawn();
    w.insert(e, tr);
    w.insert(
        e,
        Instance {
            prototype,
            variant: 0,
        },
    );
    e
}

/// Mirrored placements: `M = diag(-2, 1, 1)` at (10, 0, 5), twice. The pass keeps `M F = diag(2, 1, 1)` and draws the
/// twin `F tri`: x negated, normals still +Y, tangents (-1, 0, 0) with w negated, the triangle reversed so it still
/// faces +Y. Check by hand: corner (1, 0, 0) lands at M (1, 0, 0) + t = (8, 0, 5) before, and at
/// diag(2, 1, 1) (-1, 0, 0) + t = (8, 0, 5) after.
#[test]
fn a_mirror_moves_into_a_shared_twin() {
    let mut w = World::new();
    let p = proto(&mut w, "roof");
    let form = w.spawn();
    w.insert(form, tri(0.25));
    w.insert(
        p,
        Lods {
            detail: 0,
            forms: vec![LodForm { detail: 1, form }],
            default: None,
        },
    );
    let m = t([
        [-2.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [10.0, 0.0, 5.0],
    ]);
    let (a, b) = (place(&mut w, p, m), place(&mut w, p, m));
    let log = bake_nonrigid(&mut w, &BakeNonrigid::default());
    let kept = t([
        [2.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [10.0, 0.0, 5.0],
    ]);
    for e in [a, b] {
        assert_eq!(w.get::<Transform>(e), Some(&kept));
        assert_eq!(w.get::<Transform>(e).unwrap().classify(), TransformKind::Regular);
    }
    let twin = w.get::<Instance>(a).unwrap().prototype;
    assert_eq!(w.get::<Instance>(b).unwrap().prototype, twin, "one twin for both");
    assert_eq!(w.get::<Name>(twin), Some(&Name("roof.mirrored".into())));
    let (pos, nor, tan, idx) = streams(w.get::<Mesh>(twin).unwrap());
    assert!(
        close3(&pos, &[[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0]]),
        "{pos:?}"
    );
    assert!(close3(&nor, &[[0.0, 1.0, 0.0]; 3]), "{nor:?}");
    assert!(
        close4(
            &tan,
            &[
                [-1.0, 0.0, 0.0, -1.0],
                [-1.0, 0.0, 0.0, -1.0],
                [-1.0, 0.0, 0.0, 1.0]
            ]
        ),
        "{tan:?}"
    );
    assert_eq!(idx, [0, 2, 1]);
    // the twin's form is the form's twin; the prototype and its form are untouched
    let twin_form = w.get::<Lods>(twin).unwrap().forms[0].form;
    assert_ne!(twin_form, form);
    let (fpos, _, _, fidx) = streams(w.get::<Mesh>(twin_form).unwrap());
    assert!(
        close3(&fpos, &[[0.0, 0.25, 0.0], [0.0, 0.25, 1.0], [-1.0, 0.25, 0.0]]),
        "{fpos:?}"
    );
    assert_eq!(fidx, [0, 2, 1]);
    assert_eq!(w.get::<Mesh>(p), Some(&tri(0.0)));
    assert_eq!(w.get::<Mesh>(form), Some(&tri(0.25)));
    assert_eq!(log.counts.get("mirror twins of prototypes made"), Some(&1));
    assert_eq!(log.counts.get("mirrored transforms made rigid"), Some(&2));
}

/// Sheared placements. `L` with columns x -> (1, 1, 0), y, z: positions `L p` (corner (1, 0, 0) -> (1, 1, 0)); the
/// surface now spans (1, 1, 0) and (0, 0, 1), so its unit normal is (-1, 1, 0) / sqrt 2 (`L^-T` of +Y); tangents
/// (1, 1, 0) / sqrt 2, w kept; order kept (det 1). With columns x -> (-1, 1, 0) (det -1): corner (1, 0, 0) ->
/// (-1, 1, 0), the surface spans (-1, 1, 0) and (0, 0, 1), unit normal (1, 1, 0) / sqrt 2 (the side `L` maps +Y to,
/// kept by the determinant's sign), tangents (-1, 1, 0) / sqrt 2 with w negated, the triangle reversed. Each placement
/// keeps only its translation.
#[test]
fn a_shear_moves_into_a_copy_per_placement() {
    let mut w = World::new();
    let p = proto(&mut w, "roof");
    let s = place(
        &mut w,
        p,
        t([
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -7.0],
        ]),
    );
    let sm = place(
        &mut w,
        p,
        t([
            [-1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [4.0, 0.0, 0.0],
        ]),
    );
    let log = bake_nonrigid(&mut w, &BakeNonrigid::default());
    let mut only = Transform::IDENTITY;
    only.cols[3] = [0.0, 0.0, -7.0];
    assert_eq!(w.get::<Transform>(s), Some(&only));
    only.cols[3] = [4.0, 0.0, 0.0];
    assert_eq!(w.get::<Transform>(sm), Some(&only));
    let copy = w.get::<Instance>(s).unwrap().prototype;
    assert_eq!(
        w.get::<Name>(copy),
        Some(&Name(format!("roof.baked.e{}", s.index())))
    );
    let (pos, nor, tan, idx) = streams(w.get::<Mesh>(copy).unwrap());
    assert!(
        close3(&pos, &[[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 1.0, 0.0]]),
        "{pos:?}"
    );
    assert!(close3(&nor, &[[-H, H, 0.0]; 3]), "{nor:?}");
    assert!(
        close4(&tan, &[[H, H, 0.0, 1.0], [H, H, 0.0, 1.0], [H, H, 0.0, -1.0]]),
        "{tan:?}"
    );
    assert_eq!(idx, [0, 1, 2]);
    let copy = w.get::<Instance>(sm).unwrap().prototype;
    let (pos, nor, tan, idx) = streams(w.get::<Mesh>(copy).unwrap());
    assert!(
        close3(&pos, &[[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [-1.0, 1.0, 0.0]]),
        "{pos:?}"
    );
    assert!(close3(&nor, &[[H, H, 0.0]; 3]), "{nor:?}");
    assert!(
        close4(&tan, &[[-H, H, 0.0, -1.0], [-H, H, 0.0, -1.0], [-H, H, 0.0, 1.0]]),
        "{tan:?}"
    );
    assert_eq!(idx, [0, 2, 1]);
    assert_eq!(
        log.counts.get("prototype copies made for sheared placements"),
        Some(&2)
    );
    assert_eq!(log.counts.get("sheared transforms made rigid"), Some(&2));
}

/// An entity drawing its own geometry is mapped in place; a collision soup's triangle turns with its positions; a
/// mirrored light keeps its position and loses only the mirror; with `mirrors=keep` a mirrored placement is left.
#[test]
fn own_geometry_soups_lights_and_kept_mirrors() {
    let mut w = World::new();
    let own = w.spawn();
    w.insert(own, tri(0.0));
    w.insert(
        own,
        t([
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 2.0, 0.0],
        ]),
    );
    w.insert_resource(Surfaces(vec![Surface {
        name: "stone".into(),
        class: None,
    }]));
    let soup = w.spawn();
    w.insert(
        soup,
        Collision {
            positions: vec![[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]],
            triangles: vec![[0, 1, 2]],
            surfaces: vec![0],
        },
    );
    w.insert(
        soup,
        t([[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]]),
    );
    let light = w.spawn();
    w.insert(
        light,
        Light {
            kind: LightKind::Point,
            color: [1.0; 3],
            intensity: 1.0,
            range: None,
            cast_shadows: None,
        },
    );
    w.insert(
        light,
        t([
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 2.0, 3.0],
        ]),
    );
    let p = proto(&mut w, "roof");
    let kept = place(
        &mut w,
        p,
        t([[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]]),
    );
    let snapshot = w.clone();
    // mirrors kept: nothing here is sheared, so nothing changes
    let log = bake_nonrigid(
        &mut w,
        &BakeNonrigid {
            mirrors: primase_meshops::Mirrors::Keep,
        },
    );
    assert!(log.counts.is_empty(), "{:?}", log.counts);
    assert_eq!(w.get::<Transform>(kept), snapshot.get::<Transform>(kept));
    // mirrors baked
    bake_nonrigid(&mut w, &BakeNonrigid::default());
    let mut at = Transform::IDENTITY;
    at.cols[3] = [0.0, 2.0, 0.0];
    assert_eq!(w.get::<Transform>(own), Some(&at));
    let (pos, _, _, idx) = streams(w.get::<Mesh>(own).unwrap());
    assert!(
        close3(&pos, &[[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0]]),
        "{pos:?}"
    );
    assert_eq!(idx, [0, 2, 1]);
    let c = w.get::<Collision>(soup).unwrap();
    assert!(close3(
        &c.positions,
        &[[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0]]
    ));
    assert_eq!(c.triangles, [[0, 2, 1]]);
    let mut lt = Transform::IDENTITY;
    lt.cols[3] = [1.0, 2.0, 3.0];
    assert_eq!(w.get::<Transform>(light), Some(&lt));
    assert_eq!(
        w.get::<Transform>(kept).unwrap().classify(),
        TransformKind::Regular
    );
}

#[test]
fn a_target_taking_rigid_transforms_is_planned_with_the_pass() {
    let mut reg = PassRegistry::new();
    primase_meshops::register(&mut reg);
    let manager = PassManager::new(reg);
    let target = Target::new("rigid").dynamic_typed::<Transform>(|t| {
        matches!(t.classify(), TransformKind::Regular | TransformKind::Degenerate)
    });
    let mut w = World::new();
    let p = proto(&mut w, "roof");
    let e = place(&mut w, p, Transform::IDENTITY);
    let plan = |w: &World| {
        manager
            .plan(
                w,
                &target,
                Mode::Partial,
                &PassOptions::new(),
                &mut RunReport::default(),
            )
            .unwrap()
            .to_string()
    };
    assert_eq!(plan(&w), "", "rigid already: nothing to do");
    w.insert(
        e,
        t([[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]]),
    );
    assert_eq!(plan(&w), "bake-nonrigid");
}

/// A component a mapping drops is counted and recorded, so a name whose only holder was mapped may vanish under the
/// pass manager: here `core.mesh_file` on the one entity drawing its own geometry, mirrored.
#[test]
fn a_dropped_name_with_its_only_holder_mapped_is_a_recorded_removal() {
    let mut w = World::new();
    let e = w.spawn();
    w.insert(e, tri(0.0));
    w.insert(
        e,
        MeshFile {
            path: "a.glb".into(),
            mesh: 0,
            materials: vec![None],
        },
    );
    w.insert(
        e,
        t([[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]]),
    );
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .run(
            &mut w,
            &Pipeline {
                passes: vec![Box::new(BakeNonrigid::default())],
            },
            &mut report,
        )
        .expect("a recorded removal is accepted");
    assert!(!w.has_component_named("core.mesh_file"));
    let log = &report.steps[0].log;
    assert_eq!(
        log.counts
            .get("components dropped from mapped geometry: core.mesh_file"),
        Some(&1)
    );
    assert!(log.removed.contains("core.mesh_file"));
}

#[test]
fn a_prototype_that_cannot_follow_keeps_its_placements_as_they_were() {
    let mut w = World::new();
    let p = proto(&mut w, "roof");
    w.insert_opaque(
        p,
        "game.local_points",
        Opaque {
            version: 1,
            data: OpaqueData::Bytes(vec![0]),
        },
    );
    let mirrored = t([[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]]);
    let e = place(&mut w, p, mirrored);
    let entities = w.entity_count();
    let log = bake_nonrigid(&mut w, &BakeNonrigid::default());
    assert_eq!(w.get::<Transform>(e), Some(&mirrored));
    assert_eq!(w.get::<Instance>(e).unwrap().prototype, p);
    assert_eq!(w.entity_count(), entities, "no copy left behind");
    assert_eq!(
        log.counts.get("mirrored transforms left: game.local_points"),
        Some(&1),
        "{:?}",
        log.counts
    );
}
