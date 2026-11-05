//! Behaviour of the effects lowering on small effects built by hand (each action's lists as short as the layout allows, the
//! rest at the project's defaults), and of the pass on a world holding them. The real-data census (every installed effect
//! lowered, accounting checked) is in `tests/` of the crate.
use super::*;
use crate::sekiro::components::effect::{ClipEffectUses, Effect, EffectFile, EffectUse};
use crate::sekiro::source::EffectPlace;
use fromsoft_formats::Game;
use fromsoft_formats::fxr::{Action, Config, Fxr, Node, Property, State};
use primase_core::particles::{Anchor, Blend, Drive, Follow, Heading, Shape, Spawn, Spread, Termination};
use primase_core::{Clip, ClipEffects, Component, Entity, Extrapolation, Transform, World};
use std::sync::Arc;

fn f(v: f32) -> u32 {
    v.to_bits()
}

fn constant(v: f32) -> Property {
    Property::constant_of(&[v])
}

fn action(id: i16) -> Action {
    Action {
        id,
        unknown_02: 0,
        unknown_03: 1,
        unknown_04: 0,
        fields1: Vec::new(),
        fields2: Vec::new(),
        properties1: Vec::new(),
        properties2: Vec::new(),
        field_lists: Vec::new(),
    }
}

fn with(mut a: Action, properties1: Vec<Property>, fields1: Vec<u32>) -> Action {
    a.properties1 = properties1;
    a.fields1 = fields1;
    a
}

/// The state-config map of a node: the config each state runs.
fn map(configs: &[u32]) -> Action {
    let mut a = action(199);
    a.field_lists = vec![configs.to_vec()];
    a
}

fn node(id: i16, actions: Vec<Action>, configs: Vec<Config>, children: Vec<Node>) -> Node {
    Node {
        id,
        actions,
        configs,
        children,
    }
}

fn fx(id: i32, states: usize, root: Node) -> Fxr {
    Fxr {
        id,
        states: (0..states)
            .map(|_| State {
                conditions: Vec::new(),
            })
            .collect(),
        root,
        references: Vec::new(),
        external_values: Vec::new(),
        blood_enablers: Vec::new(),
    }
}

/// A basic config that emits two particles every quarter second from a point, heading up, spread over a 20 degree cone, each
/// living two seconds and drawn as an additive point sprite of texture 5.
fn torch() -> Config {
    Config {
        id: 1004,
        actions: vec![
            with(action(128), vec![constant(5.0)], vec![f(0.5), 0, 1, f(0.0)]),
            with(action(129), vec![constant(2.0)], vec![1]),
            with(
                action(300),
                vec![constant(0.25), constant(2.0), constant(8.0), constant(-1.0)],
                vec![1],
            ),
            with(action(400), vec![], vec![1]),
            with(action(501), vec![constant(20.0), constant(0.0)], vec![]),
            with(
                action(55),
                vec![constant(9.8), constant(1.0), constant(1.0)],
                vec![0, f(0.0)],
            ),
            with(
                action(600),
                vec![constant(5.0), constant(4.0), constant(0.3)],
                vec![],
            ),
        ],
    }
}

fn simple() -> Fxr {
    fx(
        300,
        1,
        node(
            2000,
            vec![],
            vec![],
            vec![node(2200, vec![map(&[0])], vec![torch()], vec![])],
        ),
    )
}

