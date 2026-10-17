//! `bake-nonrigid`: what a mirrored or sheared placement draws, in world space, is the same before and after, and its
//! transform is regular after.
#![allow(clippy::unwrap_used)]
use std::collections::BTreeMap;

use primase_core::{
    Collision, Entity, Instance, Light, LightKind, LodForm, Lods, Mesh, Opaque, OpaqueData, Primitive,
    Semantic, Stream, Surface, Surfaces, Topology, Transform, TransformKind, World,
};
use primase_meshops::bake_nonrigid;

/// Two triangles of a little roof facing +Y with tangents of both handedness signs.
fn roof(lift: f32) -> Mesh {
    let mut streams = BTreeMap::new();
    streams.insert(
        Semantic::Position,
        Stream::F32x3(vec![
            [0.0, lift, 0.0],
            [0.0, lift + 0.5, 1.0],
            [1.0, lift, 0.0],
            [1.0, lift + 0.5, 1.0],
        ]),
    );
    let n = [0.0, 0.894_427_2, -0.447_213_6];
    streams.insert(Semantic::Normal, Stream::F32x3(vec![n; 4]));
    streams.insert(
        Semantic::Tangent,
        Stream::F32x4(vec![
            [1.0, 0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0, -1.0],
            [1.0, 0.0, 0.0, -1.0],
        ]),
    );
    Mesh {
        primitives: vec![Primitive {
            topology: Topology::Triangles,
            streams,
            indices: Some(vec![0, 1, 2, 2, 1, 3]),
            material: None,
        }],
    }
}

fn t(cols: [[f32; 3]; 4]) -> Transform {
    Transform { cols }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(a: [f64; 3]) -> [f64; 3] {
    let l = dot(a, a).sqrt();
    a.map(|x| x / l)
}

/// A triangle in world space: corners, front side, bitangents.
type Drawn = ([[f64; 3]; 3], [f64; 3], [[f64; 3]; 3]);

/// What `e` draws in world space, triangle by triangle: corners, front side (unit), bitangent at each corner (unit).
/// Under a mirroring transform the front side is the one glTF takes: winding reversed by the determinant.
fn drawn(w: &World, e: Entity) -> Vec<Drawn> {
    let tr = *w.get::<Transform>(e).unwrap();
    let mesh = match w.get::<Instance>(e) {
        Some(i) => w.get::<Mesh>(i.prototype).unwrap(),
        None => w.get::<Mesh>(e).unwrap(),
    };
    let c: [[f64; 3]; 4] = tr.cols.map(|c| c.map(f64::from));
    let lin = |v: [f64; 3]| -> [f64; 3] {
        std::array::from_fn(|r| c[0][r] * v[0] + c[1][r] * v[1] + c[2][r] * v[2])
    };
    let point = |p: [f32; 3]| -> [f64; 3] {
        let v = lin(p.map(f64::from));
        std::array::from_fn(|r| v[r] + c[3][r])
    };
    let det = dot(c[0], cross(c[1], c[2]));
    let cof = [cross(c[1], c[2]), cross(c[2], c[0]), cross(c[0], c[1])];
    let normal = |n: [f32; 3]| -> [f64; 3] {
        let n = n.map(f64::from);
        unit(std::array::from_fn(|r| {
            (cof[0][r] * n[0] + cof[1][r] * n[1] + cof[2][r] * n[2]) * det.signum()
        }))
    };
    let mut out = Vec::new();
    for p in &mesh.primitives {
        let (Some(Stream::F32x3(pos)), Some(Stream::F32x3(nor)), Some(Stream::F32x4(tan))) = (
            p.streams.get(&Semantic::Position),
            p.streams.get(&Semantic::Normal),
            p.streams.get(&Semantic::Tangent),
        ) else {
            panic!("streams")
        };
        for tri in p.indices.as_ref().unwrap().as_chunks::<3>().0 {
            let corners = tri.map(|i| point(pos[i as usize]));
            let order = if det < 0.0 { [0, 2, 1] } else { [0, 1, 2] };
            let [a, b, cc] = order.map(|k| corners[k]);
            let front = unit(cross(sub(b, a), sub(cc, a)));
            let bit = tri.map(|i| {
                let tv = tan[i as usize];
                let w = f64::from(tv[3]) * if det < 0.0 { -1.0 } else { 1.0 };
                unit(
                    cross(
                        normal(nor[i as usize]),
                        unit(lin([tv[0], tv[1], tv[2]].map(f64::from))),
                    )
                    .map(|x| x * w),
                )
            });
            out.push((corners, front, bit));
        }
    }
    out
}

fn same(a: &[Drawn], b: &[Drawn]) {
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b) {
        // corners as a set (a reversed triangle lists them in another order), within f32 rounding of world positions
        for p in x.0 {
            assert!(
                y.0.iter().any(|q| (0..3).all(|k| (p[k] - q[k]).abs() < 1e-5)),
                "{p:?} not among {:?}",
                y.0
            );
        }
        assert!(dot(x.1, y.1) > 1.0 - 1e-9, "front {:?} became {:?}", x.1, y.1);
        for b in y.2 {
            assert!(
                x.2.iter().any(|a| dot(*a, b) > 1.0 - 1e-6),
                "bitangent {b:?} not among {:?}",
                x.2
            );
        }
    }
}

