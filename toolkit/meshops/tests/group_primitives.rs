//! `group-primitives` through the pass manager, with the conservation diagnostics enforced: what it joins is declared,
//! and the triangles stay.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::ledger::{Conservation, Measure};
use primase_core::pass::{Options, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::skeleton::{Joint, Rest, Skeleton};
use primase_core::skin::{Binding, BindingId, Deformation, Skin, Slot, WeightRule};
use primase_core::world::remap_list;
use primase_core::{
    Component, Entity, Influences, JointId, MaterialVariants, Mesh, MeshFile, Primitive, Semantic, Stream,
    Topology, Transform, World,
};
use primase_meshops::GroupPrimitives;

fn manager() -> PassManager {
    let mut registry = PassRegistry::new();
    primase_meshops::register(&mut registry);
    PassManager::new(registry)
        .with_pass_checks(true)
        .with_conservation(Conservation::Enforce)
}

fn run(w: &mut World, groups: usize, vertices: usize) -> Result<RunReport, String> {
    let mut report = RunReport::default();
    manager()
        .run(
            w,
            &Pipeline {
                passes: vec![Box::new(GroupPrimitives::new(groups, vertices).unwrap())],
            },
            &mut report,
        )
        .map_err(|e| e.to_string())?;
    Ok(report)
}

/// A triangle whose corners are identified by `x`: (x, 0, 0), (x + 1, 0, 0), (x, 1, 0).
fn triangle(material: Option<Entity>, x: f32) -> Primitive {
    Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([(
            Semantic::Position,
            Stream::F32x3(vec![[x, 0.0, 0.0], [x + 1.0, 0.0, 0.0], [x, 1.0, 0.0]]),
        )]),
        indices: None,
        material,
    }
}

/// The triangles of `p` as their corners' positions, in order.
fn corners(p: &Primitive) -> Vec<[[f32; 3]; 3]> {
    let pos = p.positions().unwrap();
    let idx: Vec<u32> = p
        .indices
        .clone()
        .unwrap_or_else(|| (0..pos.len() as u32).collect());
    idx.as_chunks::<3>()
        .0
        .iter()
        .map(|t| t.map(|i| pos[i as usize]))
        .collect()
}

fn mesh_of(w: &mut World, prims: Vec<Primitive>) -> Entity {
    let e = w.spawn();
    w.insert(e, Mesh::new(prims));
    e
}

fn materials(w: &mut World, n: usize) -> Vec<Entity> {
    (0..n).map(|_| w.spawn()).collect()
}

#[test]
fn primitives_of_a_material_are_joined_in_order_and_the_file_link_goes() {
    let mut w = World::new();
    let m = materials(&mut w, 2);
    let e = mesh_of(
        &mut w,
        vec![
            triangle(Some(m[0]), 0.0),
            triangle(Some(m[1]), 10.0),
            triangle(Some(m[0]), 20.0),
        ],
    );
    w.insert(
        e,
        MeshFile {
            path: "a.glb".into(),
            mesh: 0,
            materials: vec![Some(m[0]), Some(m[1]), Some(m[0])],
        },
    );
    let report = run(&mut w, 2, 100).unwrap();
    let prims = w.get::<Mesh>(e).unwrap().primitives();
    assert_eq!(
        prims.iter().map(|p| p.material).collect::<Vec<_>>(),
        [Some(m[0]), Some(m[1])]
    );
    assert_eq!(prims[0].indices.as_deref(), Some(&[0, 1, 2, 3, 4, 5][..]));
    assert_eq!(prims[0].vertex_count(), 6);
    assert_eq!(
        corners(&prims[0]).iter().map(|t| t[0][0]).collect::<Vec<_>>(),
        [0.0, 20.0]
    );
    // the primitive of one is kept exactly
    assert_eq!(prims[1], triangle(Some(m[1]), 10.0));
    assert!(!w.has::<MeshFile>(e));
    let log = &report.steps[0].log;
    assert_eq!(log.counts["meshes regrouped"], 1);
    assert_eq!(log.counts["primitives made"], 2);
    assert_eq!(log.counts["primitives joined by material"], 1);
    assert_eq!(log.declared(Measure::Primitives), 1);
    assert_eq!(log.declared(Measure::Triangles), 0);
    // the second A now drawn before B: the reorder is an approximation, counted once
    let reordered = |log: &primase_core::pass::PassLog| {
        log.approximations
            .iter()
            .find(|(k, _)| k.starts_with("primitives drawn earlier than the source"))
            .map(|(_, &n)| n)
    };
    assert_eq!(reordered(log), Some(1));
    // A, A, B joins without moving anything past B: no approximation
    let mut w = World::new();
    let m = materials(&mut w, 2);
    mesh_of(
        &mut w,
        vec![
            triangle(Some(m[0]), 0.0),
            triangle(Some(m[0]), 20.0),
            triangle(Some(m[1]), 10.0),
        ],
    );
    let report = run(&mut w, 2, 100).unwrap();
    assert_eq!(report.steps[0].log.counts["primitives joined by material"], 1);
    assert_eq!(reordered(&report.steps[0].log), None);
}

