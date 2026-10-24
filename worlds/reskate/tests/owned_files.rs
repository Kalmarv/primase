//! The map writer owns the files it writes in the map's folder: a run leaves none of an earlier run's that it did not
//! write itself (a pause-map picture, a vertex-blend list, a collision file, a trainer list), and none that is not its
//! own.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::Path;

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{Instance, Mesh, Name, Primitive, Semantic, Stream, Topology, Transform, World};
use primase_reskate::components::{MarkerKind, Other, SkateMarker};
use primase_reskate::passes::{
    LightCalibration, PauseColour, PauseDrawing, PauseExtent, PauseLeaveOut, PauseSource,
    PrepareReskatePauseMap, RaiseOptions,
};
use primase_reskate::{EncodingOptions, write_map};

fn map() -> World {
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
                Stream::F32x3(vec![[0.0; 3], [4.0, 0.0, 0.0], [0.0, 0.0, -4.0]]),
            )]),
            indices: None,
            material: Some(material),
        }]),
    );
    let p = w.spawn();
    w.insert(
        p,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    w.insert(p, Transform::IDENTITY);
    primase_reskate::passes::raise_skate(
        &mut w,
        &RaiseOptions::new(
            "owned",
            LightCalibration::Standard,
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

fn files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn a_run_leaves_none_of_an_earlier_runs_files_it_did_not_write() {
    let root = std::env::temp_dir().join(format!("primase-owned-{}", std::process::id()));
    let e = EncodingOptions {
        out_root: root.clone(),
        name: Some("owned".into()),
        ..EncodingOptions::default()
    };
    // the first run: a pause-map picture too
    let mut first = map();
    PrepareReskatePauseMap::new(PauseSource::Geometry {
        height: 90,
        extent: PauseExtent::Rect([-50.0, -50.0, 50.0, 50.0]),
        drawing: PauseDrawing {
            leave_out: PauseLeaveOut::Nothing,
            colour: PauseColour::Mean,
        },
    })
    .unwrap()
    .run(&mut first, &mut PassLog::default())
    .unwrap();
    let out = write_map(&first, Path::new(""), &e).unwrap().out_dir;
    assert!(
        files(&out).iter().any(|f| f.starts_with("ui/")),
        "{:?}",
        files(&out)
    );
    // files an earlier run could have left: owned ones, and one that is not the writer's
    std::fs::create_dir_all(out.join("collision")).unwrap();
    for stale in [
        "vertex_blend.json",
        "trainer.json",
        "collision/world_7.bin",
        "old_hull.glb",
    ] {
        std::fs::write(out.join(stale), b"stale").unwrap();
    }
    std::fs::write(out.join("NOTES.txt"), b"mine").unwrap();
    // the second run: no picture, no vertex blend, no trainer spot
    write_map(&map(), Path::new(""), &e).unwrap();
    let left = files(&out);
    for gone in [
        "ui/map_image.png",
        "ui/map_image.json",
        "ui/map_clouds.png",
        "vertex_blend.json",
        "trainer.json",
        "collision/world_7.bin",
        "old_hull.glb",
    ] {
        assert!(!left.iter().any(|f| f == gone), "{gone} survived: {left:?}");
    }
    assert!(
        left.iter().any(|f| f == "NOTES.txt"),
        "a file that is not the writer's stays: {left:?}"
    );
    assert!(left.iter().any(|f| f == "map.json"), "{left:?}");
    std::fs::remove_dir_all(root).ok();
}
