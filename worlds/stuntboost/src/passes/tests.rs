//! The lowerings on a synthetic level: a solid wooden ramp, a kill floor, a collision-only wall, two spawns, a
//! checkpoint trigger, the sun, a shadow-only ceiling, a transparent window and a bake material.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{
    Collision, Entity, Instance, Marker, MaterialVariants, Mesh, Primitive, Semantic, Stream, SurfaceClass,
    Surfaces, Topology, Transform, World,
};

use super::*;
use crate::components::{
    StuntboostMaterial, StuntboostNode, StuntboostPart, StuntboostRole, StuntboostTextureRef,
};
use crate::read::names::parse;

fn material(w: &mut World, name: &str, texture: Option<Entity>) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        StuntboostMaterial {
            name: name.into(),
            base_color: [1.0; 4],
            base_color_texture: texture.map(|texture| StuntboostTextureRef {
                texture,
                mag_filter: Some(9728),
                min_filter: Some(9984),
                texcoord: 0,
            }),
            metallic: 0.0,
            roughness: 0.5,
            emissive: [0.0; 3],
            double_sided: true,
            alpha_mode: "OPAQUE".into(),
            alpha_cutoff: None,
            other: BTreeMap::new(),
        },
    );
    e
}

/// A one-triangle mesh prototype with `material`.
fn prototype(w: &mut World, material: Entity) -> Entity {
    let e = w.spawn();
    let mut streams = BTreeMap::new();
    streams.insert(
        Semantic::Position,
        Stream::F32x3(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]),
    );
    w.insert(
        e,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams,
            indices: Some(vec![0, 1, 2]),
            material: Some(material),
        }]),
    );
    e
}

fn node(w: &mut World, part: Entity, name: &str, prototype: Option<Entity>) -> Entity {
    let e = w.spawn();
    w.insert(e, Transform::IDENTITY);
    w.insert(
        e,
        StuntboostNode {
            part,
            node: 0,
            name: parse(name),
            extras: None,
        },
    );
    if let Some(prototype) = prototype {
        w.insert(
            e,
            Instance {
                prototype,
                variant: 0,
            },
        );
    }
    e
}

struct Level {
    w: World,
    ramp: Entity,
    kill: Entity,
    wall: Entity,
    start: Entity,
    other: Entity,
    checkpoint: Entity,
    sun: Entity,
    ceiling: Entity,
    window: Entity,
    bake: Entity,
    glass: Entity,
}

fn level() -> Level {
    let mut w = World::new();
    let part = w.spawn();
    w.insert(
        part,
        StuntboostPart {
            level: "Test".into(),
            index: 0,
            role: StuntboostRole::Level,
            file: "Level_Test_0.xnb".into(),
        },
    );
    let wood = material(&mut w, "C_Ramp_Wood.001", None);
    let kill = material(&mut w, "C_Kill", None);
    let plain = material(&mut w, "C_Default", None);
    let atlas = w.spawn();
    let bake = material(&mut w, "bake_mat_0", Some(atlas));
    let glass = material(&mut w, "window_glass", None);
    let (p_wood, p_kill, p_plain, p_bake, p_glass) = (
        prototype(&mut w, wood),
        prototype(&mut w, kill),
        prototype(&mut w, plain),
        prototype(&mut w, bake),
        prototype(&mut w, glass),
    );
    Level {
        ramp: node(&mut w, part, "ramp_<", Some(p_wood)),
        kill: node(&mut w, part, "lava_<", Some(p_kill)),
        wall: node(&mut w, part, "Skatepark_<", Some(p_plain)),
        start: node(&mut w, part, "=Spawn(start).005", None),
        other: node(&mut w, part, "=Spawn(,22).Grind()", None),
        checkpoint: node(&mut w, part, "_#=Checkpoint(Mandatory).003", Some(p_plain)),
        sun: node(&mut w, part, "_=Sun()", None),
        ceiling: node(
            &mut w,
            part,
            "ceiling°=BackfaceShadowCaster().DynamicShadow( )",
            Some(p_bake),
        ),
        window: node(&mut w, part, "window_glass=Transparent(0.5)", Some(p_glass)),
        bake,
        glass,
        w,
    }
}

