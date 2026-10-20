//! draw-collision: a collision soup without a mesh drawn as one plain primitive; an entity with a mesh left alone.
use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{Collision, Mesh, Primitive, Semantic, Stream, Surface, Surfaces, Topology, World};
use primase_meshops::DrawCollision;

#[test]
fn a_soup_without_a_mesh_is_drawn_and_one_with_a_mesh_is_left() {
    let mut w = World::new();
    w.insert_resource(Surfaces(vec![Surface::new("default")]));
    let soup = Collision {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
        triangles: vec![[0, 1, 2], [2, 1, 3]],
        surfaces: vec![0, 0],
    };
    let bare = w.spawn();
    w.insert(bare, soup.clone());
    let drawn = w.spawn();
    w.insert(drawn, soup);
    let mut streams = std::collections::BTreeMap::new();
    streams.insert(Semantic::Position, Stream::F32x3(vec![[5.0, 5.0, 5.0]; 3]));
    let own = Mesh::new(vec![Primitive {
        topology: Topology::Triangles,
        streams,
        indices: None,
        material: None,
    }]);
    w.insert(drawn, own.clone());
    let manager = PassManager::new(PassRegistry::new()).with_pass_checks(true);
    let pipeline = Pipeline {
        passes: vec![Box::new(DrawCollision)],
    };
    let mut report = RunReport::default();
    manager.run(&mut w, &pipeline, &mut report).unwrap();
    let m = w.get::<Mesh>(bare).unwrap();
    let p = &m.primitives()[0];
    assert_eq!(p.indices.as_deref(), Some(&[0, 1, 2, 2, 1, 3][..]));
    assert_eq!(p.positions().unwrap().len(), 4);
    assert_eq!(p.material, None);
    assert!(w.has::<Collision>(bare));
    assert_eq!(w.get::<Mesh>(drawn), Some(&own));
    let log = &report.steps[0].log;
    assert_eq!(log.counts["collision triangles drawn"], 2);
    assert_eq!(
        log.approximations["collision soups drawn as meshes (a diagnostic)"],
        1
    );
}