#[test]
fn a_mesh_already_grouped_is_not_touched() {
    let mut w = World::new();
    let m = materials(&mut w, 2);
    let e = mesh_of(&mut w, vec![triangle(Some(m[0]), 0.0), triangle(Some(m[1]), 5.0)]);
    w.insert(
        e,
        MeshFile {
            path: "a.glb".into(),
            mesh: 0,
            materials: vec![Some(m[0]), Some(m[1])],
        },
    );
    let report = run(&mut w, 7, 60000).unwrap();
    assert!(w.has::<MeshFile>(e));
    assert!(!report.steps[0].log.counts.contains_key("meshes regrouped"));
}

#[test]
fn a_group_over_the_vertex_limit_is_split_by_whole_triangles() {
    let mut w = World::new();
    let m = materials(&mut w, 1);
    // a fan of four triangles around vertex 0: six vertices
    let fan = Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([(
            Semantic::Position,
            Stream::F32x3((0..6).map(|i| [i as f32, (i * i) as f32, 0.0]).collect()),
        )]),
        indices: Some(vec![0, 1, 2, 0, 2, 3, 0, 3, 4, 0, 4, 5]),
        material: Some(m[0]),
    };
    let before = corners(&fan);
    let e = mesh_of(&mut w, vec![fan]);
    let report = run(&mut w, 2, 4).unwrap();
    let prims = w.get::<Mesh>(e).unwrap().primitives();
    assert_eq!(prims.len(), 2);
    assert!(prims.iter().all(|p| p.vertex_count() <= 4));
    let after: Vec<_> = prims.iter().flat_map(corners).collect();
    assert_eq!(after, before, "every triangle, in order, with its corners");
    let log = &report.steps[0].log;
    assert_eq!(log.counts["parts split off by triangles"], 1);
    assert_eq!(log.declared(Measure::Primitives), 0);
    assert_eq!(log.declared(Measure::Triangles), 0);
}

#[test]
fn limits_that_cannot_be_met_are_refused_and_nothing_changes() {
    // one material whose vertices fill four parts of at most 4
    let mut w = World::new();
    let m = materials(&mut w, 1);
    let e = mesh_of(
        &mut w,
        (0..4).map(|i| triangle(Some(m[0]), i as f32 * 3.0)).collect(),
    );
    let before = w.get::<Mesh>(e).unwrap().clone();
    let err = run(&mut w, 3, 4).unwrap_err();
    assert!(
        err.contains("max-groups=3") && err.contains("fill 4 primitives"),
        "{err}"
    );
    assert_eq!(w.get::<Mesh>(e).unwrap(), &before);
}

