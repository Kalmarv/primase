//! `prepare-reskate-collision`: a leaf's minor surfaces written as its most common one (`surfaces=coalesce`) or kept
//! (`exact`); its pieces welded at a millimetre, the triangles that collapse declared as dropped (`weld=mm`), or only
//! bit-identical corners merged (`exact`). The writer replays the plan, refuses one made for another layout, and
//! without one writes the collision exactly.

#![allow(clippy::unwrap_used)]

use std::path::Path;

use primase_core::ledger::Measure;
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Collision, Surface, Surfaces, World};
use primase_reskate::EncodingOptions;
use primase_reskate::components::{
    CollisionSurfaces, CollisionWeld, MarkerKind, Other, SkateCollisionPlan, SkateMarker, SkateScene,
};
use primase_reskate::passes::PrepareReskateCollision;

const CONCRETE: u16 = 1;
const METAL: u16 = 3;

/// A triangle of a metre at east `x` (scene frame: north is -z), or a sliver 0.4 mm wide there (`thin`): wide enough
/// to be kept as a triangle, narrow enough for the 1 mm weld to collapse.
fn tri(x: f32, thin: bool) -> [[f32; 3]; 3] {
    let w = if thin { 0.0004 } else { 1.0 };
    [[x, 0.0, -1.0], [x + 1.0, 0.0, -1.0], [x + 1.0, 0.0, -1.0 - w]]
}

