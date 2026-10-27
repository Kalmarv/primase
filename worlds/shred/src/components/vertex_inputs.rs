//! Source vertex inputs whose shader-specific meaning is not assigned a core semantic.
use primase_core::{Component, Entity, Geometry, Linear, Mesh, World, frame::SourceFrame, world::Linearity};
use rme_formats::attributes::VertexInput;
use std::collections::BTreeMap;

/// Original shader inputs per primitive. `stored_uv0..3` retain all authored UV sets even when a submesh
/// remaps or does not bind them; `attr14/15` are the two CMP basis vectors (individual tangent roles unproven);
/// `attr12/13` are the tree half4 attributes. These are source-frame provenance, not world-space core vectors.
#[derive(Clone, Debug, PartialEq)]
pub struct VertexInputs {
    pub frame: SourceFrame,
    pub primitives: Vec<BTreeMap<String, VertexInput>>,
}

impl Component for VertexInputs {
    const NAME: &'static str = "shred.vertex_inputs";
    const GEOMETRY: Geometry = Geometry::PerVertex;
    fn apply_linear(&self, _: &Linear) -> Linearity<Self> {
        // Original source values remain provenance under a later map of the decoded geometry.
        Linearity::Invariant
    }
    fn payload_bytes(&self) -> u64 {
        self.primitives
            .iter()
            .flat_map(|p| p.values())
            .map(|a| a.values.len() as u64 * 16)
            .sum()
    }
    fn verify(&self, e: Entity, world: &World) -> Result<(), String> {
        let mesh = world.get::<Mesh>(e).ok_or("vertex inputs without mesh")?;
        if self.primitives.len() != mesh.primitives().len() {
            return Err("vertex-input primitive count differs from mesh".into());
        }
        for (inputs, primitive) in self.primitives.iter().zip(mesh.primitives()) {
            for (name, input) in inputs {
                if !(1..=4).contains(&input.lanes) || input.values.len() != primitive.vertex_count() {
                    return Err(format!("{name}: invalid lane or vertex count"));
                }
                if input.values.iter().flatten().any(|v| !v.is_finite()) {
                    return Err(format!("{name}: nonfinite source input"));
                }
            }
        }
        Ok(())
    }
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(
            primase_core::remap_list(&self.primitives, from).map(|primitives| Self {
                primitives,
                ..self.clone()
            }),
        )
    }
    fn remap_vertices(&self, primitive: usize, from: &[u32]) -> Option<Result<Self, String>> {
        Some((|| {
            let inputs = self
                .primitives
                .get(primitive)
                .ok_or_else(|| format!("primitive {primitive} out of range"))?;
            let mut mapped = BTreeMap::new();
            for (name, input) in inputs {
                let values = from
                    .iter()
                    .map(|&i| {
                        input
                            .values
                            .get(i as usize)
                            .copied()
                            .ok_or_else(|| format!("{name}: vertex {i} out of range"))
                    })
                    .collect::<Result<_, _>>()?;
                mapped.insert(
                    name.clone(),
                    VertexInput {
                        lanes: input.lanes,
                        values,
                    },
                );
            }
            let mut out = self.clone();
            out.primitives[primitive] = mapped;
            Ok(out)
        })())
    }
}
