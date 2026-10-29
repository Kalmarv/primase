//! `World::duplicate` and `World::apply_linear`: copying an entity, and mapping its local space so a mirror or shear
//! leaves its transform for its geometry.
#![allow(clippy::unwrap_used)]
use std::collections::BTreeMap;

use primase_core::{
    Instance, Light, LightKind, Linear, Mesh, MeshFile, Name, Opaque, OpaqueData, Primitive, Semantic,
    Stream, Topology, Transform, World,
};

/// One triangle facing +Y (counter-clockwise seen from above), its normals +Y, its tangents +X with w = 1.
fn triangle() -> Mesh {
    let mut streams = BTreeMap::new();
    streams.insert(
        Semantic::Position,
        Stream::F32x3(vec![[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]]),
    );
    streams.insert(Semantic::Normal, Stream::F32x3(vec![[0.0, 1.0, 0.0]; 3]));
    streams.insert(Semantic::Tangent(0), Stream::F32x4(vec![[1.0, 0.0, 0.0, 1.0]; 3]));
    streams.insert(
        Semantic::Tangent(1),
        Stream::F32x4(vec![[0.0, 0.0, 1.0, -1.0]; 3]),
    );
    Mesh::new(vec![Primitive {
        topology: Topology::Triangles,
        streams,
        indices: Some(vec![0, 1, 2]),
        material: None,
    }])
}

#[test]
fn indexed_tangents_use_the_same_json_shape_as_indexed_coordinates() {
    assert_eq!(
        serde_json::to_value(Semantic::Tangent(0)).unwrap(),
        serde_json::json!({"Tangent": 0})
    );
    assert_eq!(
        serde_json::to_value(Semantic::Tangent(1)).unwrap(),
        serde_json::json!({"Tangent": 1})
    );
    assert_eq!(<Mesh as primase_core::Component>::VERSION, 2);
}

fn f3(m: &Mesh, s: Semantic) -> Vec<[f32; 3]> {
    match m.primitives()[0].streams.get(&s) {
        Some(Stream::F32x3(v)) => v.clone(),
        _ => panic!("{s:?}"),
    }
}

