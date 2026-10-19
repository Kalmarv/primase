//! What the map writer takes: what it does not write is reported and does not change the map (a prepared world holds
//! only names the writer's target lists; a name of another namespace on a placement, or a resource of one, leaves the
//! map folder byte for byte as it was and is reported by the target's check as not written), and a world its own
//! components refuse (geometry that is not finite) is an error naming what failed, in the writer and in the passes
//! that assemble its meshes.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{
    Component, Entity, Instance, Mesh, Name, Primitive, Resource, Semantic, Stream, Topology, Transform,
    World,
};
use primase_reskate::EncodingOptions;
use primase_reskate::components::{MarkerKind, Other, SkateMarker};
use primase_reskate::passes::{PrepareReskateOverlays, RaiseOptions, SnapReskateMarkers};

/// A component of a namespace the writer does not know.
#[derive(Clone, Debug)]
struct Foreign;

impl Component for Foreign {
    const NAME: &'static str = "test.foreign";
}

/// A resource of a namespace the writer does not know.
#[derive(Clone, Debug)]
struct ForeignState;

impl Resource for ForeignState {
    const NAME: &'static str = "test.foreign_state";
}

/// A 10 m triangle placed twice, raised, with a spawn, its markers snapped: (the world, the first placement).
fn world() -> (World, Entity) {
    let mut w = World::new();
    let material = w.spawn();
    w.insert(material, Name("stone".into()));
    w.insert(material, PbrMaterial::default());
    let proto = w.spawn();
    w.insert(proto, Name("tri".into()));
    w.insert(
        proto,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(
                Semantic::Position,
                Stream::F32x3(vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 0.0, -10.0]]),
            )]),
            indices: None,
            material: Some(material),
        }]),
    );
    let place = |w: &mut World, east: f32| {
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [east, 0.0, -20.0];
        w.insert(e, t);
        e
    };
    let first = place(&mut w, 0.0);
    place(&mut w, 30.0);
    primase_reskate::passes::raise_skate(&mut w, &RaiseOptions::new("foreign").unwrap()).unwrap();
    let spawn = w.spawn();
    w.insert(
        spawn,
        SkateMarker {
            kind: MarkerKind::Spawn,
            position: Some(([5.0, 25.0], Some(0.0))),
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
    SnapReskateMarkers::for_default_encoding()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    (w, first)
}

/// Every file of the map `w` writes, by path; the writer's error if it refuses.
fn try_files(w: &World, tag: &str) -> Result<BTreeMap<PathBuf, Vec<u8>>, String> {
    let out_root = std::env::temp_dir()
        .join(format!("primase-reskate-writer-input-{}", std::process::id()))
        .join(tag);
    let _ = std::fs::remove_dir_all(&out_root);
    primase_reskate::write_map(
        w,
        Path::new(""),
        &EncodingOptions {
            out_root: out_root.clone(),
            name: Some("input".into()),
            ..EncodingOptions::default()
        },
    )
    .map_err(|e| e.to_string())?;
    let mut out = BTreeMap::new();
    let mut stack = vec![out_root.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(
                    p.strip_prefix(&out_root).unwrap().to_path_buf(),
                    std::fs::read(&p).unwrap(),
                );
            }
        }
    }
    let _ = std::fs::remove_dir_all(&out_root);
    Ok(out)
}

/// Every file of the map `w` writes, by path.
fn files(w: &World, tag: &str) -> BTreeMap<PathBuf, Vec<u8>> {
    try_files(w, tag).unwrap()
}

#[test]
fn a_foreign_name_leaves_the_map_unchanged_and_is_reported() {
    let target = primase_reskate::target();
    let (mut w, first) = world();
    // a prepared world holds only what the writer writes
    let checked = target.check_prepared(&w);
    assert!(checked.unsupported.is_empty(), "{:?}", checked.unsupported);
    assert!(
        checked.unsupported_resources.is_empty(),
        "{:?}",
        checked.unsupported_resources
    );
    let before = files(&w, "before");
    // a component and a resource of another namespace: the same map, both reported as not written
    w.insert(first, Foreign);
    w.insert_resource(ForeignState);
    assert_eq!(files(&w, "after"), before);
    let checked = target.check_prepared(&w);
    assert_eq!(checked.unsupported, vec![("test.foreign".to_owned(), 1)]);
    assert_eq!(
        checked.unsupported_resources,
        vec!["test.foreign_state".to_owned()]
    );
    assert!(checked.invalid.is_empty() && checked.invalid_resources.is_empty());
}

#[test]
fn geometry_that_is_not_finite_is_refused_naming_it() {
    let (mut w, _) = world();
    let proto = w.iter::<Mesh>().map(|(e, _)| e).next().unwrap();
    if let Some(Stream::F32x3(p)) = w
        .get_mut::<Mesh>(proto)
        .unwrap()
        .primitive_mut(0)
        .expect("a listed primitive")
        .streams_mut()
        .get_mut(&Semantic::Position)
    {
        p[1][0] = f32::NAN;
    }
    let e = try_files(&w, "nan").unwrap_err();
    assert!(
        e.contains(&format!("core.mesh of {proto:?}")) && e.contains("not finite"),
        "{e}"
    );
    // the passes that assemble the writer's meshes refuse it alike
    let e = PrepareReskateOverlays::new(0.004)
        .unwrap()
        .run(&mut w, &mut PassLog::default())
        .unwrap_err();
    assert!(e.contains("core.mesh") && e.contains("not finite"), "{e}");
}