#[test]
fn several_materials_above_the_limit_are_refused_naming_the_atlas_pass() {
    let mut w = World::new();
    let m = materials(&mut w, 3);
    let e = mesh_of(
        &mut w,
        (0..3).map(|i| triangle(Some(m[i]), i as f32 * 3.0)).collect(),
    );
    let before = w.get::<Mesh>(e).unwrap().clone();
    let err = run(&mut w, 2, 100).unwrap_err();
    assert!(
        err.contains("pack-texture-atlas") && err.contains("3 materials"),
        "{err}"
    );
    assert_eq!(w.get::<Mesh>(e).unwrap(), &before);
}

#[test]
fn other_topologies_and_unequal_streams_are_refused_naming_them() {
    let mut w = World::new();
    let m = materials(&mut w, 1);
    let mut lines = triangle(Some(m[0]), 0.0);
    lines.topology = Topology::Lines;
    mesh_of(&mut w, vec![lines]);
    let err = run(&mut w, 1, 100).unwrap_err();
    assert!(err.contains("primitive 0 is Lines"), "{err}");

    let mut w = World::new();
    let mut with_uv = triangle(Some(m[0]), 0.0);
    with_uv
        .streams
        .insert(Semantic::TexCoord(0), Stream::F32x2(vec![[0.0; 2]; 3]));
    mesh_of(&mut w, vec![with_uv, triangle(Some(m[0]), 5.0)]);
    let err = run(&mut w, 1, 100).unwrap_err();
    assert!(
        err.contains("TexCoord(0)") && err.contains("F32x2 in primitive 0 and absent in primitive 1"),
        "{err}"
    );

    // the same semantic in another type
    let mut w = World::new();
    let mut a = triangle(Some(m[0]), 0.0);
    let mut b = triangle(Some(m[0]), 5.0);
    a.streams
        .insert(Semantic::Color(0), Stream::U8x4(vec![[0; 4]; 3]));
    b.streams
        .insert(Semantic::Color(0), Stream::F32x4(vec![[0.0; 4]; 3]));
    mesh_of(&mut w, vec![a, b]);
    let err = run(&mut w, 1, 100).unwrap_err();
    assert!(
        err.contains("Color(0)") && err.contains("U8x4") && err.contains("F32x4"),
        "{err}"
    );
}

#[test]
fn material_variants_and_per_vertex_data_keep_primitives_from_being_joined() {
    let mut w = World::new();
    let m = materials(&mut w, 2);
    let e = mesh_of(&mut w, vec![triangle(Some(m[0]), 0.0), triangle(Some(m[0]), 5.0)]);
    w.insert(
        e,
        MaterialVariants(vec![BTreeMap::from([(1, m[1])]), BTreeMap::new()]),
    );
    let err = run(&mut w, 1, 100).unwrap_err();
    assert!(err.contains("material variants"), "{err}");

    // variants of a primitive that is not joined follow it
    let mut w = World::new();
    let m = materials(&mut w, 2);
    let e = mesh_of(&mut w, vec![triangle(Some(m[0]), 0.0), triangle(Some(m[1]), 5.0)]);
    w.insert(
        e,
        MaterialVariants(vec![BTreeMap::new(), BTreeMap::from([(1, m[0])])]),
    );
    run(&mut w, 2, 100).unwrap();
    assert_eq!(w.get::<MaterialVariants>(e).unwrap().0.len(), 2);

    #[derive(Clone, Debug, PartialEq)]
    struct Attributes(Vec<Vec<u32>>);
    impl Component for Attributes {
        const NAME: &'static str = "test.attributes";
        const GEOMETRY: primase_core::Geometry = primase_core::Geometry::PerVertex;
        fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
            Some(remap_list(&self.0, from).map(Attributes))
        }
        fn remap_vertices(&self, _primitive: usize, _from: &[u32]) -> Option<Result<Self, String>> {
            Some(Ok(self.clone()))
        }
    }
    let mut w = World::new();
    let e = mesh_of(&mut w, vec![triangle(Some(m[0]), 0.0), triangle(Some(m[0]), 5.0)]);
    w.insert(e, Attributes(vec![vec![0; 3], vec![1; 3]]));
    let err = run(&mut w, 1, 100).unwrap_err();
    assert!(
        err.contains("test.attributes") && err.contains("per vertex"),
        "{err}"
    );
}

