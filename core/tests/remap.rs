//! Components that hold something per mesh primitive follow a pass that rebuilds the primitives
//! ([`World::remap_primitives`]); the others are left as they are.

#![allow(clippy::unwrap_used)]

use primase_core::{Component, MaterialVariants, Name, World, remap_list};
use std::collections::BTreeMap;

/// A dialect's per-primitive data, as a reader would type it.
#[derive(Clone, Debug, PartialEq)]
struct Sources(Vec<&'static str>);
impl Component for Sources {
    const NAME: &'static str = "test.sources";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::PerPrimitive;
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(remap_list(&self.0, from).map(Sources))
    }
}

#[test]
fn per_primitive_components_follow_split_reordered_and_dropped_primitives() {
    let mut w = World::new();
    let m = w.spawn();
    let e = w.spawn();
    w.insert(e, Sources(vec!["a", "b", "c"]));
    w.insert(
        e,
        MaterialVariants(vec![BTreeMap::new(), BTreeMap::from([(1, m)]), BTreeMap::new()]),
    );
    w.insert(e, Name("kept".into()));
    // primitive 1 split in two, 0 after it, 2 dropped
    w.remap_primitives(e, &[1, 1, 0]).unwrap();
    assert_eq!(w.get::<Sources>(e).unwrap().0, ["b", "b", "a"]);
    assert_eq!(
        w.get::<MaterialVariants>(e).unwrap().0,
        [
            BTreeMap::from([(1, m)]),
            BTreeMap::from([(1, m)]),
            BTreeMap::new()
        ]
    );
    // a component that does not follow primitives is unchanged
    assert_eq!(w.get::<Name>(e).unwrap().0, "kept");
    // an entity with none of them: nothing to do
    let bare = w.spawn();
    w.remap_primitives(bare, &[0, 0]).unwrap();
}

#[test]
fn an_index_a_component_lacks_is_refused_and_changes_nothing() {
    let mut w = World::new();
    let e = w.spawn();
    w.insert(e, Sources(vec!["a", "b", "c"]));
    // two primitives' variants for a three-primitive mesh: already out of step
    w.insert(e, MaterialVariants(vec![BTreeMap::new(); 2]));
    let err = w.remap_primitives(e, &[2, 0]).unwrap_err();
    assert_eq!(err, "core.material_variants: primitive 2 of 2");
    // all or nothing: the component that could follow did not
    assert_eq!(w.get::<Sources>(e).unwrap().0, ["a", "b", "c"]);
    assert_eq!(w.get::<MaterialVariants>(e).unwrap().0.len(), 2);
}

#[test]
fn rebuild_primitives_changes_the_mesh_and_its_per_primitive_data_together() {
    use primase_core::{Mesh, Primitive, Topology};
    let prim = |material: Option<primase_core::Entity>| Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::new(),
        indices: None,
        material,
    };
    let mut w = World::new();
    let (a, b) = (w.spawn(), w.spawn());
    let e = w.spawn();
    w.insert(e, Mesh::new(vec![prim(Some(a)), prim(Some(b))]));
    w.insert(e, Sources(vec!["a", "b"]));
    // primitive 1 split in two, primitive 0 dropped
    w.rebuild_primitives(e, |m| {
        let p = m.primitives()[1].clone();
        Ok((vec![p.clone(), p], vec![1, 1]))
    })
    .unwrap();
    assert_eq!(w.get::<Mesh>(e).unwrap().primitives().len(), 2);
    assert_eq!(w.get::<Sources>(e).unwrap().0, ["b", "b"]);
    // a source past the old list, or a count that disagrees: refused, nothing changed
    let before = w.get::<Mesh>(e).unwrap().clone();
    let err = w
        .rebuild_primitives(e, |m| Ok((m.primitives().to_vec(), vec![0, 5])))
        .unwrap_err();
    assert_eq!(err, "source primitive 5 of 2");
    let err = w
        .rebuild_primitives(e, |m| Ok((m.primitives().to_vec(), vec![0])))
        .unwrap_err();
    assert_eq!(err, "2 primitives with 1 sources");
    assert_eq!(w.get::<Mesh>(e), Some(&before));
    assert_eq!(w.get::<Sources>(e).unwrap().0, ["b", "b"]);
    // a component that cannot follow: refused by name, nothing changed
    w.insert(e, Sources(vec!["only one"]));
    let err = w
        .rebuild_primitives(e, |m| Ok((m.primitives().to_vec(), vec![0, 1])))
        .unwrap_err();
    assert!(err.starts_with("test.sources: "), "{err}");
    assert_eq!(w.get::<Mesh>(e), Some(&before));
}

/// A component that follows primitives only while their count stays: a split or a drop it refuses.
#[derive(Clone, Debug, PartialEq)]
struct Fixed(Vec<u32>);
impl Component for Fixed {
    const NAME: &'static str = "test.fixed";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::PerPrimitive;
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(if from.len() == self.0.len() {
            remap_list(&self.0, from).map(Fixed)
        } else {
            Err("a fixed count of primitives".into())
        })
    }
}

fn prim(material: Option<primase_core::Entity>, x: f32) -> primase_core::Primitive {
    use primase_core::{Primitive, Semantic, Stream, Topology};
    Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([(
            Semantic::Position,
            Stream::F32x3(vec![[x, 0., 0.], [x, 1., 0.], [x, 0., 1.]]),
        )]),
        indices: None,
        material,
    }
}

