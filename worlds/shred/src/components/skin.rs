//! Authored processed-skin data. The runtime bind/pose convention is not assigned a core meaning.
use primase_core::{Component, Entity, Geometry, Linear, Mesh, World, frame::SourceFrame, world::Linearity};
use rme_formats::skin::{SourceBone, SourceSkinVertex};

#[derive(Clone, Debug, PartialEq)]
pub struct SourceSkeleton {
    pub frame: SourceFrame,
    pub bones: Vec<SourceBone>,
}

/// Source binding data aligned with the mesh's primitives. No rest, inverse bind or deformation is invented.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceSkin {
    pub frame: SourceFrame,
    pub skeleton: Entity,
    pub primitives: Vec<Vec<SourceSkinVertex>>,
}

impl Component for SourceSkeleton {
    const NAME: &'static str = "shred.source_skeleton";
    const GEOMETRY: Geometry = Geometry::None;
    fn apply_linear(&self, _: &Linear) -> Linearity<Self> {
        // These remain the original source records, interpreted in the recorded source frame.
        Linearity::Invariant
    }
    fn payload_bytes(&self) -> u64 {
        self.bones.iter().map(|b| 56 + b.name.len() as u64).sum()
    }
    fn verify(&self, _: Entity, _: &World) -> Result<(), String> {
        if self.bones.is_empty() {
            return Err("empty source skeleton".into());
        }
        for bone in &self.bones {
            if bone.matrix.cols.iter().flatten().any(|x| !x.is_finite()) {
                return Err("nonfinite source bone matrix".into());
            }
        }
        Ok(())
    }
}

impl Component for SourceSkin {
    const NAME: &'static str = "shred.source_skin";
    const GEOMETRY: Geometry = Geometry::PerVertex;
    fn apply_linear(&self, _: &Linear) -> Linearity<Self> {
        // Original binding provenance; this does not assert a world-space deformation under geometry maps.
        Linearity::Invariant
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.skeleton);
    }
    fn payload_bytes(&self) -> u64 {
        self.primitives.iter().map(|v| v.len() as u64 * 32).sum()
    }
    fn verify(&self, e: Entity, world: &World) -> Result<(), String> {
        let skeleton = world
            .get::<SourceSkeleton>(self.skeleton)
            .ok_or("source skeleton missing")?;
        let mesh = world.get::<Mesh>(e).ok_or("source skin without mesh")?;
        if self.primitives.len() != mesh.primitives().len() {
            return Err("source skin primitive count differs from mesh".into());
        }
        for (vertices, primitive) in self.primitives.iter().zip(mesh.primitives()) {
            if vertices.len() != primitive.vertex_count() {
                return Err("source skin vertex count differs from mesh".into());
            }
            for vertex in vertices {
                if vertex.weights.iter().map(|&x| u16::from(x)).sum::<u16>() != 255 {
                    return Err("source skin weights do not sum to 255".into());
                }
                for (&bone, &weight) in vertex.bones.iter().zip(&vertex.weights) {
                    if (bone == 255 && weight != 0)
                        || (bone != 255 && usize::from(bone) >= skeleton.bones.len())
                    {
                        return Err("source skin bone reference out of range".into());
                    }
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
            let vertices = self
                .primitives
                .get(primitive)
                .ok_or("source skin primitive out of range")?;
            let from: Vec<_> = from.iter().map(|&i| i as usize).collect();
            let mapped = primase_core::remap_list(vertices, &from)?;
            let mut out = self.clone();
            out.primitives[primitive] = mapped;
            Ok(out)
        })())
    }
}
