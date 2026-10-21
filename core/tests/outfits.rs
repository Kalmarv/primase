//! Outfits as state: conditions on an asset's parts and on single primitives, resolved by `select-outfit` and
//! `resolve-state`. The fixture is a character asset of a rig, a body, two outfits sharing the rig (each skinned to
//! it) and a hat that is the second primitive of a two-primitive mesh, drawn with a material variant: hand-worked, so
//! what each choice leaves is stated here, not computed by the code under test.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};

use primase_core::ledger::Conservation;
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::state::{
    Clause, Condition, Domain, Literal, PrimitiveCondition, Scope, StateAxis, StateOverride,
};
use primase_core::{
    Asset, Binding, BindingId, Component, Deformation, Entity, Influences, Instance, Joint, JointId,
    MaterialVariants, Mesh, Name, Primitive, Rest, Role, Semantic, Skeleton, Skin, Slot, Stream, Topology,
    Transform, WeightRule, World,
};

struct Wardrobe {
    w: World,
    root: Entity,
    skeleton: Entity,
    body: Entity,
    outfit_a: Entity,
    outfit_b: Entity,
    hat: Entity,
    /// The axes: outfit (a|b, no default), hat (off|on, default off), skin (pale|dark, no default).
    outfit: Entity,
    hat_axis: Entity,
    tone: Entity,
    placement_a: Entity,
    placement_b: Entity,
}

fn axis(w: &mut World, key: &str, values: &[&str], default: Option<u32>) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        StateAxis {
            key: key.into(),
            domain: Domain::Enum(values.iter().map(|v| (*v).into()).collect()),
            default,
            scope: Scope::Global,
            class: "outfit".into(),
            source: format!("wardrobe {key}"),
        },
    );
    e
}

fn is(axis: Entity, value: u32) -> Condition {
    Condition(vec![Clause(vec![Literal {
        axis,
        values: 1 << value,
    }])])
}

/// A mesh of `n` one-point primitives, each skinned wholly to the skeleton's only joint.
fn skinned(w: &mut World, name: &str, n: usize, material: Entity, skeleton: Entity) -> Entity {
    let e = w.spawn();
    w.insert(e, Name(name.into()));
    w.insert(
        e,
        Mesh::new(
            (0..n)
                .map(|i| Primitive {
                    topology: Topology::Points,
                    streams: [(Semantic::Position, Stream::F32x3(vec![[i as f32, 0.0, 0.0]]))].into(),
                    indices: None,
                    material: Some(material),
                })
                .collect(),
        ),
    );
    w.insert(
        e,
        Skin {
            influences: vec![Some(Influences::Whole { slot: 0 }); n],
            weights: WeightRule::SumsToOne,
            bindings: vec![Binding {
                id: BindingId(0),
                skeleton,
                palettes: vec![
                    Some(vec![Slot {
                        joint: JointId(0),
                        inverse_bind: Transform::IDENTITY,
                    }]);
                    n
                ],
                deformation: Deformation::Linear,
            }],
        },
    );
    e
}

