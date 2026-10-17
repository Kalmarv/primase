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