#[test]
fn mirrored_and_sheared_placements_draw_the_same_with_regular_transforms() {
    let mut w = World::new();
    let proto = w.spawn();
    w.insert(proto, roof(0.0));
    let form = w.spawn();
    w.insert(form, roof(0.25));
    w.insert(
        proto,
        Lods {
            detail: 0,
            forms: vec![LodForm { detail: 1, form }],
            default: None,
        },
    );
    let place = |w: &mut World, tr: Transform| {
        let p = w.spawn();
        w.insert(p, tr);
        w.insert(
            p,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        p
    };
    // a rotation of 90 degrees about y with x scaled by -2: mirrored; twice, to share the twin
    let mirrored = t([
        [0.0, 0.0, 2.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        [10.0, 0.0, 5.0],
    ]);
    let m1 = place(&mut w, mirrored);
    let m2 = place(
        &mut w,
        t([
            [0.0, 0.0, 2.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            [-3.0, 1.0, 0.0],
        ]),
    );
    // x leans toward y: sheared; and sheared and mirrored at once
    let s1 = place(
        &mut w,
        t([
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -7.0],
        ]),
    );
    let s2 = place(
        &mut w,
        t([
            [-1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [4.0, 0.0, 0.0],
        ]),
    );
    let regular = place(&mut w, Transform::IDENTITY);
    // an entity drawing its own geometry, mirrored
    let own = w.spawn();
    w.insert(own, roof(0.0));
    w.insert(
        own,
        t([
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 2.0, 0.0],
        ]),
    );
    let before: Vec<_> = [m1, m2, s1, s2, regular, own]
        .iter()
        .map(|&e| drawn(&w, e))
        .collect();
    // a collision soup placed by a mirror, and a mirrored light
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
        t([
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 0.0],
        ]),
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

    let log = bake_nonrigid(&mut w);
    assert!(w.verify_components().is_empty(), "{:?}", w.verify_components());
    for (&e, b) in [m1, m2, s1, s2, regular, own].iter().zip(&before) {
        assert_eq!(
            w.get::<Transform>(e).unwrap().classify(),
            TransformKind::Regular,
            "{e:?}"
        );
        same(b, &drawn(&w, e));
    }
    // the two mirrored placements share one twin, whose form is twinned too; the regular one keeps the prototype
    let twin = w.get::<Instance>(m1).unwrap().prototype;
    assert_ne!(twin, proto);
    assert_eq!(w.get::<Instance>(m2).unwrap().prototype, twin);
    assert_eq!(w.get::<Instance>(regular).unwrap().prototype, proto);
    let twin_form = w.get::<Lods>(twin).unwrap().forms[0].form;
    assert_ne!(twin_form, form);
    assert_eq!(
        w.get::<Mesh>(proto),
        Some(&roof(0.0)),
        "the prototype itself is untouched"
    );
    // the form under the twin's placement draws the mirrored form
    let p = w.spawn();
    w.insert(p, *w.get::<Transform>(m1).unwrap());
    w.insert(
        p,
        Instance {
            prototype: twin_form,
            variant: 0,
        },
    );
    let q = w.spawn();
    w.insert(q, mirrored);
    w.insert(
        q,
        Instance {
            prototype: form,
            variant: 0,
        },
    );
    same(&drawn(&w, q), &drawn(&w, p));
    // the soup's triangle turned with its positions; the light only lost its mirror
    let c = w.get::<Collision>(soup).unwrap();
    assert_eq!(
        c.positions,
        [[-0.0, 0.0, 0.0], [-0.0, 0.0, 1.0], [-1.0, 0.0, 0.0]]
    );
    assert_eq!(c.triangles, [[0, 2, 1]]);
    assert_eq!(
        w.get::<Transform>(light).unwrap().classify(),
        TransformKind::Regular
    );
    assert_eq!(w.get::<Transform>(light).unwrap().cols[3], [1.0, 2.0, 3.0]);
    assert_eq!(log.counts.get("mirror twins of prototypes made"), Some(&1));
    assert_eq!(
        log.counts.get("prototype copies made for sheared placements"),
        Some(&2)
    );
    assert_eq!(
        log.counts.get("mirrored transforms made rigid"),
        Some(&5),
        "{:?}",
        log.counts
    );
    assert_eq!(
        log.counts.get("sheared transforms made rigid"),
        Some(&2),
        "{:?}",
        log.counts
    );
}

#[test]
fn a_prototype_that_cannot_follow_keeps_its_placements_as_they_were() {
    let mut w = World::new();
    let proto = w.spawn();
    w.insert(proto, roof(0.0));
    w.insert_opaque(
        proto,
        "game.local_points",
        Opaque {
            version: 1,
            data: OpaqueData::Bytes(vec![0]),
        },
    );
    let p = w.spawn();
    let mirrored = t([
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, 0.0],
    ]);
    w.insert(p, mirrored);
    w.insert(
        p,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    let entities = w.entity_count();
    let log = bake_nonrigid(&mut w);
    assert_eq!(w.get::<Transform>(p), Some(&mirrored));
    assert_eq!(w.get::<Instance>(p).unwrap().prototype, proto);
    assert_eq!(w.entity_count(), entities, "no copy left behind");
    assert_eq!(
        log.counts.get("mirrored transforms left: game.local_points"),
        Some(&1),
        "{:?}",
        log.counts
    );
}