/// A map of one world soup: 20 concrete triangles and `more`, and a spawn.
fn world(more: &[(u16, bool)]) -> World {
    let mut w = World::new();
    let table = ["default", "concrete", "asphalt", "metal"];
    w.insert_resource(Surfaces(table.iter().map(|&n| Surface::new(n)).collect()));
    w.insert_resource(SkateScene {
        name: "plan".into(),
        surfaces: Some(table.iter().map(|&n| n.to_owned()).collect()),
        ..Default::default()
    });
    let mut c = Collision::default();
    let all: Vec<(u16, bool)> = std::iter::repeat_n((CONCRETE, false), 20)
        .chain(more.iter().copied())
        .collect();
    for (i, &(surface, thin)) in all.iter().enumerate() {
        c.extend(&tri(i as f32 * 1.5, thin), &[[0, 1, 2]], surface, |p| p);
    }
    let soup = w.spawn();
    w.insert(soup, c);
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

fn plan(w: &mut World, surfaces: CollisionSurfaces, weld: CollisionWeld) -> PassLog {
    let mut log = PassLog::default();
    PrepareReskateCollision::new(surfaces, weld, &EncodingOptions::default())
        .unwrap()
        .run(w, &mut log)
        .unwrap();
    log
}

/// What the writer writes of `w`'s collision: its triangle-mesh pieces' names, and its collision triangles.
fn written(w: &World, tag: &str) -> primase_reskate::Result<(Vec<String>, i64)> {
    let out_root = std::env::temp_dir()
        .join(format!("primase-reskate-collision-plan-{}", std::process::id()))
        .join(tag);
    let r = primase_reskate::write_map(
        w,
        Path::new(""),
        &EncodingOptions {
            out_root,
            name: Some("plan".into()),
            ..EncodingOptions::default()
        },
    )?;
    let map: serde_json::Value =
        serde_json::from_slice(&std::fs::read(r.out_dir.join("map.json")).unwrap()).unwrap();
    let mut pieces: Vec<String> = map["objects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["collision_mode"] == "triangle_mesh")
        .map(|o| o["name"].as_str().unwrap().to_owned())
        .collect();
    pieces.sort();
    std::fs::remove_dir_all(&r.out_dir).ok();
    Ok((pieces, r.collision_tris))
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

/// 20 concrete triangles and 1 metal in one leaf: coalesced, the metal one is written as concrete (an approximation);
/// exact, and without a plan, it is a piece of its own.
#[test]
fn a_minor_surface_is_coalesced_or_kept() {
    let mut w = world(&[(METAL, false)]);
    let log = plan(&mut w, CollisionSurfaces::Coalesce, CollisionWeld::Millimetre);
    let p = w.resource::<SkateCollisionPlan>().unwrap();
    assert_eq!(p.leaves.len(), 1);
    let leaf = p.leaves.values().next().unwrap();
    assert_eq!(leaf.into, [(METAL as usize, CONCRETE as usize)].into());
    assert_eq!(
        log.approximations["collision triangles of surface metal written as their leaf's concrete"],
        1
    );
    assert_eq!(log.declared(Measure::CollisionTriangles), 0);
    assert_eq!(written(&w, "coalesce").unwrap(), (names(&["c_0_0_concrete"]), 21));

    let mut w = world(&[(METAL, false)]);
    let log = plan(&mut w, CollisionSurfaces::Exact, CollisionWeld::Exact);
    assert!(log.approximations.is_empty());
    let both = (names(&["c_0_0_concrete", "c_0_0_metal"]), 21);
    assert_eq!(written(&w, "exact").unwrap(), both);
    assert_eq!(
        written(&world(&[(METAL, false)]), "minor-unplanned").unwrap(),
        both
    );
}

/// A sliver the 1 mm weld collapses is declared as a dropped collision triangle and not written; the exact weld, and
/// the writer without a plan, keep it.
#[test]
fn the_weld_collapses_are_declared_drops() {
    let mut w = world(&[(CONCRETE, true)]);
    let log = plan(&mut w, CollisionSurfaces::Exact, CollisionWeld::Millimetre);
    assert_eq!(log.declared(Measure::CollisionTriangles), 1);
    assert_eq!(log.declared(Measure::Triangles), 0);
    assert_eq!(written(&w, "mm").unwrap(), (names(&["c_0_0_concrete"]), 20));

    let mut w = world(&[(CONCRETE, true)]);
    let log = plan(&mut w, CollisionSurfaces::Exact, CollisionWeld::Exact);
    assert_eq!(log.declared(Measure::CollisionTriangles), 0);
    assert_eq!(written(&w, "kept").unwrap(), (names(&["c_0_0_concrete"]), 21));
    assert_eq!(
        written(&world(&[(CONCRETE, true)]), "sliver-unplanned").unwrap(),
        (names(&["c_0_0_concrete"]), 21)
    );
}

/// A surface whose only triangle the weld collapses leaves no piece at all.
#[test]
fn a_piece_the_weld_empties_is_not_written() {
    let mut w = world(&[(METAL, true)]);
    let log = plan(&mut w, CollisionSurfaces::Exact, CollisionWeld::Millimetre);
    assert_eq!(log.declared(Measure::CollisionTriangles), 1);
    assert_eq!(written(&w, "emptied").unwrap(), (names(&["c_0_0_concrete"]), 20));
}

/// A plan made for another layout is refused: a leaf with other triangles, a leaf the layout no longer has.
#[test]
fn a_plan_for_another_layout_is_refused() {
    let mut w = world(&[(METAL, false)]);
    plan(&mut w, CollisionSurfaces::Coalesce, CollisionWeld::Millimetre);
    let soup = w.iter::<Collision>().map(|(e, _)| e).next().unwrap();
    w.get_mut::<Collision>(soup)
        .unwrap()
        .extend(&tri(35.0, false), &[[0, 1, 2]], METAL, |p| p);
    let e = written(&w, "stale").unwrap_err().to_string();
    assert!(
        e.contains("planned for another layout (leaf c_0_0 has the triangles"),
        "{e}"
    );
    // a triangle in a cell the plan never saw: a leaf it does not have
    w.get_mut::<Collision>(soup)
        .unwrap()
        .extend(&tri(-10.0, false), &[[0, 1, 2]], METAL, |p| p);
    let e = written(&w, "new-cell").unwrap_err().to_string();
    assert!(e.contains("is not in the plan"), "{e}");

    let mut w = world(&[(METAL, false)]);
    plan(&mut w, CollisionSurfaces::Coalesce, CollisionWeld::Millimetre);
    let mut p = w.resource::<SkateCollisionPlan>().unwrap().clone();
    let leaf = p.leaves.values().next().unwrap().clone();
    p.leaves.insert("c_9_9".into(), leaf);
    w.insert_resource(p);
    let e = written(&w, "extra").unwrap_err().to_string();
    assert!(e.contains("the plan has 2 leaves, the layout 1"), "{e}");
}

/// Both choices are the program's; the options print every setting and read back as the same pass.
#[test]
fn the_choices_are_required_and_printed() {
    let o = |kv: &[(&str, &str)]| -> Options {
        kv.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    };
    assert!(PrepareReskateCollision::from_options(&o(&[("weld", "mm")])).is_err());
    assert!(PrepareReskateCollision::from_options(&o(&[("surfaces", "coalesce")])).is_err());
    assert!(PrepareReskateCollision::from_options(&o(&[("surfaces", "some"), ("weld", "mm")])).is_err());
    assert!(
        PrepareReskateCollision::from_options(&o(&[("surfaces", "exact"), ("weld", "mm"), ("grid", "1")]))
            .is_err()
    );
    let p = PrepareReskateCollision::from_options(&o(&[
        ("surfaces", "coalesce"),
        ("weld", "mm"),
        ("tile", "128"),
    ]))
    .unwrap();
    let printed = p.options();
    assert_eq!(
        (printed["surfaces"].as_str(), printed["weld"].as_str()),
        ("coalesce", "mm")
    );
    assert_eq!(printed["tile"], "128");
    assert_eq!(PrepareReskateCollision::from_options(&printed).unwrap(), p);
}
