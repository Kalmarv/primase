//! Behaviour of the effects lowering on small effects built by hand (each action's lists as short as the layout allows, the
//! rest at the project's defaults), and of the pass on a world holding them. The real-data census (every installed effect
//! lowered, accounting checked) is in `tests/` of the crate.
use super::*;
use crate::sekiro::components::effect::{Effect, EffectFile};
use crate::sekiro::source::EffectPlace;
use fromsoft_formats::Game;
use fromsoft_formats::fxr::{Action, Config, Fxr, Node, Property, State};
use primase_core::particles::{Anchor, Blend, Drive, Follow, Heading, Shape, Spawn, Spread, Termination};
use primase_core::{Component, Entity, Transform, World};
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
    Node { id, actions, configs, children }
}

fn fx(id: i32, states: usize, root: Node) -> Fxr {
    Fxr {
        id,
        states: (0..states).map(|_| State { conditions: Vec::new() }).collect(),
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
            with(action(300), vec![constant(0.25), constant(2.0), constant(8.0), constant(-1.0)], vec![1]),
            with(action(400), vec![], vec![1]),
            with(action(501), vec![constant(20.0), constant(0.0)], vec![]),
            with(action(55), vec![constant(9.8), constant(1.0), constant(1.0)], vec![0, f(0.0)]),
            with(action(600), vec![constant(5.0), constant(4.0), constant(0.3)], vec![]),
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
        Spawn::Periodic { interval, count, total, max_alive } => {
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
    assert_eq!(e.law.lifetime.as_ref().map(|v| v.curve.value_at(0.0)[0]), Some(2.0));
    let m = e.law.movement.as_ref().unwrap();
    assert!(matches!(m.drive, Drive::Acceleration { .. }));
    assert_eq!(m.gravity.curve.value_at(0.0)[0], 9.8);
    match &e.draw {
        Some(primase_core::particles::Draw::Point(p)) => assert_eq!(p.blend, Blend::Additive),
        other => panic!("{other:?}"),
    }
    // no file was found for texture 5: counted
    assert_eq!(l.notes.lost.get("a texture that is not in the effect binders"), Some(&1));
    // the unknown action 130 is in no hand-built config; a point sprite's colour factors that are 1 are left out
    assert_eq!(s.lost.iter().map(|(_, n)| *n).sum::<u32>(), l.notes.lost.values().sum::<u32>());
    // the system the lowering made is one the world accepts
    let mut world = World::new();
    let e = world.spawn();
    assert!(s.verify(e, &world).is_ok());
}

#[test]
fn a_node_with_two_configs_is_two_emitters_by_state() {
    let second = Config {
        id: 1004,
        actions: vec![with(action(128), vec![constant(-1.0)], vec![f(0.0), 0, 1, f(0.0)]), with(action(399), vec![], vec![])],
    };
    let root = node(2000, vec![], vec![], vec![node(2200, vec![map(&[0, 1])], vec![torch(), second], vec![])]);
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
        fn model(&mut self, _: i32) -> Option<Entity> {
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
    let root = node(2000, vec![], vec![], vec![node(2001, vec![reference], vec![], vec![])]);
    let l = lower(&fx(3, 1, root), Game::Sekiro, 0, &mut One(target)).unwrap();
    assert_eq!(l.notes.lost.get("a spawned effect that is not in the effect binders or closes a cycle"), Some(&1));
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
    let cfg = Config { id: 1004, actions: vec![place, with(action(399), vec![], vec![])] };
    let root = node(2000, vec![], vec![], vec![node(2200, vec![map(&[0])], vec![cfg], vec![])]);
    let l = lower(&fx(5, 1, root), Game::Sekiro, 0, &mut NoFiles).unwrap();
    let t = l.system.emitters[0].placement;
    assert_eq!(t.cols[3], [1.0, 2.0, -3.0]);
    let _ = (Anchor::Parent, Follow::Frame);
}

/// A world with the effect and a place for it.
fn world_with(effect: Fxr, starts_disabled: bool) -> (World, Entity) {
    let mut world = World::new();
    let file = world.spawn();
    world.insert(file, EffectFile { effect: effect.id, game: Game::Sekiro, file: Arc::new(effect.clone()) });
    let place = world.spawn();
    world.insert(place, Transform::IDENTITY);
    world.insert(
        place,
        Effect { effect: effect.id, place: EffectPlace::Region { region: "r".into() }, starts_disabled },
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
    assert!(log.omitted.contains_key("a texture that is not in the effect binders"));
    assert!(system.verify(place, &world).is_ok());
}

#[test]
fn the_pass_leaves_out_regions_that_start_disabled_unless_asked() {
    let (mut world, place) = world_with(simple(), true);
    let mut log = PassLog::default();
    pass().run(&mut world, &mut log).unwrap();
    assert!(world.get::<ParticleSystem>(place).is_none());
    assert_eq!(log.omitted.get("effects of regions that start disabled, left out"), Some(&1));
    let mut log = PassLog::default();
    pass().with_include_disabled(Disabled::Include).run(&mut world, &mut log).unwrap();
    assert!(world.get::<ParticleSystem>(place).is_some());
}

#[test]
fn a_spawn_cycle_is_cut_where_it_closes_and_a_spawned_effect_gets_a_system_of_its_own() {
    let mut a = action(132);
    a.fields1 = vec![2];
    let mut b = action(132);
    b.fields1 = vec![1];
    let mut one = fx(1, 1, node(2000, vec![], vec![], vec![node(2001, vec![a], vec![], vec![])]));
    one.references = vec![2];
    let mut two = fx(2, 1, node(2000, vec![], vec![], vec![node(2001, vec![b], vec![], vec![])]));
    two.references = vec![1];
    let mut world = World::new();
    for e in [one, two] {
        let file = world.spawn();
        world.insert(file, EffectFile { effect: e.id, game: Game::Sekiro, file: Arc::new(e) });
    }
    let place = world.spawn();
    world.insert(place, Transform::IDENTITY);
    world.insert(place, Effect { effect: 1, place: EffectPlace::Region { region: "r".into() }, starts_disabled: false });
    let mut log = PassLog::default();
    pass().run(&mut world, &mut log).unwrap();
    // effect 1 names effect 2's system; effect 2 may not name effect 1's (it can reach it): counted
    let s = world.get::<ParticleSystem>(place).unwrap();
    assert_eq!(s.emitters[0].spawns.len(), 1);
    assert_eq!(log.counts.get("effects lowered"), Some(&2));
    assert!(log.omitted.contains_key("a spawned effect that is not in the effect binders or closes a cycle"));
    // the named system is one of the world's, with its own emitters
    let named = s.emitters[0].spawns[0];
    assert!(world.get::<ParticleSystem>(named).is_some());
    let mut refs = Vec::new();
    s.references(&mut refs);
    assert_eq!(refs, vec![named]);
    assert_eq!(ParticleSystem::NAME, "core.particles");
}