/// A skinned world: a skeleton of joints 10, 11, 12, and a skin to fill in per test.
fn skeleton(w: &mut World) -> Entity {
    let s = w.spawn();
    let joint = |id: u32| Joint {
        id: JointId(id),
        name: format!("j{id}"),
        parent: None,
        rest: Rest::Affine(Transform::IDENTITY),
    };
    w.insert(
        s,
        Skeleton {
            joints: vec![joint(10), joint(11), joint(12)],
        },
    );
    s
}

fn slot(joint: u32, y: f32) -> Slot {
    let mut inverse_bind = Transform::IDENTITY;
    inverse_bind.cols[3] = [0.0, y, 0.0];
    Slot {
        joint: JointId(joint),
        inverse_bind,
    }
}

/// A skinned triangle: vertex k bound wholly to slot `slots[k]`.
fn skinned(material: Entity, x: f32, slots: [u8; 3]) -> Primitive {
    let mut p = triangle(Some(material), x);
    p.streams.insert(
        Semantic::Joints(0),
        Stream::U8x4(slots.map(|s| [s, 0, 0, 0]).to_vec()),
    );
    p.streams
        .insert(Semantic::Weights(0), Stream::U8x4(vec![[255, 0, 0, 0]; 3]));
    p
}

/// The joint and inverse bind vertex `v` of primitive `i` is wholly bound to.
fn bound(w: &World, e: Entity, i: usize, v: usize) -> Slot {
    let Stream::U8x4(j) = &w.get::<Mesh>(e).unwrap().primitives()[i].streams[&Semantic::Joints(0)] else {
        panic!("joints are bytes");
    };
    w.get::<Skin>(e).unwrap().bindings[0].palettes[i]
        .as_ref()
        .unwrap()[j[v][0] as usize]
}

#[test]
fn skinned_primitives_get_one_palette_and_their_joints_are_rewritten() {
    let mut w = World::new();
    let sk = skeleton(&mut w);
    let m = materials(&mut w, 1)[0];
    // the palettes hold the same slots in another order, and the second one a slot more
    let a = vec![slot(10, 0.0), slot(11, -1.0)];
    let b = vec![slot(11, -1.0), slot(10, 0.0), slot(12, -2.0)];
    let e = mesh_of(
        &mut w,
        vec![skinned(m, 0.0, [0, 1, 0]), skinned(m, 5.0, [0, 1, 2])],
    );
    w.insert(
        e,
        Skin {
            influences: vec![Some(Influences::Streams { sets: 1 }); 2],
            weights: WeightRule::SumsToOne,
            bindings: vec![Binding {
                id: BindingId(0),
                skeleton: sk,
                palettes: vec![Some(a.clone()), Some(b.clone())],
                deformation: Deformation::Linear,
            }],
        },
    );
    assert!(w.verify_components().is_empty());
    let wanted: Vec<Slot> = [a[0], a[1], a[0], b[0], b[1], b[2]].to_vec();
    run(&mut w, 1, 100).unwrap();
    assert!(w.verify_components().is_empty(), "{:?}", w.verify_components());
    assert_eq!(w.get::<Mesh>(e).unwrap().primitives().len(), 1);
    let palette = w.get::<Skin>(e).unwrap().bindings[0].palettes[0].clone().unwrap();
    assert_eq!(palette, [a[0], a[1], b[2]], "first-use order, each slot once");
    let got: Vec<Slot> = (0..6).map(|v| bound(&w, e, 0, v)).collect();
    assert_eq!(
        got, wanted,
        "every vertex stays bound to its joint and inverse bind"
    );
}

