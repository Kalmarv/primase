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