fn wardrobe() -> Wardrobe {
    let mut w = World::new();
    let skeleton = w.spawn();
    w.insert(
        skeleton,
        Skeleton {
            joints: vec![Joint {
                id: JointId(0),
                name: "root".into(),
                parent: None,
                rest: Rest::Affine(Transform::IDENTITY),
            }],
        },
    );
    let (paint, pale, dark) = (w.spawn(), w.spawn(), w.spawn());
    let body = skinned(&mut w, "body", 1, paint, skeleton);
    let outfit_a = skinned(&mut w, "outfit a", 1, paint, skeleton);
    let outfit_b = skinned(&mut w, "outfit b", 1, paint, skeleton);
    // the hat: primitive 0 is the head, primitive 1 the hat, with the skin tone as a material variant of the head
    let hat = skinned(&mut w, "head and hat", 2, pale, skeleton);
    w.insert(
        hat,
        MaterialVariants(vec![BTreeMap::from([(1, dark)]), BTreeMap::new()]),
    );
    let outfit = axis(&mut w, "outfit", &["a", "b"], None);
    let hat_axis = axis(&mut w, "hat", &["off", "on"], Some(0));
    let tone = axis(&mut w, "skin", &["pale", "dark"], None);
    w.insert(outfit_a, is(outfit, 0));
    w.insert(outfit_b, is(outfit, 1));
    w.insert(hat, PrimitiveCondition(vec![None, Some(is(hat_axis, 1))]));
    let placement = |w: &mut World, proto: Entity| {
        let p = w.spawn();
        w.insert(
            p,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        w.insert(p, Transform::IDENTITY);
        p
    };
    let (placement_a, placement_b) = (placement(&mut w, outfit_a), placement(&mut w, outfit_b));
    // the head's variant follows the skin axis, on its placement
    let head = placement(&mut w, hat);
    w.insert(
        head,
        StateOverride(vec![(
            Clause(vec![Literal {
                axis: tone,
                values: 0b10,
            }]),
            primase_core::state::Override::Variant(1),
        )]),
    );
    let root = w.spawn();
    w.insert(root, Name("character".into()));
    w.insert(
        root,
        Asset {
            members: vec![
                (skeleton, Role::Rig),
                (body, Role::Part),
                (outfit_a, Role::Alternative),
                (outfit_b, Role::Alternative),
                (hat, Role::Part),
            ],
        },
    );
    Wardrobe {
        w,
        root,
        skeleton,
        body,
        outfit_a,
        outfit_b,
        hat,
        outfit,
        hat_axis,
        tone,
        placement_a,
        placement_b,
    }
}

fn manager() -> PassManager {
    let mut r = PassRegistry::new();
    primase_core::register(&mut r);
    PassManager::new(r)
        .with_pass_checks(true)
        .with_conservation(Conservation::Enforce)
}

fn run(m: &PassManager, w: &mut World, pipeline: &str) -> RunReport {
    let mut report = RunReport::default();
    m.run(w, &m.registry.parse(pipeline).unwrap(), &mut report)
        .unwrap_or_else(|e| panic!("{pipeline}: {e}"));
    report
}

fn members(w: &World, root: Entity) -> Vec<Entity> {
    w.get::<Asset>(root)
        .unwrap()
        .members
        .iter()
        .map(|&(e, _)| e)
        .collect()
}

/// The world as JSON text, entity by entity, for comparing two routes to the same state.
fn dump(w: &World) -> String {
    let mut s = String::new();
    for e in w.entities() {
        for c in w.components_json(e).unwrap() {
            s += &format!("{e:?} {} {:?}\n", c.name, c.value);
        }
    }
    s
}

#[test]
fn the_fixture_verifies() {
    let f = wardrobe();
    assert_eq!(f.w.verify_components(), vec![]);
}

/// Choosing outfit a drops outfit b (member, entity and placement) and leaves the unchosen axes and what depends on
/// them: the hat's primitive condition and the head's override, which names the skin axis.
#[test]
fn choosing_an_outfit_drops_the_other_and_leaves_the_rest_open() {
    let m = manager();
    let mut f = wardrobe();
    let report = run(&m, &mut f.w, "select-outfit{choices=outfit:a}");
    let log = &report.steps[0].log;
    assert_eq!(members(&f.w, f.root), [f.skeleton, f.body, f.outfit_a, f.hat]);
    assert!(!f.w.is_alive(f.outfit_b) && !f.w.is_alive(f.placement_b));
    assert!(f.w.is_alive(f.outfit_a) && f.w.is_alive(f.placement_a));
    assert!(
        !f.w.has::<Condition>(f.outfit_a),
        "its condition always holds now"
    );
    // unchosen axes stay: the hat axis has a default but select-outfit chooses only what it names
    assert!(
        f.w.has::<StateAxis>(f.hat_axis) && f.w.has::<StateAxis>(f.tone) && !f.w.has::<StateAxis>(f.outfit)
    );
    assert_eq!(f.w.get::<PrimitiveCondition>(f.hat).unwrap().0.len(), 2);
    assert_eq!(log.counts["asset members despawned"], 1);
    assert_eq!(log.counts["placements of an absent member, despawned"], 1);
    assert_eq!(log.counts["outfit=a kept"], 1);
    assert_eq!(log.counts["outfit=a despawned"], 1);
    assert_eq!(f.w.verify_components(), vec![]);
}

/// The hat axis off drops primitive 1 of the hat mesh; its skin palettes and material variants follow, and the
/// declared primitive drop satisfies the conservation check (enforced by the manager).
#[test]
fn a_primitive_condition_drops_its_primitive_and_every_per_primitive_component_follows() {
    let m = manager();
    let mut f = wardrobe();
    let report = run(&m, &mut f.w, "select-outfit{choices=hat:off}");
    let mesh = f.w.get::<Mesh>(f.hat).unwrap();
    assert_eq!(mesh.primitives().len(), 1);
    assert_eq!(mesh.primitives()[0].positions().unwrap(), [[0.0, 0.0, 0.0]]);
    let skin = f.w.get::<Skin>(f.hat).unwrap();
    assert_eq!((skin.influences.len(), skin.bindings[0].palettes.len()), (1, 1));
    assert_eq!(f.w.get::<MaterialVariants>(f.hat).unwrap().0.len(), 1);
    assert!(!f.w.has::<PrimitiveCondition>(f.hat));
    let log = &report.steps[0].log;
    assert_eq!(log.counts["primitives dropped by their condition"], 1);
    assert_eq!(log.counts["hat=off primitives dropped"], 1);
    assert_eq!(f.w.verify_components(), vec![]);

    // hat on: both primitives stay, the condition is gone
    let mut f = wardrobe();
    run(&m, &mut f.w, "select-outfit{choices=hat:on}");
    assert_eq!(f.w.get::<Mesh>(f.hat).unwrap().primitives().len(), 2);
    assert!(!f.w.has::<PrimitiveCondition>(f.hat));
}

/// A mesh none of whose primitives is present is not present: its member, placements and entity go.
#[test]
fn a_mesh_with_no_primitive_left_is_not_present() {
    let m = manager();
    let mut f = wardrobe();
    f.w.insert(
        f.hat,
        PrimitiveCondition(vec![Some(is(f.hat_axis, 1)), Some(is(f.hat_axis, 1))]),
    );
    run(&m, &mut f.w, "select-outfit{choices=hat:off}");
    assert!(!f.w.is_alive(f.hat));
    assert_eq!(
        members(&f.w, f.root),
        [f.skeleton, f.body, f.outfit_a, f.outfit_b]
    );
    assert_eq!(f.w.verify_components(), vec![]);
}

/// A condition on a primitive follows the primitive when another pass splits, reorders or drops primitives.
#[test]
fn a_primitive_condition_follows_remap_primitives() {
    let mut f = wardrobe();
    f.w.rebuild_primitives(f.hat, |mesh| {
        let p = mesh.primitives();
        Ok((vec![p[1].clone(), p[0].clone(), p[1].clone()], vec![1, 0, 1]))
    })
    .unwrap();
    let c = &f.w.get::<PrimitiveCondition>(f.hat).unwrap().0;
    assert_eq!(c.len(), 3);
    assert_eq!(
        (c[0].is_some(), c[1].is_some(), c[2].is_some()),
        (true, false, true)
    );
    assert_eq!(f.w.verify_components(), vec![]);
}

/// Choosing every axis by hand in one `select-outfit` gives the world `resolve-state` with the same assignment
/// gives; choosing them one `select-outfit` at a time does too.
#[test]
fn select_outfit_in_steps_equals_resolve_state_at_once() {
    let m = manager();
    let mut once = wardrobe();
    run(&m, &mut once.w, "resolve-state{assign=outfit:b;hat:on;skin:dark}");
    let mut steps = wardrobe();
    run(
        &m,
        &mut steps.w,
        "select-outfit{choices=outfit:b}, select-outfit{choices=skin:dark}, select-outfit{choices=hat:on}",
    );
    assert_eq!(dump(&once.w), dump(&steps.w));
    // by hand: outfit b stays with its placement, the dark variant is on the head, both hat primitives stay
    assert_eq!(
        members(&once.w, once.root),
        [once.skeleton, once.body, once.outfit_b, once.hat]
    );
    assert_eq!(once.w.iter::<StateAxis>().count(), 0);
    let head: Vec<u32> = once
        .w
        .iter::<Instance>()
        .filter(|(_, i)| i.prototype == once.hat)
        .map(|(_, i)| i.variant)
        .collect();
    assert_eq!(head, [1]);
}

/// An axis with no default and no choice stays unresolved with what depends on it (no outfit is invented).
#[test]
fn an_unchosen_axis_without_a_default_invents_nothing() {
    let m = manager();
    let mut f = wardrobe();
    run(&m, &mut f.w, "resolve-state{}");
    let unresolved: BTreeSet<String> = f.w.iter::<StateAxis>().map(|(_, a)| a.key.clone()).collect();
    assert_eq!(unresolved, ["outfit".to_owned(), "skin".to_owned()].into());
    assert!(f.w.is_alive(f.outfit_a) && f.w.is_alive(f.outfit_b));
    assert!(f.w.has::<Condition>(f.outfit_a) && f.w.has::<Condition>(f.outfit_b));
}

#[test]
fn select_outfit_refuses_what_it_cannot_apply() {
    let m = manager();
    for (bad, why) in [
        ("select-outfit", "choices is required"),
        ("select-outfit{choices=outfit}", "not key:value"),
        ("select-outfit{choices=outfit:a;outfit:b}", "outfit twice"),
        ("select-outfit{pick=outfit:a}", "unknown option pick"),
    ] {
        let err = m.registry.parse(bad).unwrap_err().to_string();
        assert!(err.contains(why), "{bad}: {err}");
    }
    let mut f = wardrobe();
    let err = m
        .run(
            &mut f.w,
            &m.registry.parse("select-outfit{choices=colour:red}").unwrap(),
            &mut RunReport::default(),
        )
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("colour: no axis has that key (the world's: hat, outfit, skin)"),
        "{err}"
    );
    let err = m
        .run(
            &mut f.w,
            &m.registry.parse("select-outfit{choices=outfit:purple}").unwrap(),
            &mut RunReport::default(),
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("purple"), "{err}");
}