#[test]
fn a_skinned_primitive_is_not_joined_to_an_unskinned_one() {
    let mut w = World::new();
    let sk = skeleton(&mut w);
    let m = w.spawn();
    let e = mesh_of(&mut w, vec![skinned(m, 0.0, [0, 0, 0]), triangle(Some(m), 5.0)]);
    w.insert(
        e,
        Skin {
            influences: vec![Some(Influences::Streams { sets: 1 }), None],
            weights: WeightRule::SumsToOne,
            bindings: vec![Binding {
                id: BindingId(0),
                skeleton: sk,
                palettes: vec![Some(vec![slot(10, 0.0)]), None],
                deformation: Deformation::Linear,
            }],
        },
    );
    let err = run(&mut w, 1, 100).unwrap_err();
    assert!(
        err.contains("Joints(0) is U8x4 in primitive 0 and absent in primitive 1"),
        "{err}"
    );
}

#[test]
fn primitives_bound_wholly_to_different_joints_are_refused_and_to_one_joint_joined() {
    let build = |slots: [u32; 2]| {
        let mut w = World::new();
        let sk = skeleton(&mut w);
        let m = w.spawn();
        let e = mesh_of(&mut w, vec![triangle(Some(m), 0.0), triangle(Some(m), 5.0)]);
        let palette = vec![slot(10, 0.0), slot(11, -1.0)];
        w.insert(
            e,
            Skin {
                influences: slots.map(|slot| Some(Influences::Whole { slot })).to_vec(),
                weights: WeightRule::SumsToOne,
                bindings: vec![Binding {
                    id: BindingId(0),
                    skeleton: sk,
                    palettes: vec![Some(palette.clone()), Some(palette)],
                    deformation: Deformation::Linear,
                }],
            },
        );
        (w, e)
    };
    let (mut w, _) = build([0, 1]);
    let err = run(&mut w, 1, 100).unwrap_err();
    assert!(err.contains("different joint"), "{err}");
    let (mut w, e) = build([1, 1]);
    run(&mut w, 1, 100).unwrap();
    let skin = w.get::<Skin>(e).unwrap();
    assert_eq!(skin.influences, [Some(Influences::Whole { slot: 1 })]);
    assert_eq!(
        skin.bindings[0].palettes[0].as_ref().unwrap(),
        &[slot(10, 0.0), slot(11, -1.0)]
    );
    assert!(w.verify_components().is_empty());
}

#[test]
fn options_are_required_and_checked() {
    let ok = Options::from([
        ("max-groups".to_owned(), "7".to_owned()),
        ("max-vertices".to_owned(), "60000".to_owned()),
    ]);
    let pass = GroupPrimitives::from_options(&ok).unwrap();
    assert_eq!((pass.max_groups(), pass.max_vertices()), (7, 60000));
    for k in ["max-groups", "max-vertices"] {
        let mut o = ok.clone();
        o.remove(k);
        assert!(GroupPrimitives::from_options(&o).unwrap_err().contains(k));
    }
    for (k, v) in [("max-groups", "0"), ("max-vertices", "2"), ("max-groups", "x")] {
        let mut o = ok.clone();
        o.insert(k.to_owned(), v.to_owned());
        assert!(GroupPrimitives::from_options(&o).is_err(), "{k}={v}");
    }
    let mut o = ok;
    o.insert("max".to_owned(), "1".to_owned());
    assert!(
        GroupPrimitives::from_options(&o)
            .unwrap_err()
            .contains("unknown option max")
    );
    // registered with its required options as the example
    let mut registry = PassRegistry::new();
    primase_meshops::register(&mut registry);
    assert!(registry.requires_options("group-primitives"));
}