fn opts(pairs: &[(&str, &str)]) -> Options {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Solid nodes collide with their physics material's surface; C_Kill is left out; a trigger gets no collision.
#[test]
fn solid_nodes_collide_with_their_physics_surfaces() {
    let mut l = level();
    let o = opts(&[
        ("plain", "concrete"),
        ("rails", "metal"),
        ("kill", "leave-out"),
        ("unassigned", "C_Default"),
    ]);
    let pass = LowerStuntboostCollision::from_options(&o).unwrap();
    assert_eq!(pass.options(), o);
    let mut log = PassLog::default();
    pass.run(&mut l.w, &mut log).unwrap();
    let surfaces = l.w.resource::<Surfaces>().unwrap();
    let of = |e: Entity| -> Option<(String, Option<SurfaceClass>)> {
        let c = l.w.get::<Collision>(e)?;
        let s = &surfaces.0[usize::from(c.surfaces[0])];
        Some((s.name.clone(), s.class))
    };
    assert_eq!(of(l.ramp), Some(("C_Ramp_Wood".into(), Some(SurfaceClass::Wood))));
    assert_eq!(
        of(l.wall),
        Some(("C_Default".into(), Some(SurfaceClass::Concrete)))
    );
    assert_eq!(of(l.kill), None);
    assert_eq!(of(l.checkpoint), None);
    assert_eq!(log.counts.get("C_Kill triangles left out"), Some(&1));
}

/// The start spawn is a spawn marker, the other a travel marker or left out.
#[test]
fn the_stated_spawn_starts() {
    let mut l = level();
    let o = opts(&[("start", "start"), ("others", "travel")]);
    let pass = LowerStuntboostSpawns::from_options(&o).unwrap();
    assert_eq!(pass.options(), o);
    pass.run(&mut l.w, &mut PassLog::default()).unwrap();
    let kind = |w: &World, e| w.get::<Marker>(e).map(|m| m.kind.clone());
    assert_eq!(kind(&l.w, l.start).as_deref(), Some("spawn"));
    assert_eq!(kind(&l.w, l.other).as_deref(), Some("travel"));

    let mut l = level();
    let pass = LowerStuntboostSpawns {
        start: String::new(),
        others: OtherSpawns::LeaveOut,
    };
    pass.run(&mut l.w, &mut PassLog::default()).unwrap();
    assert_eq!(kind(&l.w, l.other).as_deref(), Some("spawn"));
    assert!(!l.w.is_alive(l.start));
    let missing = LowerStuntboostSpawns {
        start: "nowhere".into(),
        others: OtherSpawns::Travel,
    };
    assert!(missing.run(&mut level().w, &mut PassLog::default()).is_err());
}

/// The bake atlas lowers as base colour or emission; a transparent node's material blends at its alpha.
#[test]
fn the_baked_atlas_lowers_as_stated() {
    let mut l = level();
    let o = opts(&[("baked", "emission"), ("transparent", "blend")]);
    let pass = LowerStuntboostMaterials::from_options(&o).unwrap();
    assert_eq!(pass.options(), o);
    pass.run(&mut l.w, &mut PassLog::default()).unwrap();
    let bake = l.w.get::<PbrMaterial>(l.bake).unwrap();
    assert!(bake.base_color_texture.is_none() && bake.emissive_texture.is_some());
    assert_eq!(bake.base_color, [0.0, 0.0, 0.0, 1.0]);
    // the window's own material is unchanged; its placement takes a blended copy through a variant
    assert_eq!(
        l.w.get::<PbrMaterial>(l.glass).unwrap().alpha_mode,
        AlphaMode::Opaque
    );
    let i = l.w.get::<Instance>(l.window).unwrap();
    let copy = l.w.get::<MaterialVariants>(i.prototype).unwrap().0[0][&i.variant];
    let blended = l.w.get::<PbrMaterial>(copy).unwrap();
    assert_eq!(
        (blended.alpha_mode, blended.base_color[3]),
        (AlphaMode::Blend, 0.5)
    );

    let mut l = level();
    LowerStuntboostMaterials {
        baked: Baked::BaseColour,
        transparent: Transparent::Opaque,
    }
    .run(&mut l.w, &mut PassLog::default())
    .unwrap();
    let bake = l.w.get::<PbrMaterial>(l.bake).unwrap();
    assert!(bake.base_color_texture.is_some() && bake.emissive_texture.is_none());
    assert_eq!(
        l.w.get::<PbrMaterial>(l.glass).unwrap().alpha_mode,
        AlphaMode::Opaque
    );
}

/// Two nodes drawing one mesh and material, one of them `Transparent(0.5)`: the opaque node keeps the material, the
/// transparent one draws a blended copy (STUNTBOOST's glass shares the level's bake material).
#[test]
fn a_transparent_node_blends_a_copy_not_the_shared_material() {
    let mut w = World::new();
    let part = w.spawn();
    let atlas = w.spawn();
    let bake = material(&mut w, "bake_mat_0", Some(atlas));
    let mesh = prototype(&mut w, bake);
    let floor = node(&mut w, part, "bake_0_object", Some(mesh));
    let glass = node(&mut w, part, "window_glass=Transparent(0.5)", Some(mesh));
    let mut log = PassLog::default();
    LowerStuntboostMaterials {
        baked: Baked::BaseColour,
        transparent: Transparent::Blend,
    }
    .run(&mut w, &mut log)
    .unwrap();
    assert_eq!(w.get::<PbrMaterial>(bake).unwrap().alpha_mode, AlphaMode::Opaque);
    assert_eq!(w.get::<Instance>(floor).unwrap().variant, 0);
    let i = w.get::<Instance>(glass).unwrap();
    assert_ne!(i.variant, 0);
    let copy = w.get::<MaterialVariants>(mesh).unwrap().0[0][&i.variant];
    let blended = w.get::<PbrMaterial>(copy).unwrap();
    assert_eq!(
        (blended.alpha_mode, blended.base_color[3]),
        (AlphaMode::Blend, 0.5)
    );
    assert_eq!(
        log.counts
            .get("blended material copies made for transparent nodes"),
        Some(&1)
    );
}

/// The four lowerings in a pipeline through the pass manager with its checks on: every effect each has (collision
/// added, spawns made markers and others left out, blended copies and variants, placements taken off and nodes
/// despawned) is one it declares.
#[test]
fn the_lowerings_declare_their_effects_to_the_pass_manager() {
    use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
    let mut l = level();
    let manager = PassManager::new(PassRegistry::new()).with_pass_checks(true);
    let pipeline = Pipeline {
        passes: vec![
            Box::new(LowerStuntboostCollision {
                plain: SurfaceClass::Concrete,
                rails: SurfaceClass::Metal,
                kill: KillSurface::LeaveOut,
                unassigned: "C_Default".into(),
            }),
            Box::new(LowerStuntboostSpawns {
                start: "start".into(),
                others: OtherSpawns::LeaveOut,
            }),
            Box::new(LowerStuntboostMaterials {
                baked: Baked::BaseColour,
                transparent: Transparent::Blend,
            }),
            Box::new(OmitStuntboostHidden {
                invisible: true,
                shadow_only: true,
                triggers: true,
                classes: BTreeSet::from(["Sun".to_owned()]),
            }),
        ],
    };
    manager
        .run(&mut l.w, &pipeline, &mut RunReport::default())
        .unwrap();
    assert!(l.w.has::<Collision>(l.wall) && !l.w.has::<Instance>(l.wall));
    assert!(!l.w.is_alive(l.other) && l.w.has::<Marker>(l.start));
    assert_ne!(l.w.get::<Instance>(l.window).unwrap().variant, 0);
}

/// Triggers, the shadow-only ceiling and the stated classes go; a collision-only wall keeps its collision without
/// its mesh; the spawn marker and the window stay.
#[test]
fn hidden_and_gameplay_nodes_are_left_out_as_stated() {
    let mut l = level();
    LowerStuntboostCollision {
        plain: SurfaceClass::Concrete,
        rails: SurfaceClass::Metal,
        kill: KillSurface::LeaveOut,
        unassigned: "C_Default".into(),
    }
    .run(&mut l.w, &mut PassLog::default())
    .unwrap();
    LowerStuntboostSpawns {
        start: "start".into(),
        others: OtherSpawns::Travel,
    }
    .run(&mut l.w, &mut PassLog::default())
    .unwrap();
    let o = opts(&[
        ("invisible", "leave-out"),
        ("shadow-only", "leave-out"),
        ("triggers", "leave-out"),
        ("classes", "Sun"),
    ]);
    let pass = OmitStuntboostHidden::from_options(&o).unwrap();
    assert_eq!(pass.options(), o);
    assert_eq!(pass.classes, BTreeSet::from(["Sun".to_owned()]));
    pass.run(&mut l.w, &mut PassLog::default()).unwrap();
    assert!(!l.w.is_alive(l.checkpoint) && !l.w.is_alive(l.ceiling) && !l.w.is_alive(l.sun));
    assert!(l.w.is_alive(l.wall) && l.w.has::<Collision>(l.wall) && !l.w.has::<Instance>(l.wall));
    assert!(l.w.is_alive(l.start) && l.w.is_alive(l.window) && l.w.has::<Instance>(l.ramp));
    assert!(OmitStuntboostHidden::from_options(&Options::new()).is_err());
}
