//! Stateful pose simulations, exposed by world components independently of the engine that implements them.
//!
//! A reader attaches a [`Simulation`] to its definition and [`SimulationVertices`] to meshes it drives. A client
//! starts an independent runner for each posed instance, supplies model-space joints and world-space skinned meshes,
//! and steps at its chosen fixed timestep before drawing driven joints and vertex overrides. Restarting from
//! [`SimulationSource::start`] resets a runner; pre-roll uses ordinary steps at the pose that will first be shown.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use crate::deform::Vertices;
use crate::pose::Affine;
use primase_core::{Component, Entity, JointId, Mesh, Skeleton, VertexBlend, World};

/// A simulation definition in a world. The implementation is supplied by its reader, never chosen by the client.
#[derive(Clone, Debug)]
pub struct Simulation {
    /// The skeleton whose model-space joints the simulation follows.
    pub skeleton: Entity,
    /// An immutable factory; every posed instance starts its own mutable runner.
    pub source: Arc<dyn SimulationSource>,
    /// Vertex count of each output, for validating mesh bindings without inspecting an engine's layout.
    pub outputs: Vec<u32>,
}

impl Simulation {
    pub fn new(skeleton: Entity, source: Arc<dyn SimulationSource>, outputs: Vec<u32>) -> Self {
        Self {
            skeleton,
            source,
            outputs,
        }
    }
}

impl Component for Simulation {
    const NAME: &'static str = "animation.simulation";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.skeleton);
    }
    fn apply_linear(&self, _: &primase_core::Linear) -> primase_core::Linearity<Self> {
        primase_core::Linearity::Invariant
    }
    fn verify(&self, _: Entity, world: &World) -> Result<(), String> {
        if world.has::<Skeleton>(self.skeleton) {
            Ok(())
        } else {
            Err("simulation skeleton has no core.skeleton".into())
        }
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(&serde_json::json!({
            "name": self.source.name(), "skeleton": self.skeleton, "outputs": self.outputs
        }))
        .map(Some)
    }
}

/// A simulation as the world states it, for one skeleton: what starts runners of it.
pub trait SimulationSource: fmt::Debug + Send + Sync {
    /// What it is called (the cloth's file, its source's name).
    fn name(&self) -> String;
    /// A runner of it on a rig of `skeleton`, at rest. `joints` takes the joints of the skeleton the world states the
    /// simulation for to `skeleton`'s, where the rig is another skeleton's (a part's simulation on the character its
    /// parts were assembled into); `None` when `skeleton` is the stated one.
    fn start(
        &self,
        skeleton: &Skeleton,
        joints: Option<&BTreeMap<JointId, JointId>>,
    ) -> Result<Box<dyn JointSimulation>, String>;
}

/// A mesh as the rig poses it without the simulation, which a step reads as output `output`'s vertices.
#[derive(Clone, Copy, Debug)]
pub struct MeshInput<'a> {
    /// The simulation's output the mesh shows.
    pub output: usize,
    /// For each vertex of the mesh, its vertex in the output.
    pub vertices: &'a [u32],
    /// The mesh's vertices posed, in the world.
    pub posed: &'a Vertices,
}

/// A running simulation on one rig.
pub trait JointSimulation: Send {
    /// One step of `seconds`: read the rig's joints in `model` (each in the skeleton's space; the rig placed in the world
    /// by `placement`) and the meshes it moves as posed (`meshes`), and set the joints it drives there.
    fn step(
        &mut self,
        placement: &Affine,
        model: &mut BTreeMap<JointId, Affine>,
        meshes: &[MeshInput<'_>],
        seconds: f32,
    ) -> Result<(), String>;
    /// Set the joints it drives in `model` to where its last step left them, without stepping (nothing before a step).
    fn hold(&self, placement: &Affine, model: &mut BTreeMap<JointId, Affine>);
    /// The joints it drives.
    fn driven(&self) -> &BTreeSet<JointId>;
    /// What its last step left output `output` with, in the world, by output vertex (positions, normals and tangents,
    /// a tangent's `w` not stated); `None` before a step and for an output it does not have.
    fn output(&self, output: usize) -> Option<&Vertices>;
}

/// `animation.simulation_vertices`: the output vertex each vertex of a mesh primitive follows.
/// `None` leaves a primitive on its ordinary deformation path. Reordering and copying preserve the mapping;
/// interpolating several vertices is refused because it cannot select one simulation output vertex.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimulationVertices {
    /// The entity holding the simulation.
    pub simulation: Entity,
    pub primitives: Vec<Option<SimulationPrimitive>>,
}

/// One primitive's simulation output and vertex mapping ([`SimulationVertices`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimulationPrimitive {
    /// An index into [`Simulation::outputs`].
    pub output: u32,
    /// For each vertex of the primitive, its vertex in the output.
    pub vertices: Vec<u32>,
}

impl SimulationVertices {
    /// `self` with primitive `primitive`'s vertices made `vertex`'s for each new vertex (an error for a vertex the
    /// primitive does not have); itself for a primitive no output holds.
    fn gather(
        &self,
        primitive: usize,
        vertex: impl Fn(usize) -> Result<usize, String>,
        count: usize,
    ) -> Result<Self, String> {
        let entry = self
            .primitives
            .get(primitive)
            .ok_or_else(|| format!("primitive {primitive} of {}", self.primitives.len()))?;
        let Some(p) = entry else {
            return Ok(self.clone());
        };
        let vertices = (0..count)
            .map(|i| {
                let old = vertex(i)?;
                p.vertices
                    .get(old)
                    .copied()
                    .ok_or_else(|| format!("vertex {old} of {}", p.vertices.len()))
            })
            .collect::<Result<Vec<u32>, String>>()?;
        let mut out = self.clone();
        out.primitives[primitive] = Some(SimulationPrimitive {
            output: p.output,
            vertices,
        });
        Ok(out)
    }
}

