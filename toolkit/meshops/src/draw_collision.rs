//! `draw-collision`: every entity with `core.collision` and no `core.mesh` gets a mesh drawing its collision soup:
//! one triangle primitive, its positions and triangles, no material. A diagnostic: it makes what collides visible to a
//! writer that draws meshes but not collision (a glTF to inspect), and is reported as such. The collision stays.

use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Collision, Component, Entity, Mesh, Primitive, Semantic, Stream, Topology, World};

/// See the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrawCollision;

impl Pass for DrawCollision {
    fn name(&self) -> &'static str {
        "draw-collision"
    }
    fn options(&self) -> Options {
        Options::new()
    }
    fn requires(&self) -> &[&'static str] {
        &[Collision::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Mesh::NAME]
    }
    fn approximates(&self) -> &'static str {
        "collision drawn as plain meshes, to inspect it"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let soups: Vec<(Entity, Collision)> = w
            .iter::<Collision>()
            .filter(|(e, _)| !w.has::<Mesh>(*e))
            .map(|(e, c)| (e, c.clone()))
            .collect();
        for (e, c) in soups {
            let triangles = c.triangles.len() as u64;
            let mut streams = BTreeMap::new();
            streams.insert(Semantic::Position, Stream::F32x3(c.positions));
            w.insert(
                e,
                Mesh::new(vec![Primitive {
                    topology: Topology::Triangles,
                    streams,
                    indices: Some(c.triangles.into_iter().flatten().collect()),
                    material: None,
                }]),
            );
            log.approximate("collision soups drawn as meshes (a diagnostic)", 1);
            log.count("collision triangles drawn", triangles);
        }
        Ok(())
    }
}