#[test]
fn a_basic_node_becomes_one_emitter_with_its_slots_lowered() {
    let l = lower(&simple(), Game::Sekiro, 1, &mut NoFiles).unwrap();
    let s = &l.system;
    assert_eq!(s.emitters.len(), 1);
    assert_eq!(s.states, 1);
    let e = &s.emitters[0];
    assert_eq!(e.parent, None);
    assert!(e.states.is_empty());
    assert_eq!(e.delay, 0.5);
    assert_eq!(e.duration, Some(5.0));
    match &e.spawn {
        Spawn::Periodic {
            interval,
            count,
            total,
            max_alive,
        } => {
            assert_eq!(interval.curve.value_at(0.0)[0], 0.25);
            assert_eq!(count.curve.value_at(0.0)[0], 2.0);
            assert_eq!((*total, *max_alive), (Some(8), None));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(e.shape, Shape::Point);
    assert_eq!(e.heading, Heading::World([0.0, 1.0, 0.0]));
    match &e.spread {
        Spread::Cone { angle, .. } => assert_eq!(angle.curve.value_at(0.0)[0], 20.0),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        e.law.lifetime.as_ref().map(|v| v.curve.value_at(0.0)[0]),
        Some(2.0)
    );
    let m = e.law.movement.as_ref().unwrap();
    assert!(matches!(m.drive, Drive::Acceleration { .. }));
    assert_eq!(m.gravity.curve.value_at(0.0)[0], 9.8);
    match &e.draw {
        Some(primase_core::particles::Draw::Point(p)) => assert_eq!(p.blend, Blend::Additive),
        other => panic!("{other:?}"),
    }
    // no file was found for texture 5: counted
    assert_eq!(
        l.notes.lost.get("a texture that is not in the effect binders"),
        Some(&1)
    );
    // the unknown action 130 is in no hand-built config; a point sprite's colour factors that are 1 are left out
    assert_eq!(
        s.lost.iter().map(|(_, n)| *n).sum::<u32>(),
        l.notes.lost.values().sum::<u32>()
    );
    // the system the lowering made is one the world accepts
    let mut world = World::new();
    let e = world.spawn();
    assert!(s.verify(e, &world).is_ok());
}

#[test]
fn a_distortion_sheet_blends_by_its_alpha_whatever_its_files_blend_mode() {
    for (mode, approximated) in [(0.0, Some(&1)), (2.0, None), (4.0, Some(&1))] {
        let mut config = torch();
        config.actions.retain(|a| a.id != 600);
        config
            .actions
            .push(with(action(607), vec![constant(mode)], vec![]));
        let effect = fx(
            300,
            1,
            node(
                2000,
                vec![],
                vec![],
                vec![node(2200, vec![map(&[0])], vec![config], vec![])],
            ),
        );
        let l = lower(&effect, Game::EldenRing, 1, &mut NoFiles).unwrap();
        match &l.system.emitters[0].draw {
            Some(primase_core::particles::Draw::Refraction(r)) => {
                assert_eq!(r.blend, Blend::Alpha, "mode {mode}")
            }
            other => panic!("mode {mode}: {other:?}"),
        }
        assert_eq!(
            l.notes.approximated.get(
                "a distortion's blend mode as normal (the game's distortion draws blend by their alpha; that the file's mode is unused is not proven)"
            ),
            approximated,
            "mode {mode}"
        );
    }
}

/// The normal map of a distortion scrolls where the game's program moves it: `GXFfxdistortionBump` adds its first scroll
/// distance to the sheet's u and takes its second from v, so core's offset and scroll (added to both) carry the file's V
/// negated and its U as stored.
#[test]
fn a_distortion_normal_map_moves_back_along_v_as_the_games_program_moves_it() {
    let mut config = torch();
    config.actions.retain(|a| a.id != 600);
    // blendMode, offset xyz, size xyz, color and the four-component property after it, intensity, the property after it,
    // stirSpeed, radius, then normalMapOffsetU and V and normalMapSpeedU and V
    let mut properties = vec![constant(2.0)];
    properties.extend((1..7).map(|_| constant(1.0)));
    properties.extend((7..9).map(|_| Property::constant_of(&[1.0; 4])));
    properties.extend((9..13).map(|_| constant(1.0)));
    properties.extend([constant(0.2), constant(0.3), constant(0.1), constant(0.25)]);
    config.actions.push(with(action(607), properties, vec![]));
    let effect = fx(
        300,
        1,
        node(
            2000,
            vec![],
            vec![],
            vec![node(2200, vec![map(&[0])], vec![config], vec![])],
        ),
    );
    let l = lower(&effect, Game::EldenRing, 1, &mut NoFiles).unwrap();
    let Some(primase_core::particles::Draw::Refraction(r)) = &l.system.emitters[0].draw else {
        panic!("{:?}", l.system.emitters[0].draw);
    };
    let at = |v: &primase_core::particles::Value| v.curve.value_at(0.0)[0];
    assert_eq!(r.normal_offset.each_ref().map(at), [0.2, -0.3]);
    assert_eq!(r.normal_scroll.each_ref().map(at), [0.1, -0.25]);
    assert_eq!(
        l.notes.approximated.get(
            "a distortion's normal-map offset and speed as its program's scroll distances (u forward, v back; which file field feeds which is not read)"
        ),
        Some(&1)
    );
}

#[test]
fn a_node_with_two_configs_is_two_emitters_by_state() {
    let second = Config {
        id: 1004,
        actions: vec![
            with(action(128), vec![constant(-1.0)], vec![f(0.0), 0, 1, f(0.0)]),
            with(action(399), vec![], vec![]),
        ],
    };
    let root = node(
        2000,
        vec![],
        vec![],
        vec![node(2200, vec![map(&[0, 1])], vec![torch(), second], vec![])],
    );
    let l = lower(&fx(1, 2, root), Game::Sekiro, 0, &mut NoFiles).unwrap();
    assert_eq!(l.system.states, 2);
    assert_eq!(l.system.emitters.len(), 2);
    assert_eq!(l.system.emitters[0].states, vec![0]);
    assert_eq!(l.system.emitters[1].states, vec![1]);
    assert_eq!(l.system.emitters[1].duration, None);
    assert!(matches!(l.system.emitters[1].spawn, Spawn::Once));
}

#[test]
fn children_hang_from_their_parents_emitter_and_a_proxy_names_the_spawned_effect() {
    struct One(Entity);
    impl Resolver for One {
        fn texture(&mut self, _: fromsoft_formats::fxr::resources::TextureKind, _: i32) -> Option<Entity> {
            None
        }
        fn model(&mut self, _: i32) -> Option<(Entity, super::models::Look)> {
            None
        }
        fn animation(&mut self, _: i32) -> Option<Entity> {
            None
        }
        fn effect(&mut self, id: i32) -> Option<Entity> {
            (id == 77).then_some(self.0)
        }
    }
    let mut reference = action(132);
    reference.fields1 = vec![77];
    let proxy = node(2001, vec![reference], vec![], vec![]);
    let basic = node(2200, vec![map(&[0])], vec![torch()], vec![proxy]);
    let root = node(2000, vec![], vec![], vec![basic]);
    let target = Entity::from_index(9);
    let l = lower(&fx(2, 1, root), Game::EldenRing, 0, &mut One(target)).unwrap();
    assert_eq!(l.system.emitters.len(), 2);
    assert_eq!(l.system.emitters[1].parent, Some(0));
    assert_eq!(l.system.emitters[1].spawns, vec![target]);
    assert!(l.system.emitters[1].draw.is_none());
    // a reference to an effect the resolver does not know is counted
    let mut reference = action(132);
    reference.fields1 = vec![78];
    let root = node(
        2000,
        vec![],
        vec![],
        vec![node(2001, vec![reference], vec![], vec![])],
    );
    let l = lower(&fx(3, 1, root), Game::Sekiro, 0, &mut One(target)).unwrap();
    assert_eq!(
        l.notes
            .lost
            .get("a spawned effect that is not in the effect binders or closes a cycle"),
        Some(&1)
    );
    assert!(l.system.emitters[0].spawns.is_empty());
}

#[test]
fn the_root_sets_the_systems_time_view_distance_and_termination() {
    let mut rate = action(10500);
    rate.properties1 = vec![constant(0.5)];
    // limit view distance, 40 m, initial simulation time 45 frames at 30 per second
    rate.fields1 = vec![1, f(40.0), 0, 0, 0, 0, 0, 45];
    let mut fade = action(701);
    fade.properties1 = vec![constant(1.5)];
    let root = node(2000, vec![rate, fade], vec![], vec![]);
    let l = lower(&fx(4, 1, root), Game::EldenRing, 0, &mut NoFiles).unwrap();
    let s = l.system;
    assert_eq!(s.time_scale.as_ref().map(|v| v.curve.value_at(0.0)[0]), Some(0.5));
    assert_eq!(s.max_view_distance, Some(40.0));
    assert_eq!(s.warmup, 1.5);
    assert_eq!(s.termination, Termination::Fade(1.5));
}

#[test]
fn a_static_node_transform_places_the_emitter_in_cores_frame() {
    let mut place = action(35);
    place.fields1 = vec![f(1.0), f(2.0), f(3.0), f(0.0), f(0.0), f(0.0)];
    let cfg = Config {
        id: 1004,
        actions: vec![place, with(action(399), vec![], vec![])],
    };
    let root = node(
        2000,
        vec![],
        vec![],
        vec![node(2200, vec![map(&[0])], vec![cfg], vec![])],
    );
    let l = lower(&fx(5, 1, root), Game::Sekiro, 0, &mut NoFiles).unwrap();
    let t = l.system.emitters[0].placement;
    assert_eq!(t.cols[3], [1.0, 2.0, -3.0]);
    let _ = (Anchor::Parent, Follow::Frame);
}

/// A world with the effect and a place for it.
fn world_with(effect: Fxr, starts_disabled: bool) -> (World, Entity) {
    let mut world = World::new();
    let file = world.spawn();
    world.insert(
        file,
        EffectFile {
            effect: effect.id,
            game: Game::Sekiro,
            file: Arc::new(effect.clone()),
        },
    );
    let place = world.spawn();
    world.insert(place, Transform::IDENTITY);
    world.insert(
        place,
        Effect {
            effect: effect.id,
            place: EffectPlace::Region { region: "r".into() },
            starts_disabled,
        },
    );
    (world, place)
}

fn pass() -> LowerFromSoftEffects {
    LowerFromSoftEffects::new(Factor::new(2.0).unwrap())
}

#[test]
fn the_pass_puts_a_system_on_each_placed_effect_and_logs_what_it_lost() {
    let (mut world, place) = world_with(simple(), false);
    let mut log = PassLog::default();
    pass().run(&mut world, &mut log).unwrap();
    let system = world.get::<ParticleSystem>(place).unwrap();
    assert_eq!(system.emitters.len(), 1);
    assert_eq!(log.counts.get("placed effects lowered"), Some(&1));
    assert_eq!(log.counts.get("effects lowered"), Some(&1));
    assert!(
        log.omitted
            .contains_key("a texture that is not in the effect binders")
    );
    assert!(system.verify(place, &world).is_ok());
}

#[test]
fn the_pass_leaves_out_regions_that_start_disabled_unless_asked() {
    let (mut world, place) = world_with(simple(), true);
    let mut log = PassLog::default();
    pass().run(&mut world, &mut log).unwrap();
    assert!(world.get::<ParticleSystem>(place).is_none());
    assert_eq!(
        log.omitted
            .get("effects of regions that start disabled, left out"),
        Some(&1)
    );
    let mut log = PassLog::default();
    pass()
        .with_include_disabled(Disabled::Include)
        .run(&mut world, &mut log)
        .unwrap();
    assert!(world.get::<ParticleSystem>(place).is_some());
}

#[test]
fn a_spawn_cycle_is_cut_where_it_closes_and_a_spawned_effect_gets_a_system_of_its_own() {
    let mut a = action(132);
    a.fields1 = vec![2];
    let mut b = action(132);
    b.fields1 = vec![1];
    let mut one = fx(
        1,
        1,
        node(2000, vec![], vec![], vec![node(2001, vec![a], vec![], vec![])]),
    );
    one.references = vec![2];
    let mut two = fx(
        2,
        1,
        node(2000, vec![], vec![], vec![node(2001, vec![b], vec![], vec![])]),
    );
    two.references = vec![1];
    let mut world = World::new();
    for e in [one, two] {
        let file = world.spawn();
        world.insert(
            file,
            EffectFile {
                effect: e.id,
                game: Game::Sekiro,
                file: Arc::new(e),
            },
        );
    }
    let place = world.spawn();
    world.insert(place, Transform::IDENTITY);
    world.insert(
        place,
        Effect {
            effect: 1,
            place: EffectPlace::Region { region: "r".into() },
            starts_disabled: false,
        },
    );
    let mut log = PassLog::default();
    pass().run(&mut world, &mut log).unwrap();
    // effect 1 names effect 2's system; effect 2 may not name effect 1's (it can reach it): counted
    let s = world.get::<ParticleSystem>(place).unwrap();
    assert_eq!(s.emitters[0].spawns.len(), 1);
    assert_eq!(log.counts.get("effects lowered"), Some(&2));
    assert!(
        log.omitted
            .contains_key("a spawned effect that is not in the effect binders or closes a cycle")
    );
    // the named system is one of the world's, with its own emitters
    let named = s.emitters[0].spawns[0];
    assert!(world.get::<ParticleSystem>(named).is_some());
    let mut refs = Vec::new();
    s.references(&mut refs);
    assert_eq!(refs, vec![named]);
    assert_eq!(ParticleSystem::NAME, "core.particles");
}

fn use_of(
    event: u32,
    effect: i32,
    dummy: Option<i32>,
    follow: bool,
    repeating: bool,
    start: f32,
    end: Option<f32>,
) -> EffectUse {
    EffectUse {
        event,
        kind: 96,
        effect,
        route: crate::sekiro::effect_data::UseRoute::Event,
        dummy,
        span: Vec::new(),
        source: None,
        follow,
        repeating,
        start,
        end,
    }
}

#[test]
fn a_clips_effect_uses_become_spawns_of_the_lowered_systems_at_the_sockets_named_as_the_model_names_them() {
    let mut world = World::new();
    let file = world.spawn();
    world.insert(
        file,
        EffectFile {
            effect: 300,
            game: Game::Sekiro,
            file: Arc::new(simple()),
        },
    );
    let clip = world.spawn();
    world.insert(
        clip,
        Clip {
            name: "a000_003000".into(),
            duration: 2.0,
            extrapolation: Extrapolation::Hold,
            channels: Vec::new(),
            root_motion: None,
            additive: None,
        },
    );
    world.insert(
        clip,
        ClipEffectUses {
            uses: vec![
                use_of(4, 300, Some(220), true, false, 0.5, Some(0.6)),
                use_of(7, 300, None, true, true, 1.0, Some(1.5)),
                use_of(9, 301, Some(10), false, false, 1.2, None),
            ],
            lost: vec![(
                "an effect a goods or magic param row names (events 114 and 115)".to_owned(),
                2,
            )],
            read: Vec::new(),
        },
    );
    // the clip's source events: event 5 ends before it starts and has no core.clip_events entry, so source event 7 is core's
    // event 6
    let events: Vec<fromsoft_formats::tae::Event> = (0..10)
        .map(|i| fromsoft_formats::tae::Event {
            kind: 96,
            unk04: 0,
            start: 0.1 * i as f32,
            end: if i == 5 { 0.0 } else { 0.1 * i as f32 + 0.1 },
            parameters: Vec::new(),
        })
        .collect();
    world.insert(
        clip,
        crate::sekiro::character_data::ClipEvents {
            file: "c0000.tae".into(),
            animation: 3000,
            header: fromsoft_formats::tae::Header::None,
            file_name: None,
            authored_events: events.clone(),
            typed: vec![crate::sekiro::character_data::DecodedEvent::default(); events.len()],
            events,
        },
    );
    let mut log = PassLog::default();
    pass().run(&mut world, &mut log).unwrap();
    let effects = world.get::<ClipEffects>(clip).unwrap();
    assert_eq!(effects.verify(clip, &world), Ok(()));
    assert_eq!(effects.spawns.len(), 2);
    let (first, second) = (&effects.spawns[0], &effects.spawns[1]);
    // both spawn the one system effect 300 was lowered to, which no transform places
    assert_eq!(first.system, second.system);
    assert!(world.get::<ParticleSystem>(first.system).is_some());
    assert!(world.get::<Transform>(first.system).is_none());
    // a use with a dummy stands at that dummy's socket and follows it where the event says; one with none stands at the origin
    assert_eq!(
        (first.socket.as_deref(), first.follow, first.event),
        (Some("dummy 220"), true, Some(4))
    );
    assert_eq!((first.start, first.end), (0.5, None));
    assert_eq!(
        (second.socket.as_deref(), second.follow, second.event),
        (None, false, Some(6))
    );
    // a repeating event stops the effect at its end; a one-shot one lets it play out
    assert_eq!(second.end, Some(1.5));
    // the effect the read did not find, and what the read could not name, are counted
    assert_eq!(
        log.omitted
            .get("animation events naming an effect the read did not find"),
        Some(&1)
    );
    assert_eq!(
        log.omitted
            .get("an effect a goods or magic param row names (events 114 and 115)"),
        Some(&2)
    );
    assert_eq!(log.counts.get("effects started by animation events"), Some(&2));
    assert_eq!(effects.lost.len(), 2);
}

#[test]
fn effect_uses_on_something_that_is_not_a_lowered_clip_are_counted_and_make_nothing() {
    let mut world = World::new();
    let file = world.spawn();
    world.insert(
        file,
        EffectFile {
            effect: 300,
            game: Game::Sekiro,
            file: Arc::new(simple()),
        },
    );
    let not_a_clip = world.spawn();
    world.insert(
        not_a_clip,
        ClipEffectUses {
            uses: vec![use_of(0, 300, Some(1), false, false, 0.0, None)],
            lost: Vec::new(),
            read: Vec::new(),
        },
    );
    let mut log = PassLog::default();
    pass().run(&mut world, &mut log).unwrap();
    assert!(world.get::<ClipEffects>(not_a_clip).is_none());
    assert_eq!(
        log.omitted.get("effect uses on a clip that is not lowered"),
        Some(&1)
    );
}

/// A one-mesh model of three vertices with a vertex colour, its material binding `texture` (a path, empty for none).
fn effect_model(texture: &str, cull: bool) -> fromsoft_formats::flver::Model {
    use fromsoft_formats::flver::{FaceSet, Material, Member, Mesh as SourceMesh, Model, Texture, Vertex};
    Model {
        bones: Vec::new(),
        dummies: Vec::new(),
        version: 0x2001a,
        speedtree: false,
        layouts: vec![vec![Member {
            unknown: 0,
            stored: true,
            offset: 0,
            kind: 0x1a,
            semantic: 10,
            index: 0,
        }]],
        materials: vec![Material {
            name: "s1".into(),
            mtd: r"N:\GR\data\Material\mtd\sfx\matxml\S[A]_add_GXFlver.matxml".into(),
            textures: vec![Texture {
                path: texture.into(),
                kind: "g_DiffuseTexture".into(),
                ..Texture::default()
            }],
            string_bytes: 0,
            index: 0,
            gx: Vec::new(),
        }],
        meshes: vec![SourceMesh {
            dynamic: false,
            material: 0,
            node: -1,
            palette: Vec::new(),
            layouts: vec![0],
            vertices: (0..3)
                .map(|i| Vertex {
                    position: [i as f32, (i == 1) as u8 as f32, 0.0],
                    normal: [0.0, 1.0, 0.0],
                    colors: [[0.5, 0.25, i as f32, 1.0]].into(),
                    ..Vertex::default()
                })
                .collect(),
            faces: vec![FaceSet {
                flags: 0,
                strip: false,
                cull,
                unknown_06: 0,
                indices: vec![0, 1, 2],
            }],
        }],
    }
}

/// A world holding `simple()` placed, the effect texture 34031 (absent: what matters is the entity) and the models `models`.
fn world_with_models(models: Vec<(i32, fromsoft_formats::flver::Model)>) -> (World, Entity, Vec<Entity>) {
    use crate::sekiro::components::effect::{EffectModel, EffectResource, ResourceKind};
    let (mut world, _) = world_with(simple(), false);
    let texture = world.spawn();
    world.insert(
        texture,
        EffectResource {
            kind: ResourceKind::Albedo,
            id: 34031,
        },
    );
    world.insert(
        texture,
        primase_core::TextureAbsent {
            reason: "a stand-in".into(),
        },
    );
    let entities = models
        .into_iter()
        .map(|(id, model)| {
            let e = world.spawn();
            world.insert(
                e,
                EffectResource {
                    kind: ResourceKind::Model,
                    id,
                },
            );
            world.insert(e, EffectModel::new(fromsoft_formats::Game::EldenRing, model));
            e
        })
        .collect();
    (world, texture, entities)
}

/// A mesh particle's model is the `core.mesh` of its entity: its level-0 face set with the vertex colour the material
/// binary's program reads, a material of its own whose colour texture is the effect texture the binding's `_a` path names,
/// double-sided where the face set draws back faces. A material whose definition is not held is drawn unlit and blended,
/// counted; one that binds no file draws in the particles' colour alone, counted.
#[test]
fn an_effect_model_becomes_a_mesh_with_an_unlit_material_on_its_colour_texture() {
    use primase_core::pbr::{AlphaMode, PbrMaterial, Shading};
    use primase_core::{Mesh, Semantic, Stream};
    let (mut world, texture, models) = world_with_models(vec![
        (83050, effect_model(r"N:\GR\data\Sfx\Tex\s34031_a.tif", false)),
        (84016, effect_model("", true)),
    ]);
    let mut log = PassLog::default();
    pass().run(&mut world, &mut log).unwrap();
    assert_eq!(log.counts.get("effect models lowered"), Some(&2));
    // the world holds neither model's definition
    assert_eq!(log.approximations.get(models::UNREAD_DEFINITION), Some(&2));
    assert_eq!(log.approximations.get(models::ADDITIVE_BLENDED), None);
    assert_eq!(
        log.approximations.get(
            "effect model materials with no colour texture held, drawn in their particles' colour alone"
        ),
        Some(&1)
    );

    let mesh = world.get::<Mesh>(models[0]).expect("a mesh");
    assert_eq!(mesh.primitives().len(), 1);
    let p = &mesh.primitives()[0];
    assert_eq!(p.indices.as_ref().map(Vec::len), Some(3));
    let Some(Stream::F32x4(colours)) = p.streams.get(&Semantic::Color(0)) else {
        panic!("no vertex colour: {:?}", p.streams.keys());
    };
    assert_eq!(colours.len(), 3);
    assert!(colours.iter().all(|c| c[0] == 0.5 && c[1] == 0.25));
    let material = world.get::<PbrMaterial>(p.material.expect("a material")).unwrap();
    assert_eq!(material.shading, Shading::Unlit);
    assert_eq!(material.alpha_mode, AlphaMode::Blend);
    assert!(material.double_sided);
    assert_eq!(
        material.base_color_texture.as_ref().map(|t| t.texture),
        Some(texture)
    );

    let crystal = world.get::<Mesh>(models[1]).expect("a mesh");
    let material = world
        .get::<PbrMaterial>(crystal.primitives()[0].material.unwrap())
        .unwrap();
    assert!(material.base_color_texture.is_none());
    assert!(!material.double_sided);
}

/// A material binary as the effect models' are: `shader`, `params` and `samplers` (kind, path).
fn effect_definition(
    shader: &str,
    params: Vec<(&str, fromsoft_formats::matbin::Value)>,
    samplers: Vec<(&str, &str)>,
) -> crate::components::materials::Definition {
    use fromsoft_formats::matbin::{Material, Param, Sampler};
    crate::components::materials::Definition::Matbin(Material {
        shader: shader.into(),
        source: String::new(),
        key: 0,
        params: params
            .into_iter()
            .map(|(name, value)| Param {
                name: name.into(),
                key: 0,
                value,
            })
            .collect(),
        samplers: samplers
            .into_iter()
            .map(|(kind, path)| Sampler {
                kind: kind.into(),
                path: path.into(),
                key: 0,
                unknown: [0.0; 2],
            })
            .collect(),
        unreferenced_tail: Vec::new(),
    })
}

/// An effect model's material resolved with its definition, as the captured frames draw them: `S[A]_add_GXFlver` (a
/// `GXFlver_Col` program, lighting off, blend 4) multiplies its texture by `g_DiffuseColor` times `g_DiffuseColorPower` and
/// reads only the alpha of the colour the FLVER indexes 1; `S[ShiningStone]` (an `S[AMSN]_Glow` program, blend 0) is lit
/// and opaque, its base texture the one its definition's albedo sampler names where the FLVER's path is empty.
#[test]
fn an_effect_model_material_is_drawn_as_its_definition_states() {
    use crate::sekiro::components::effect::{EffectModel, EffectResource, ResourceKind};
    use fromsoft_formats::matbin::Value;
    use primase_core::pbr::{AlphaMode, PbrMaterial, Shading};
    use primase_core::{Mesh, Semantic, Stream};
    use std::sync::Arc;
    // the plain model: its one colour stored as COLOR1, (1, 0, 1, a)
    let mut plain = effect_model(r"N:\GR\data\Sfx\Tex\s34031_a.tif", false);
    plain.layouts[0][0].index = 1;
    for (i, v) in plain.meshes[0].vertices.iter_mut().enumerate() {
        v.colors = [[1.0, 0.0, 1.0, 0.25 * i as f32]].into();
    }
    // the crystal: no paths of its own
    let mut crystal = effect_model("", true);
    crystal.materials[0].mtd = r"N:\GR\data\Material\mtd\sfx\matxml\S[ShiningStone].matxml".into();
    crystal.materials[0].textures[0].kind = "C_Crystal__snp_Texture2D_1_AlbedoMap".into();
    let (mut world, texture, models) = world_with_models(vec![(83050, plain), (84016, crystal)]);
    let stone = world.spawn();
    world.insert(
        stone,
        EffectResource {
            kind: ResourceKind::Albedo,
            id: 34050,
        },
    );
    world.insert(
        stone,
        primase_core::TextureAbsent {
            reason: "a stand-in".into(),
        },
    );
    let add = effect_definition(
        r"N:\GR\data\Material\spx\GXFlver_Col.spx",
        vec![
            ("GXFT_InvalidLighting", Value::Bool(true)),
            ("g_BlendMode", Value::Int(4)),
            ("g_DiffuseColor", Value::Float3([1.0, 0.5, 1.0], Some([1.0, 1.0]))),
            ("g_DiffuseColorPower", Value::Float(20.0)),
        ],
        vec![("g_DiffuseTexture", "")],
    );
    let shining = effect_definition(
        r"N:\GR\data\Material\sat\OutputData\SPX\S[AMSN]_Glow.spx",
        vec![
            ("g_BlendMode", Value::Int(0)),
            (
                "g_DiffuseMapColor",
                Value::Float3([1.0, 1.0, 1.0], Some([1.0, 1.0])),
            ),
        ],
        vec![
            (
                "S_AMSN__Glow_snp_Texture2D_2_AlbedoMap_0",
                r"N:\GR\data\Sfx\Tex\s34050_a.tif",
            ),
            (
                "S_AMSN__Glow_snp_Texture2D_7_NormalMap_4",
                r"N:\GR\data\Sfx\Tex\s00005_n.tif",
            ),
        ],
    );
    for (e, stem, d) in [
        (models[0], "s[a]_add_gxflver", add),
        (models[1], "s[shiningstone]", shining),
    ] {
        let mut m = world.get::<EffectModel>(e).unwrap().clone();
        m.definitions.insert(stem.into(), Arc::new(d));
        world.insert(e, m);
    }
    let resources = world
        .iter::<EffectResource>()
        .map(|(e, r)| ((r.kind, r.id), e))
        .collect();
    let mut log = PassLog::default();
    let looks = models::lower(&mut world, &resources, &mut log).unwrap();
    // what a rich model drawing each is drawn with
    assert_eq!(
        looks[&models[0]],
        models::Look {
            blend: Some(Blend::Additive),
            lit: false
        }
    );
    assert_eq!(
        looks[&models[1]],
        models::Look {
            blend: Some(Blend::Replace),
            lit: true
        }
    );
    assert_eq!(log.approximations.get(models::UNREAD_DEFINITION), None);
    assert_eq!(log.approximations.get(models::PROGRAM_NOT_DRAWN), Some(&2));
    assert_eq!(log.approximations.get(models::ADDITIVE_BLENDED), Some(&1));

    let mesh = world.get::<Mesh>(models[0]).unwrap();
    let p = &mesh.primitives()[0];
    let material = world.get::<PbrMaterial>(p.material.unwrap()).unwrap();
    assert_eq!(material.base_color, [20.0, 10.0, 20.0, 1.0]);
    assert_eq!(material.shading, Shading::Unlit);
    assert_eq!(material.alpha_mode, AlphaMode::Blend);
    assert_eq!(
        material.base_color_texture.as_ref().map(|t| t.texture),
        Some(texture)
    );
    // the colour the program reads: white with COLOR1's alpha
    let Some(Stream::F32x4(colours)) = p.streams.get(&Semantic::Color(0)) else {
        panic!("no vertex colour: {:?}", p.streams.keys());
    };
    assert_eq!(
        colours,
        &vec![[1.0, 1.0, 1.0, 0.0], [1.0, 1.0, 1.0, 0.25], [1.0, 1.0, 1.0, 0.5]]
    );

    let mesh = world.get::<Mesh>(models[1]).unwrap();
    let material = world
        .get::<PbrMaterial>(mesh.primitives()[0].material.unwrap())
        .unwrap();
    assert_eq!(material.base_color, [1.0; 4]);
    assert_eq!(material.shading, Shading::Lit);
    assert_eq!(material.alpha_mode, AlphaMode::Opaque);
    assert_eq!(
        material.base_color_texture.as_ref().map(|t| t.texture),
        Some(stone)
    );
    assert_eq!(
        log.approximations.get("effect model normal textures not drawn"),
        None,
        "the normal map is not held: the preview names none"
    );
}

/// A model with no level-0 triangles is left without a mesh, counted: no particle draws it.
#[test]
fn an_effect_model_with_nothing_to_draw_is_left_out() {
    use primase_core::Mesh;
    let mut model = effect_model("", true);
    model.meshes[0].faces[0].flags = 0x0100_0000;
    let (mut world, _, models) = world_with_models(vec![(1, model)]);
    let mut log = PassLog::default();
    pass().run(&mut world, &mut log).unwrap();
    assert!(!world.has::<Mesh>(models[0]));
    assert_eq!(
        log.omitted.get("effect model meshes with no level-0 face set"),
        Some(&1)
    );
    assert_eq!(
        log.omitted
            .get("effect models with no level-0 triangles, left out"),
        Some(&1)
    );
}

/// A bullet row that flies at 10 m/s for a second and shows `effect`.
fn bullet_row(id: i32, effect: i32) -> fromsoft_formats::param::Bullet {
    fromsoft_formats::param::Bullet {
        id,
        effect,
        hit_effect: -1,
        flick_effect: -1,
        life: 1.0,
        life_random: 0.0,
        range: 0.0,
        gravity_in_range: 0.0,
        gravity_out_of_range: 0.0,
        initial_speed: 10.0,
        acceleration_in_range: 0.0,
        acceleration_out_of_range: 0.0,
        max_speed: 10.0,
        min_speed: 0.0,
        acceleration_delay: 0.0,
        spawned_bullet: -1,
        follow: 0,
        interval_bullet: -1,
        interval_min: 0.0,
        interval_max: 0.0,
        interval_wait: 0.0,
        inherit_speed: false,
        homing_angle: 0,
        homing_angle_x: -1,
        homing_begin: 0.0,
        homing_stop_range: 0.0,
        homing_offset: 0.0,
        auto_homing: false,
    }
}

#[test]
fn a_bullet_flies_carrying_the_system_of_its_effect_and_one_whose_effect_is_not_kept_flies_unseen() {
    use crate::sekiro::components::effect::Bullet;
    let fx = simple();
    let mut world = World::new();
    let file = world.spawn();
    world.insert(
        file,
        EffectFile {
            effect: fx.id,
            game: Game::Sekiro,
            file: Arc::new(fx.clone()),
        },
    );
    let seen = world.spawn();
    world.insert(seen, Transform::IDENTITY);
    world.insert(seen, Bullet(bullet_row(1, fx.id)));
    let unseen = world.spawn();
    world.insert(unseen, Transform::IDENTITY);
    world.insert(unseen, Bullet(bullet_row(2, fx.id + 1)));
    let mut log = PassLog::default();
    pass().run(&mut world, &mut log).unwrap();
    assert_eq!(log.counts.get("bullets lowered"), Some(&2));
    assert_eq!(log.omitted.get(bullets::NO_EFFECT), Some(&1));

    let system = world.get::<ParticleSystem>(seen).unwrap();
    assert!(system.verify(seen, &world).is_ok());
    let [emitter] = system.emitters.as_slice() else {
        panic!("{} emitters", system.emitters.len());
    };
    assert_eq!(emitter.duration, Some(1.0));
    let [started] = emitter.spawns.as_slice() else {
        panic!("{} systems started", emitter.spawns.len());
    };
    assert!(world.has::<ParticleSystem>(*started) && !world.has::<Transform>(*started));
    let motion = emitter.motion.as_ref().unwrap();
    assert_eq!((motion.forward, motion.max_speed), (units::FORWARD, Some(10.0)));

    let system = world.get::<ParticleSystem>(unseen).unwrap();
    assert!(system.emitters[0].spawns.is_empty());
    assert_eq!(system.lost, [(bullets::NO_EFFECT.to_owned(), 1)]);

    // a bullet stands where its transform places it
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Bullet(bullet_row(1, 5)));
    let e = pass().run(&mut world, &mut PassLog::default()).unwrap_err();
    assert!(e.contains("a bullet without a transform"), "{e}");
}
