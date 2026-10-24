//! The lowerings on a synthetic level: a solid wooden ramp, a kill floor, a collision-only wall, two spawns, a
//! checkpoint trigger, the sun, a shadow-only ceiling, a transparent window and a bake material.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{AlphaMode, Filter, Filters, MipFilter, PbrMaterial, Shading, Wrap};
use primase_core::{
    Collision, Entity, Instance, Marker, MaterialVariants, Mesh, Primitive, Semantic, Stream, SurfaceClass,
    Surfaces, Topology, Transform, World,
};

use super::*;
use crate::components::names::parse;
use crate::components::{
    StuntboostMaterial, StuntboostNode, StuntboostPart, StuntboostRole, StuntboostTextureRef,
};

fn material(w: &mut World, name: &str, texture: Option<Entity>) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        StuntboostMaterial {
            name: name.into(),
            base_color: [1.0; 4],
            base_color_texture: texture.map(|texture| StuntboostTextureRef {
                texture,
                filters: Filters {
                    mag: Some(Filter::Nearest),
                    min: Some(Filter::Nearest),
                    mip: Some(MipFilter::Linear),
                },
                wrap: [Wrap::Repeat; 2],
                texcoord: 0,
            }),
            metallic: 0.0,
            roughness: 0.5,
            emissive: [0.0; 3],
            double_sided: true,
            alpha_mode: "OPAQUE".into(),
            alpha_cutoff: None,
            shading: Shading::Lit,
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
        ramp: node(&mut w, part, "ramp<", Some(p_wood)),
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
    let pass = LOWER_STUNTBOOST_COLLISION.instance(&o).unwrap();
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
    let pass = LOWER_STUNTBOOST_SPAWNS.instance(&o).unwrap();
    assert_eq!(pass.options(), o);
    pass.run(&mut l.w, &mut PassLog::default()).unwrap();
    let kind = |w: &World, e| w.get::<Marker>(e).map(|m| m.kind.clone());
    assert_eq!(kind(&l.w, l.start).as_deref(), Some("spawn"));
    assert_eq!(kind(&l.w, l.other).as_deref(), Some("travel"));

    let mut l = level();
    let pass = LowerStuntboostSpawns {
        start: Start::Named(String::new()),
        others: OtherSpawns::LeaveOut,
    };
    pass.run(&mut l.w, &mut PassLog::default()).unwrap();
    assert_eq!(kind(&l.w, l.other).as_deref(), Some("spawn"));
    assert!(!l.w.is_alive(l.start));
    let missing = LowerStuntboostSpawns {
        start: Start::Named("nowhere".into()),
        others: OtherSpawns::Travel,
    };
    assert!(missing.run(&mut level().w, &mut PassLog::default()).is_err());
}

/// `start=first` starts the first spawn in stored order, `start=none` starts none, `name:` names a spawn called
/// `first`; the spawn listing is that order; a level without a spawn has no first.
#[test]
fn the_start_is_the_first_spawn_or_none() {
    let kind = |w: &World, e| w.get::<Marker>(e).map(|m| m.kind.clone());
    let mut l = level();
    let listed = spawns(&l.w);
    assert_eq!(
        listed
            .iter()
            .map(|s| (s.entity, s.name.as_str()))
            .collect::<Vec<_>>(),
        [(l.start, "start"), (l.other, "")]
    );
    let first = opts(&[("start", "first"), ("others", "travel")]);
    // printed back as the word, not as a spawn's name (name:first)
    let pass = LOWER_STUNTBOOST_SPAWNS.instance(&first).unwrap();
    assert_eq!(pass.options(), first);
    pass.run(&mut l.w, &mut PassLog::default()).unwrap();
    assert_eq!(kind(&l.w, l.start).as_deref(), Some("spawn"));
    assert_eq!(kind(&l.w, l.other).as_deref(), Some("travel"));

    let mut l = level();
    let none = opts(&[("start", "none"), ("others", "leave-out")]);
    let pass = LOWER_STUNTBOOST_SPAWNS.instance(&none).unwrap();
    assert_eq!(pass.options(), none);
    let mut log = PassLog::default();
    pass.run(&mut l.w, &mut log).unwrap();
    assert!(!l.w.is_alive(l.start) && !l.w.is_alive(l.other));
    assert_eq!(log.counts.get("spawns other than the start left out"), Some(&2));

    // the words are reserved; a spawn of that name is written with its prefix
    let named = LOWER_STUNTBOOST_SPAWNS
        .instance(&opts(&[("start", "name:first"), ("others", "travel")]))
        .unwrap();
    let typed = LowerStuntboostSpawns {
        start: Start::Named("first".into()),
        others: OtherSpawns::Travel,
    };
    assert_eq!(named.options(), typed.options());
    assert_eq!(named.options()["start"], "name:first");
    assert!(named.run(&mut level().w, &mut PassLog::default()).is_err());

    let mut no_spawns = World::new();
    let err = LowerStuntboostSpawns {
        start: Start::First,
        others: OtherSpawns::Travel,
    }
    .run(&mut no_spawns, &mut PassLog::default())
    .unwrap_err();
    assert!(err.contains("no spawn"), "{err}");
}

