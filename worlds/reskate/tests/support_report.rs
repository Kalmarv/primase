//! What the writer reports it did not write. A core name raise-skate raises is covered by its raised form: a light
//! raised to a `skate.light` is written, one without is reported as not written (the target's check, which a direct
//! `write_map` caller gets in its report too). What the writer leaves out is counted in `omitted`, what it writes
//! approximately in `approximated`, never as left out.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::Path;

use primase_core::pbr::PbrMaterial;
use primase_core::{
    Instance, Light, LightKind, Mesh, Name, Primitive, Semantic, Stream, Topology, Transform, World,
};
use primase_reskate::EncodingOptions;
use primase_reskate::components::{MarkerKind, Other, SkateLight, SkateMarker};
use primase_reskate::passes::RaiseOptions;

fn light(w: &mut World, x: f32) -> primase_core::Entity {
    let e = w.spawn();
    w.insert(
        e,
        Light {
            kind: LightKind::Point,
            color: [1.0, 0.9, 0.8],
            intensity: 100.0,
            range: Some(5.0),
            cast_shadows: None,
            radius: None,
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[3] = [x, 2.0, -1.0];
    w.insert(e, t);
    e
}

/// A raised map: a prototype of one triangle placed once, a light raised with it, and a spawn.
fn raised() -> World {
    let mut w = World::new();
    let material = w.spawn();
    w.insert(material, Name("stone".into()));
    w.insert(material, PbrMaterial::default());
    let proto = w.spawn();
    w.insert(proto, Name("slab".into()));
    w.insert(
        proto,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(
                Semantic::Position,
                Stream::F32x3(vec![[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 0.0, -4.0]]),
            )]),
            indices: None,
            material: Some(material),
        }]),
    );
    let placed = w.spawn();
    w.insert(
        placed,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    w.insert(placed, Transform::IDENTITY);
    light(&mut w, 1.0);
    primase_reskate::passes::raise_skate(
        &mut w,
        &RaiseOptions::new(
            "support",
            primase_reskate::passes::LightCalibration::Standard,
            primase_reskate::passes::Unclassified::LeaveOut,
        )
        .unwrap(),
    )
    .unwrap();
    let spawn = w.spawn();
    w.insert(
        spawn,
        SkateMarker {
            kind: MarkerKind::Spawn,
            position: Some(([1.0, 1.0], Some(0.0))),
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

fn write(w: &World, tag: &str) -> primase_reskate::BuildReport {
    let out_root = std::env::temp_dir()
        .join(format!("primase-reskate-support-{}", std::process::id()))
        .join(tag);
    let r = primase_reskate::write_map(
        w,
        Path::new(""),
        &EncodingOptions {
            out_root,
            name: Some("support".into()),
            ..EncodingOptions::default()
        },
    )
    .unwrap();
    std::fs::remove_dir_all(&r.out_dir).ok();
    r
}

/// The review's case: one light raised, one added after raise-skate. The unraised one is reported, once; the raised one
/// is not; the target's check and a direct write_map caller's report say the same.
#[test]
fn an_unraised_light_is_reported_and_a_raised_one_is_not() {
    let mut w = raised();
    assert_eq!(w.iter::<SkateLight>().count(), 1);
    assert!(
        primase_reskate::target()
            .check_prepared(&w)
            .unsupported
            .is_empty()
    );
    assert!(write(&w, "raised").unsupported_names.is_empty());

    light(&mut w, 3.0);
    let expected = vec![("core.light not covered by skate.light".to_owned(), 1)];
    assert_eq!(primase_reskate::target().check_prepared(&w).unsupported, expected);
    let r = write(&w, "mixed");
    assert_eq!(r.unsupported_names, expected);
    assert_eq!(
        r.omitted,
        Vec::<(String, u64)>::new(),
        "a name the target does not take is not the writer's loss"
    );
}

/// A placement of a prototype with nothing to draw or collide with is counted as left out, not only warned of.
#[test]
fn a_placement_without_geometry_is_counted_as_left_out() {
    let mut w = raised();
    let empty = w.spawn();
    let placed = w.spawn();
    w.insert(
        placed,
        Instance {
            prototype: empty,
            variant: 0,
        },
    );
    w.insert(placed, Transform::IDENTITY);
    let r = write(&w, "no-geometry");
    assert_eq!(
        r.omitted,
        [("placements of prototypes without geometry".to_owned(), 1)]
    );
    assert!(r.approximated.is_empty(), "{:?}", r.approximated);
    assert!(
        r.warnings
            .iter()
            .any(|m| m == "1 placements of prototypes without geometry left out"),
        "{:?}",
        r.warnings
    );
}