impl Component for SimulationVertices {
    const NAME: &'static str = "animation.simulation_vertices";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::PerVertex;
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.simulation);
    }
    fn payload_bytes(&self) -> u64 {
        let vertices: usize = self.primitives.iter().flatten().map(|p| p.vertices.len()).sum();
        u64::try_from(vertices).map_or(u64::MAX, |n| n.saturating_mul(4))
    }
    /// Vertex indices, not local-space geometry.
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::Linearity<Self> {
        primase_core::Linearity::Invariant
    }
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(
            primase_core::remap_list(&self.primitives, from).map(|primitives| SimulationVertices {
                simulation: self.simulation,
                primitives,
            }),
        )
    }
    fn remap_vertices(&self, primitive: usize, from: &[u32]) -> Option<Result<Self, String>> {
        Some(self.gather(primitive, |i| Ok(from[i] as usize), from.len()))
    }
    /// A copy of one old vertex is that vertex; a vertex blended from two or three is refused.
    fn blend_vertices(&self, primitive: usize, from: &[VertexBlend]) -> Option<Result<Self, String>> {
        Some(self.gather(
            primitive,
            |i| {
                let mut terms = from[i].terms();
                match (terms.next(), terms.next()) {
                    (Some((v, _)), None) => Ok(v as usize),
                    _ => Err(format!(
                        "new vertex {i} is blended from several vertices: no one cloth vertex is it"
                    )),
                }
            },
            from.len(),
        ))
    }
    /// One mapping per primitive, one output index per mapped vertex, and every output vertex in range.
    fn verify(&self, e: Entity, w: &World) -> std::result::Result<(), String> {
        let Some(mesh) = w.get::<Mesh>(e) else {
            return Ok(());
        };
        let simulation = w
            .get::<Simulation>(self.simulation)
            .ok_or_else(|| format!("{:?} holds no animation.simulation", self.simulation))?;
        let primitives = mesh.primitives();
        if primitives.len() != self.primitives.len() {
            return Err(format!(
                "{} primitives, {} simulation entries",
                primitives.len(),
                self.primitives.len()
            ));
        }
        for (i, (p, c)) in primitives.iter().zip(&self.primitives).enumerate() {
            let Some(c) = c else {
                continue;
            };
            if c.vertices.len() != p.vertex_count() {
                return Err(format!(
                    "primitive {i}: {} vertices, {} simulation vertices",
                    p.vertex_count(),
                    c.vertices.len()
                ));
            }
            let count = *simulation
                .outputs
                .get(c.output as usize)
                .ok_or_else(|| format!("primitive {i}: no simulation output {}", c.output))?;
            if let Some(v) = c.vertices.iter().find(|&&v| v >= count) {
                return Err(format!(
                    "primitive {i}: simulation output vertex {v} of {}",
                    count
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Falling;

    impl SimulationSource for Falling {
        fn name(&self) -> String {
            "falling output".into()
        }
        fn start(
            &self,
            _: &Skeleton,
            _: Option<&BTreeMap<JointId, JointId>>,
        ) -> Result<Box<dyn JointSimulation>, String> {
            Ok(Box::new(Fall {
                stepped: false,
                driven: BTreeSet::new(),
                vertices: Vertices {
                    positions: vec![[0.0; 3]],
                    normals: None,
                    tangents: None,
                    further_tangents: BTreeMap::new(),
                },
            }))
        }
    }

    struct Fall {
        stepped: bool,
        driven: BTreeSet<JointId>,
        vertices: Vertices,
    }
    impl JointSimulation for Fall {
        fn step(
            &mut self,
            _: &Affine,
            _: &mut BTreeMap<JointId, Affine>,
            _: &[MeshInput<'_>],
            seconds: f32,
        ) -> Result<(), String> {
            self.vertices.positions[0][1] -= seconds;
            self.stepped = true;
            Ok(())
        }
        fn hold(&self, _: &Affine, _: &mut BTreeMap<JointId, Affine>) {}
        fn driven(&self) -> &BTreeSet<JointId> {
            &self.driven
        }
        fn output(&self, output: usize) -> Option<&Vertices> {
            (self.stepped && output == 0).then_some(&self.vertices)
        }
    }

    #[test]
    fn a_world_exposes_independent_simulations_without_an_engine_type() {
        let mut world = World::new();
        let skeleton = world.spawn();
        let joints = Skeleton { joints: Vec::new() };
        world.insert(skeleton, joints.clone());
        let definition = world.spawn();
        world.insert(definition, Simulation::new(skeleton, Arc::new(Falling), vec![1]));
        let simulation = world.get::<Simulation>(definition).unwrap();
        assert_eq!(simulation.verify(definition, &world), Ok(()));
        let mut first = simulation.source.start(&joints, None).unwrap();
        let second = simulation.source.start(&joints, None).unwrap();
        let identity = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0., 0., 0.]];
        let mut model = BTreeMap::new();
        first.step(&identity, &mut model, &[], 0.25).unwrap();
        assert_eq!(first.output(0).unwrap().positions[0], [0., -0.25, 0.]);
        first.hold(&identity, &mut model);
        assert_eq!(first.output(0).unwrap().positions[0], [0., -0.25, 0.]);
        assert!(second.output(0).is_none());
        let restarted = simulation.source.start(&joints, None).unwrap();
        assert!(restarted.output(0).is_none());
    }
}
