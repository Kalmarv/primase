//! A prototype's FLVER model as the reader keeps it, and as `fromsoft-apply-rigid-skinning` posed it.
use fromsoft_formats::flver::Model;
use primase_core::Component;

/// A prototype's FLVER as the file stores it (`fromsoft.flver`): skeleton, every mesh with its face sets (levels of
/// detail and motion-blur variants), original material indices and every vertex channel, in the file's frame.
#[derive(Clone, Debug)]
pub struct SourceModel(pub Model);
impl Component for SourceModel {
    const NAME: &'static str = "fromsoft.flver";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// The meshes' vertices and indices ([`Model::resident_bytes`]): the largest thing an area read holds, which a
    /// world's size estimate (the lab's reset point) must count or it takes a multi-gigabyte area for a small one.
    fn payload_bytes(&self) -> u64 {
        self.0.resident_bytes()
    }
}
/// The same model statically posed by [`lower_model`] (`fromsoft.rigid_model`), beside the source it came from.
#[derive(Clone, Debug)]
pub struct RigidModel(pub Model);
impl Component for RigidModel {
    const NAME: &'static str = "fromsoft.rigid_model";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// As [`SourceModel`]'s: the posed copy beside the source.
    fn payload_bytes(&self) -> u64 {
        self.0.resident_bytes()
    }
}

/// The skeleton a prototype's model is bound to (`fromsoft.skeleton`, on the prototype): the entity holding the
/// `core.skeleton` the reader made of its FLVER's bones, which `lower-fromsoft-geometry` binds its skinned meshes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SkeletonOf(pub primase_core::Entity);
impl Component for SkeletonOf {
    const NAME: &'static str = "fromsoft.skeleton";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn references(&self, out: &mut Vec<primase_core::Entity>) {
        out.push(self.0);
    }
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::Linearity<Self> {
        primase_core::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
    /// The entity holds a `core.skeleton`.
    fn verify(&self, _e: primase_core::Entity, w: &primase_core::World) -> Result<(), String> {
        if w.has::<primase_core::Skeleton>(self.0) {
            Ok(())
        } else {
            Err(format!("{:?} holds no core.skeleton", self.0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fromsoft_formats::flver::{FaceSet, Mesh};

    /// The world size estimate counts a model's indices (a model with no vertices is its faces alone).
    #[test]
    fn a_model_states_the_bytes_its_meshes_hold() {
        let model = Model {
            bones: Vec::new(),
            dummies: Vec::new(),
            version: 0x2001a,
            speedtree: false,
            layouts: Vec::new(),
            materials: Vec::new(),
            meshes: vec![Mesh {
                dynamic: false,
                material: 0,
                node: -1,
                palette: Vec::new(),
                layouts: Vec::new(),
                vertices: Vec::new(),
                faces: vec![FaceSet {
                    flags: 0,
                    strip: false,
                    cull: true,
                    unknown_06: 0,
                    indices: vec![0, 1, 2, 2, 1, 3],
                }],
            }],
        };
        assert_eq!(model.resident_bytes(), 24);
        assert_eq!(SourceModel(model.clone()).payload_bytes(), 24);
        assert_eq!(RigidModel(model).payload_bytes(), 24);
    }
}