/// The verifier's rules for the new places a condition may sit.
#[test]
fn verify_accepts_members_and_primitives_and_refuses_the_rest() {
    let mut f = wardrobe();
    // a condition on the rig, a clip-less rule: only parts and alternatives are chosen
    f.w.insert(f.skeleton, is(f.outfit, 0));
    assert!(
        f.w.verify_components()
            .iter()
            .any(|(n, e, m)| *n == Condition::NAME
                && *e == f.skeleton
                && m.contains("not a part or alternative"))
    );
    f.w.remove::<Condition>(f.skeleton);
    // a body that is a member and placed by nobody still takes one
    f.w.insert(f.body, is(f.hat_axis, 1));
    assert_eq!(f.w.verify_components(), vec![]);
    // one entry per primitive
    f.w.insert(f.hat, PrimitiveCondition(vec![None]));
    assert!(
        f.w.verify_components()
            .iter()
            .any(|(n, _, m)| *n == PrimitiveCondition::NAME && m.contains("1 conditions for 2 primitives"))
    );
    // on an entity without a mesh
    f.w.insert(f.hat, PrimitiveCondition(vec![None, None]));
    f.w.insert(f.root, PrimitiveCondition(vec![]));
    assert!(
        f.w.verify_components()
            .iter()
            .any(|(n, e, m)| *n == PrimitiveCondition::NAME
                && *e == f.root
                && m.contains("without a core.mesh"))
    );
}