/// edit_mesh is all or nothing: a component refusing a valid mapping, a failing closure that already edited the
/// mesh, or a malformed mapping leave the mesh and its per-primitive components as they were, and usable; an accepted
/// rebuild moves them together.
#[test]
fn edit_mesh_keeps_the_mesh_and_its_components_consistent_on_every_failure() {
    use primase_core::Mesh;
    let mut w = World::new();
    let (a, b) = (w.spawn(), w.spawn());
    let e = w.spawn();
    let original = Mesh::new(vec![prim(Some(a), 0.), prim(Some(b), 1.)]);
    w.insert(e, original.clone());
    w.insert(e, Sources(vec!["a", "b"]));
    w.insert(e, Fixed(vec![10, 20]));
    let unchanged = |w: &World| {
        assert_eq!(w.get::<Mesh>(e), Some(&original));
        assert_eq!(w.get::<Sources>(e).unwrap().0, ["a", "b"]);
        assert_eq!(w.get::<Fixed>(e).unwrap().0, [10, 20]);
    };
    // a component refusing a mapping with valid indices: the split is undone, the mesh is not lost
    let err = w
        .edit_mesh(e, |m| {
            let p = m.primitives()[1].clone();
            *m = Mesh::new(vec![p.clone(), p.clone(), p]);
            Ok(Some(vec![1, 1, 1]))
        })
        .unwrap_err();
    assert_eq!(err, "test.fixed: a fixed count of primitives");
    unchanged(&w);
    // the closure fails after editing in place: the edit is not published
    let err = w
        .edit_mesh(e, |m| {
            *m.primitive_mut(0).unwrap().material_mut() = None;
            Err("gave up".to_owned())
        })
        .unwrap_err();
    assert_eq!(err, "gave up");
    unchanged(&w);
    // a mapping naming a primitive that never was
    assert!(w.edit_mesh(e, |_| Ok(Some(vec![0, 5]))).is_err());
    unchanged(&w);
    // still usable: a reorder every component follows
    assert!(
        w.edit_mesh(e, |m| {
            let p = m.primitives().to_vec();
            *m = Mesh::new(vec![p[1].clone(), p[0].clone()]);
            Ok(Some(vec![1, 0]))
        })
        .unwrap()
    );
    assert_eq!(w.get::<Mesh>(e).unwrap().primitives()[0].material, Some(b));
    assert_eq!(w.get::<Sources>(e).unwrap().0, ["b", "a"]);
    assert_eq!(w.get::<Fixed>(e).unwrap().0, [20, 10]);
}

/// A primitive edited in place stays in its place: its per-primitive components still describe it.
#[test]
fn a_primitive_edited_in_place_keeps_its_components() {
    use primase_core::{Mesh, Semantic};
    let mut w = World::new();
    let (a, b) = (w.spawn(), w.spawn());
    let e = w.spawn();
    w.insert(e, Mesh::new(vec![prim(Some(a), 0.), prim(Some(b), 1.)]));
    w.insert(e, Sources(vec!["a", "b"]));
    let m = w.get_mut::<Mesh>(e).unwrap();
    for mut p in m.primitives_mut() {
        p.streams_mut().remove(&Semantic::Position);
    }
    *m.primitive_mut(1).unwrap().material_mut() = Some(a);
    assert!(m.primitive_mut(2).is_none());
    let m = w.get::<Mesh>(e).unwrap();
    assert_eq!(m.primitives().len(), 2);
    assert!(m.primitives().iter().all(|p| p.streams.is_empty()));
    assert_eq!(m.primitives()[1].material, Some(a));
    assert_eq!(w.get::<Sources>(e).unwrap().0, ["a", "b"]);
}

/// Opaque data can't follow a change to the primitives: a reorder, split or drop is refused before anything
/// changes, naming it; keeping the primitives as they are is allowed.
#[test]
fn opaque_components_refuse_a_primitive_change_before_anything_changes() {
    use primase_core::{Mesh, Opaque, OpaqueData};
    let mut w = World::new();
    let (a, b) = (w.spawn(), w.spawn());
    let e = w.spawn();
    let original = Mesh::new(vec![prim(Some(a), 0.), prim(Some(b), 1.)]);
    w.insert(e, original.clone());
    w.insert(e, Sources(vec!["a", "b"]));
    let blob = Opaque {
        version: 1,
        data: OpaqueData::Bytes(vec![1, 2]),
    };
    w.insert_opaque(e, "other.per_primitive", blob.clone());
    for from in [vec![1, 0], vec![0, 0, 1], vec![1]] {
        let err = w.remap_primitives(e, &from).unwrap_err();
        assert!(err.contains("other.per_primitive"), "{err}");
        assert_eq!(w.get::<Sources>(e).unwrap().0, ["a", "b"]);
        let n = from.len();
        let err = w
            .edit_mesh(e, |m| {
                let p = m.primitives().to_vec();
                *m = Mesh::new(from.iter().map(|&i| p[i].clone()).collect());
                Ok(Some(from.clone()))
            })
            .unwrap_err();
        assert!(err.contains("other.per_primitive"), "{n}: {err}");
        assert_eq!(w.get::<Mesh>(e), Some(&original));
        assert_eq!(w.get_opaque(e, "other.per_primitive"), Some(&blob));
    }
    // the identity keeps everything where it is
    w.remap_primitives(e, &[0, 1]).unwrap();
    // removed first, the reorder goes through
    w.remove_opaque(e, "other.per_primitive");
    w.remap_primitives(e, &[1, 0]).unwrap();
    assert_eq!(w.get::<Sources>(e).unwrap().0, ["b", "a"]);
}