fn f4(m: &Mesh, s: Semantic) -> Vec<[f32; 4]> {
    match m.primitives()[0].streams.get(&s) {
        Some(Stream::F32x4(v)) => v.clone(),
        _ => panic!("{s:?}"),
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The front side of each triangle (counter-clockwise) against its first corner's normal, and its bitangent
/// `cross(N, T) * w` at that corner.
fn fronts_and_bitangent(m: &Mesh) -> (Vec<f32>, [f32; 3]) {
    let (p, n, t) = (
        f3(m, Semantic::Position),
        f3(m, Semantic::Normal),
        f4(m, Semantic::Tangent(0)),
    );
    let idx = m.primitives()[0].indices.clone().unwrap();
    let fronts = idx
        .as_chunks::<3>()
        .0
        .iter()
        .map(|&[a, b, c]| {
            let (a, b, c) = (p[a as usize], p[b as usize], p[c as usize]);
            let f = cross(
                [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
                [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
            );
            dot(f, n[idx[0] as usize])
        })
        .collect();
    let b = cross(n[0], [t[0][0], t[0][1], t[0][2]]).map(|x| x * t[0][3]);
    (fronts, b)
}

#[test]
fn duplicate_copies_every_component_shallowly() {
    let mut w = World::new();
    let proto = w.spawn();
    w.insert(proto, triangle());
    w.insert(proto, Name("p".into()));
    w.insert_opaque(
        proto,
        "game.thing",
        Opaque {
            version: 1,
            data: OpaqueData::Json(serde_json::json!({"a": 1})),
        },
    )
    .unwrap();
    let placement = w.spawn();
    w.insert(
        placement,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    let copy = w.duplicate(proto).expect("alive");
    assert_ne!(copy, proto);
    assert_eq!(w.get::<Mesh>(copy), w.get::<Mesh>(proto));
    assert_eq!(w.get::<Name>(copy), Some(&Name("p".into())));
    assert_eq!(
        w.get_opaque(copy, "game.thing"),
        w.get_opaque(proto, "game.thing")
    );
    // shallow: a copied reference names the same entity
    let pc = w.duplicate(placement).expect("alive");
    assert_eq!(w.get::<Instance>(pc).map(|i| i.prototype), Some(proto));
    assert!(w.despawn(copy));
    assert_eq!(w.duplicate(copy), None, "not alive");
}

#[test]
fn a_mirror_moves_into_the_geometry_with_fronts_and_bitangents_kept() {
    let mut w = World::new();
    let e = w.spawn();
    w.insert(e, triangle());
    w.insert(e, Name("p".into()));
    w.insert(
        e,
        MeshFile {
            path: "a.glb".into(),
            mesh: 0,
            materials: vec![None],
        },
    );
    let before = fronts_and_bitangent(w.get::<Mesh>(e).unwrap());
    let l = Linear::mirror_x();
    let dropped = w.apply_linear(e, &l).expect("every component follows");
    assert_eq!(dropped, ["core.mesh_file"], "what it dropped is reported");
    let m = w.get::<Mesh>(e).unwrap().clone();
    // positions, normals and tangent directions mirrored in x, bit for bit (negation only)
    assert_eq!(
        f3(&m, Semantic::Position),
        [[-0.0, 0.0, 0.0], [-0.0, 0.0, 1.0], [-1.0, 0.0, 0.0]]
    );
    assert_eq!(f3(&m, Semantic::Normal), [[0.0, 1.0, 0.0]; 3].to_vec());
    assert_eq!(f4(&m, Semantic::Tangent(0)), [[-1.0, 0.0, 0.0, -1.0]; 3].to_vec());
    assert_eq!(f4(&m, Semantic::Tangent(1)), [[0.0, 0.0, 1.0, 1.0]; 3].to_vec());
    // the triangle reversed, so its front still faces its normal; the bitangent is the mirrored one (x negated)
    assert_eq!(m.primitives()[0].indices.as_deref(), Some(&[0, 2, 1][..]));
    let after = fronts_and_bitangent(&m);
    assert!(before.0[0] > 0.0 && after.0[0] > 0.0, "{before:?} {after:?}");
    assert_eq!(after.1, [-before.1[0], before.1[1], before.1[2]]);
    // the file no longer holds this geometry; the name holds no geometry
    assert!(!w.has::<MeshFile>(e));
    assert_eq!(w.get::<Name>(e), Some(&Name("p".into())));
}

#[test]
fn a_shear_maps_points_and_keeps_normals_perpendicular() {
    let mut w = World::new();
    let e = w.spawn();
    w.insert(e, triangle());
    // local +X leans toward +Y: the triangle's plane y = 0 tilts
    let l = Linear::new([[1.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    w.apply_linear(e, &l).expect("mesh follows");
    let m = w.get::<Mesh>(e).unwrap();
    let p = f3(m, Semantic::Position);
    assert_eq!(p, [[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 1.0, 0.0]]);
    let n = f3(m, Semantic::Normal)[0];
    for edge in [p[1], p[2]] {
        assert!(dot(n, edge).abs() < 1e-6, "{n:?} against {edge:?}");
    }
    assert!((dot(n, n) - 1.0).abs() < 1e-6 && n[1] > 0.0, "{n:?}");
    assert_eq!(
        m.primitives()[0].indices.as_deref(),
        Some(&[0, 1, 2][..]),
        "no mirror, order kept"
    );
}

#[test]
fn a_component_that_cannot_follow_leaves_the_entity_unchanged() {
    let mut w = World::new();
    let e = w.spawn();
    w.insert(e, triangle());
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
        Light {
            kind: LightKind::Point,
            color: [1.0; 3],
            intensity: 1.0,
            range: None,
            cast_shadows: None,
            radius: None,
            specular: None,
        },
    );
    w.insert_opaque(
        e,
        "game.thing",
        Opaque {
            version: 1,
            data: OpaqueData::Bytes(vec![1]),
        },
    )
    .unwrap();
    let err = w.apply_linear(e, &Linear::mirror_x()).unwrap_err();
    assert_eq!(err, ["core.light", "game.thing"]);
    assert_eq!(w.get::<Mesh>(e), Some(&triangle()), "nothing changed");
    assert!(w.has::<MeshFile>(e));
    // a transform is the pass's to change, not mapped
    let t = w.spawn();
    w.insert(t, Transform::IDENTITY);
    w.apply_linear(t, &Linear::mirror_x())
        .expect("transform is invariant");
    assert_eq!(w.get::<Transform>(t), Some(&Transform::IDENTITY));
}

/// Colliders under a map, by hand: a box (centre (1, 2, 3), half extents (1, 0.5, 2)) mirrored in x is the box
/// centred at (-1, 2, 3) with the same half extents; sheared (x -> (1, 1, 0)) it is the hull of its corners
/// (x, x + y, z); a sphere of radius 1 at (1, 2, 3) under diag(-2, 2, 2) is radius 2 at (-2, 4, 6); under the shear it
/// is an ellipsoid, unsupported; a convex shape's points map as points; a heightfield follows only a positive scale
/// per axis.
#[test]
fn colliders_map_exactly_or_not_at_all() {
    use primase_core::{Collider, Diagonal, Heightfield, Shape, Surface, Surfaces};
    let collider = |shape| Collider { shape, surface: 0 };
    let shear = Linear::new([[1.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    let mut w = World::new();
    w.insert_resource(Surfaces(vec![Surface {
        name: "stone".into(),
        class: None,
    }]));
    let b = w.spawn();
    w.insert(
        b,
        collider(Shape::Box {
            center: [1.0, 2.0, 3.0],
            half_extents: [1.0, 0.5, 2.0],
        }),
    );
    w.apply_linear(b, &Linear::mirror_x()).unwrap();
    assert_eq!(
        w.get::<Collider>(b).unwrap().shape,
        Shape::Box {
            center: [-1.0, 2.0, 3.0],
            half_extents: [1.0, 0.5, 2.0],
        }
    );
    let s = w.spawn();
    w.insert(
        s,
        collider(Shape::Box {
            center: [1.0, 2.0, 3.0],
            half_extents: [1.0, 0.5, 2.0],
        }),
    );
    w.apply_linear(s, &shear).unwrap();
    // corners (x in {0, 2}, y in {1.5, 2.5}, z in {1, 5}), each to (x, x + y, z), in the order x fastest, then y, z
    let mut want = Vec::new();
    for z in [1.0, 5.0] {
        for y in [1.5, 2.5] {
            for x in [0.0, 2.0] {
                want.push([x, x + y, z]);
            }
        }
    }
    assert_eq!(
        w.get::<Collider>(s).unwrap().shape,
        Shape::Convex { points: want }
    );
    let sp = w.spawn();
    w.insert(
        sp,
        collider(Shape::Sphere {
            center: [1.0, 2.0, 3.0],
            radius: 1.0,
        }),
    );
    let mut sp2 = w.clone();
    w.apply_linear(
        sp,
        &Linear::new([[-2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 2.0]]),
    )
    .unwrap();
    assert_eq!(
        w.get::<Collider>(sp).unwrap().shape,
        Shape::Sphere {
            center: [-2.0, 4.0, 6.0],
            radius: 2.0,
        }
    );
    assert_eq!(sp2.apply_linear(sp, &shear).unwrap_err(), ["core.collider"]);
    let c = w.spawn();
    w.insert(
        c,
        collider(Shape::Convex {
            points: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 2.0]],
        }),
    );
    w.apply_linear(c, &shear).unwrap();
    assert_eq!(
        w.get::<Collider>(c).unwrap().shape,
        Shape::Convex {
            points: vec![[1.0, 1.0, 0.0], [0.0, 1.0, 2.0]],
        }
    );
    let h = w.spawn();
    w.insert(h, collider(Shape::Heightfield));
    w.insert(
        h,
        Heightfield {
            columns: 2,
            rows: 2,
            spacing: [1.0, 1.0],
            heights: vec![0.0; 4],
            diagonal: Diagonal::Main,
            holes: None,
            surfaces: None,
        },
    );
    assert_eq!(
        w.apply_linear(h, &Linear::mirror_x()).unwrap_err(),
        ["core.heightfield"]
    );
    // a positive scale per axis keeps the grid: spacing (1, 1) -> (2, 3), heights times 0.5
    w.get_mut::<Heightfield>(h).unwrap().heights = vec![0.0, 1.0, 2.0, 4.0];
    w.apply_linear(
        h,
        &Linear::new([[2.0, 0.0, 0.0], [0.0, 0.5, 0.0], [0.0, 0.0, 3.0]]),
    )
    .unwrap();
    let hf = w.get::<Heightfield>(h).unwrap();
    assert_eq!(hf.spacing, [2.0, 3.0]);
    assert_eq!(hf.heights, [0.0, 0.5, 1.0, 2.0]);
}
