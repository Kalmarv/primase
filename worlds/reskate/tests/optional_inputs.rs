//! A map's features are optional: a scene with no materials, no blends, no grind curves and no decals goes through the
//! ReSkate preparation passes a program runs for them, as the pass manager runs them, and the writer: each pass finds
//! nothing to do, and says so where it counts.

#![allow(clippy::unwrap_used)]

use std::path::Path;

use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Collision, Surfaces, World};
use primase_reskate::dialect::{MarkerKind, Other, SkateMarker};
use primase_reskate::prepare::RaiseOptions;
use primase_reskate::{EncodingOptions, WriterChoices};

#[test]
fn a_map_without_the_optional_features_is_prepared_and_written() {
    // collision and a spawn, nothing drawn
    let mut w = World::new();
    let mut surfaces = Surfaces::default();
    let concrete = surfaces.index("concrete").unwrap();
    w.insert_resource(surfaces);
    let ground = w.spawn();
    let mut c = Collision::default();
    c.extend(
        &[
            [-50.0, 0.0, 50.0],
            [50.0, 0.0, 50.0],
            [-50.0, 0.0, -50.0],
            [50.0, 0.0, -50.0],
        ],
        &[[0, 1, 2], [2, 1, 3]],
        concrete,
        |p| p,
    );
    w.insert(ground, c);
    primase_reskate::prepare::raise_skate(
        &mut w,
        &RaiseOptions {
            name: "bare".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let spawn = w.spawn();
    w.insert(
        spawn,
        SkateMarker {
            kind: MarkerKind::Spawn,
            position: Some(([0.0, 0.0], Some(0.0))),
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
    let mut registry = PassRegistry::new();
    primase_reskate::register(&mut registry);
    let pipeline = registry
        .parse(
            "lower-layered-reskate, prepare-reskate-textures, budget-reskate-textures{max=64}, \
             prepare-reskate-grinds, prepare-reskate-overlays{step=0.004}",
        )
        .unwrap();
    let mut report = RunReport::default();
    PassManager::new(registry)
        .run(&mut w, &pipeline, &mut report)
        .unwrap();
    let count = |pass: &str, what: &str| {
        report
            .steps
            .iter()
            .find(|s| s.pass.starts_with(pass))
            .and_then(|s| s.log.counts.get(what).copied())
    };
    assert_eq!(
        count("prepare-reskate-grinds", "grind curves made prisms"),
        Some(0)
    );
    assert_eq!(
        count("prepare-reskate-overlays", "overlay pieces lifted"),
        Some(0)
    );
    let out_root = std::env::temp_dir().join(format!("primase-reskate-optional-{}", std::process::id()));
    let r = primase_reskate::write_map(
        &w,
        Path::new(""),
        &EncodingOptions {
            out_root,
            name: Some("bare".into()),
            ..EncodingOptions::default()
        },
        &WriterChoices::default(),
    )
    .unwrap();
    assert_eq!((r.tiles, r.textures), (0, 0));
    assert!(r.collision_tris > 0);
    assert!(r.unsupported.is_empty(), "{:?}", r.unsupported);
}