/// The bake atlas lowers as base colour or emission; a transparent node's material blends at its alpha.
#[test]
fn the_baked_atlas_lowers_as_stated() {
    let mut l = level();
    let o = opts(&[("baked", "emission"), ("transparent", "blend")]);
    let pass = LOWER_STUNTBOOST_MATERIALS.instance(&o).unwrap();
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

    // unlit: the atlas as the base colour of an unlit material, its nearest filters kept
    let mut l = level();
    LowerStuntboostMaterials {
        baked: Baked::Unlit,
        transparent: Transparent::Opaque,
    }
    .run(&mut l.w, &mut PassLog::default())
    .unwrap();
    let bake = l.w.get::<PbrMaterial>(l.bake).unwrap();
    assert_eq!(bake.shading, Shading::Unlit);
    let atlas = bake.base_color_texture.as_ref().unwrap();
    assert_eq!(atlas.filters.mag, Some(Filter::Nearest));
    assert_eq!(l.w.get::<PbrMaterial>(l.glass).unwrap().shading, Shading::Lit);

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

/// `hang_sack=Transparent(1.5)` (18 STUNTBOOST levels) draws with the room's bake material, which the game brightens by
/// 1.5 and draws opaque. Lowered as emission, the sack's copy carries the factor in its emission strength, opaque; the
/// shared material is unchanged.
#[test]
fn an_alpha_above_one_brightens_an_emission_copy_opaque() {
    let mut w = World::new();
    let part = w.spawn();
    let atlas = w.spawn();
    let bake = material(&mut w, "bake_mat_0", Some(atlas));
    let mesh = prototype(&mut w, bake);
    let floor = node(&mut w, part, "bake_0_object", Some(mesh));
    let sack = node(&mut w, part, "hang_sack=Transparent(1.5).001", Some(mesh));
    let mut log = PassLog::default();
    LowerStuntboostMaterials {
        baked: Baked::Emission,
        transparent: Transparent::Blend,
    }
    .run(&mut w, &mut log)
    .unwrap();
    let shared = w.get::<PbrMaterial>(bake).unwrap().clone();
    assert_eq!(shared.emissive_strength, 1.0);
    assert_eq!(w.get::<Instance>(floor).unwrap().variant, 0);
    let i = w.get::<Instance>(sack).unwrap();
    assert_ne!(i.variant, 0);
    let copy = w.get::<MaterialVariants>(mesh).unwrap().0[0][&i.variant];
    let brightened = w.get::<PbrMaterial>(copy).unwrap();
    assert_eq!(brightened.alpha_mode, AlphaMode::Opaque);
    assert_eq!(brightened.base_color, shared.base_color);
    assert_eq!(brightened.emissive_strength, 1.5);
    assert_eq!(brightened.emissive_texture, shared.emissive_texture);
    assert_eq!(
        log.counts
            .get("brightened emission copies made for transparent nodes with an alpha above 1"),
        Some(&1)
    );
    assert!(log.approximations.is_empty(), "{:?}", log.approximations);
}

/// The same sack with the atlas as base colour: a base colour holds at most 1, so the node draws opaque with the
/// shared material and the factor it does not carry is an approximation on the node. Through the pass manager with its
/// checks on, every material verifies (this was the lab's `base colour [1.0, 1.0, 1.0, 1.5] is not within 0-1`).
#[test]
fn an_alpha_above_one_on_a_base_colour_is_counted_not_clamped() {
    use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
    let mut w = World::new();
    let part = w.spawn();
    let atlas = w.spawn();
    let bake = material(&mut w, "bake_mat_0", Some(atlas));
    let mesh = prototype(&mut w, bake);
    let sack = node(&mut w, part, "hang_sack=Transparent(1.5).001", Some(mesh));
    let glass = node(&mut w, part, "window_glass=Transparent(0.5)", Some(mesh));
    let manager = PassManager::new(PassRegistry::new()).with_pass_checks(true);
    let pipeline = Pipeline {
        passes: vec![Box::new(LowerStuntboostMaterials {
            baked: Baked::BaseColour,
            transparent: Transparent::Blend,
        })],
    };
    let mut report = RunReport::default();
    manager.run(&mut w, &pipeline, &mut report).unwrap();
    assert_eq!(w.get::<Instance>(sack).unwrap().variant, 0);
    assert_ne!(w.get::<Instance>(glass).unwrap().variant, 0);
    for (_, m) in w.iter::<PbrMaterial>() {
        assert!(
            m.base_color.iter().all(|c| (0.0..=1.0).contains(c)),
            "{:?}",
            m.base_color
        );
    }
    let log = &report.steps[0].log;
    let reason = materials::BRIGHTER_NOT_CARRIED;
    assert_eq!(log.approximations.get(reason), Some(&1));
    assert_eq!(log.approximated_on.get(reason), Some(&BTreeSet::from([sack])));
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
                start: Start::Named("start".into()),
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
        start: Start::Named("start".into()),
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
    let pass = OMIT_STUNTBOOST_HIDDEN.instance(&o).unwrap();
    assert_eq!(pass.options(), o);
    pass.run(&mut l.w, &mut PassLog::default()).unwrap();
    assert!(!l.w.is_alive(l.checkpoint) && !l.w.is_alive(l.ceiling) && !l.w.is_alive(l.sun));
    assert!(l.w.is_alive(l.wall) && l.w.has::<Collision>(l.wall) && !l.w.has::<Instance>(l.wall));
    assert!(l.w.is_alive(l.start) && l.w.is_alive(l.window) && l.w.has::<Instance>(l.ramp));
    assert!(OMIT_STUNTBOOST_HIDDEN.instance(&Options::new()).is_err());
}
