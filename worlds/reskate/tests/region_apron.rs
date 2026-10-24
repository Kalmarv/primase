//! What the region clip leaves of the placements outside the region, and what the map's limit leaves out: the apron
//! `clip-reskate-region` records is applied where the writer assembles geometry and reported as the clip's; a
//! placement whose transform is degenerate is an error; what lies past the map's limit is left out and reported.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::Path;

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{Entity, Instance, Mesh, Name, Primitive, Semantic, Stream, Topology, Transform, World};
use primase_reskate::EncodingOptions;
use primase_reskate::components::{
    MarkerKind, Other, SkateMarker, SkatePlacement, SkateRegionClip, SkateScene,
};
use primase_reskate::passes::{ClipReskateRegion, RaiseOptions};

/// A row of ten 5 m triangles along east, from the placement out to 50 m.
fn strip(w: &mut World) -> Entity {
    let mut pos = Vec::new();
    for i in 0..10 {
        let x = i as f32 * 5.0;
        pos.extend([[x, 0.0, 0.0], [x + 5.0, 0.0, 0.0], [x, 0.0, -1.0]]);
    }
    let material = w.spawn();
    w.insert(material, Name("stone".into()));
    w.insert(material, PbrMaterial::default());
    let p = w.spawn();
    w.insert(p, Name("strip".into()));
    w.insert(
        p,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(pos))]),
            indices: None,
            material: Some(material),
        }]),
    );
    p
}

fn place(w: &mut World, proto: Entity, east: f32, north: f32) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[3] = [east, 0.0, -north];
    w.insert(e, t);
    e
}

/// The strip placed inside the region [0, 0, 100, 100) (east 30 to 80), from 40 m west of it (east -40 to 10: its
/// western 10 m past a 30 m apron, its eastern end in the region), from 60 m west (east -60 to -10: nothing reaching the
/// region) and from 200 m west (past the apron), raised, with a spawn.
fn world() -> (World, [Entity; 4]) {
    let mut w = World::new();
    let proto = strip(&mut w);
    let placed = [
        place(&mut w, proto, 30.0, 50.0),
        place(&mut w, proto, -40.0, 50.0),
        place(&mut w, proto, -60.0, 50.0),
        place(&mut w, proto, -200.0, 50.0),
    ];
    primase_reskate::passes::raise_skate(
        &mut w,
        &RaiseOptions::new(
            "apron",
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
            position: Some(([50.0, -50.0], Some(0.0))),
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
    (w, placed)
}

fn write(w: &World, tag: &str, e: EncodingOptions) -> Result<primase_reskate::BuildReport, String> {
    let out_root = std::env::temp_dir()
        .join(format!("primase-reskate-region-apron-{}", std::process::id()))
        .join(tag);
    primase_reskate::write_map(
        w,
        Path::new(""),
        &EncodingOptions {
            out_root,
            name: Some("apron".into()),
            ..e
        },
    )
    .map_err(|e| e.to_string())
}

/// The count of the entry of `list` (a report's `omitted` or `approximated`) that starts `starts`.
fn notice(list: &[(String, u64)], starts: &str) -> Option<u64> {
    list.iter()
        .find(|(what, _)| what.starts_with(starts))
        .map(|(_, n)| *n)
}

/// The clip marks the three placements west of the region outside and records its 30 m apron; the one reaching into
/// the region is trimmed to the apron, the two with nothing reaching it are left out, both reported as the clip's.
#[test]
fn the_region_clip_trims_and_drops_and_says_so() {
    let (mut w, _) = world();
    ClipReskateRegion::new([0.0, 0.0, 100.0, 100.0], 30.0)
        .unwrap()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    assert_eq!(
        w.resource::<SkateRegionClip>(),
        Some(&SkateRegionClip {
            region: [0.0, 0.0, 100.0, 100.0],
            apron: 30.0
        })
    );
    let r = write(&w, "clipped", EncodingOptions::default()).unwrap();
    assert_eq!(
        notice(
            &r.omitted,
            "clip-reskate-region: placements outside the region with nothing within its apron"
        ),
        Some(2),
        "{:?}",
        r.omitted
    );
    assert_eq!(
        notice(
            &r.approximated,
            "clip-reskate-region: placements outside the region trimmed to its apron"
        ),
        Some(1),
        "{:?}",
        r.approximated
    );
}

/// A record for another region than the scene's is refused; so are placements marked outside with no apron recorded.
#[test]
fn an_apron_for_another_region_or_none_is_refused() {
    let (mut w, _) = world();
    ClipReskateRegion::new([0.0, 0.0, 100.0, 100.0], 30.0)
        .unwrap()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    w.resource_mut::<SkateScene>().unwrap().region = Some([0.0, 0.0, 90.0, 100.0]);
    let e = write(&w, "other-region", EncodingOptions::default()).unwrap_err();
    assert!(e.contains("was made for the region"), "{e}");
    let (mut w, [_, west, ..]) = world();
    w.get_mut::<SkatePlacement>(west).unwrap().clip = Some(true);
    let e = write(&w, "no-apron", EncodingOptions::default()).unwrap_err();
    assert!(e.contains("no apron was recorded"), "{e}");
}

/// A placement whose transform is degenerate is an error naming it.
#[test]
fn a_degenerate_placement_is_an_error() {
    let (mut w, [inside, ..]) = world();
    w.get_mut::<Transform>(inside).unwrap().cols[0] = [0.0; 3];
    let e = write(&w, "degenerate", EncodingOptions::default()).unwrap_err();
    assert!(
        e.contains(&format!("placement {inside:?}")) && e.contains("degenerate"),
        "{e}"
    );
}

/// Past the map's limit: an instanced placement reaching it is left out, a merged one's triangles past it are; each
/// reported with its count.
#[test]
fn what_lies_past_the_limit_is_left_out_and_reported() {
    let (w, _) = world();
    // a 55 m limit: the strip inside the region (east 30 to 80) reaches past it
    let merged = EncodingOptions {
        limit: 55.0,
        ..EncodingOptions::default()
    };
    let r = write(&w, "limit-merged", merged).unwrap();
    assert!(
        notice(&r.omitted, "triangles past the map's limit").is_some_and(|n| n > 0),
        "{:?}",
        r.omitted
    );
    let instanced = EncodingOptions {
        limit: 55.0,
        min_instances: 1,
        min_instance_tris: 0,
        ..EncodingOptions::default()
    };
    let r = write(&w, "limit-instanced", instanced).unwrap();
    assert!(
        notice(&r.omitted, "instanced placements reaching past the map's limit").is_some_and(|n| n > 0),
        "{:?}",
        r.omitted
    );
}
