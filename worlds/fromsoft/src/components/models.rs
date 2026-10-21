//! A prototype's FLVER model as the reader keeps it, and as `fromsoft-apply-rigid-skinning` posed it.
use crate::flver::Model;
use primase_core::Component;

/// A prototype's FLVER as the file stores it (`fromsoft.flver`): skeleton, every mesh with its face sets (levels of
/// detail and motion-blur variants), original material indices and every vertex channel, in the file's frame.
#[derive(Clone, Debug)]
pub struct SourceModel(pub Model);
impl Component for SourceModel {
    const NAME: &'static str = "fromsoft.flver";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}
/// The same model statically posed by [`lower_model`] (`fromsoft.rigid_model`), beside the source it came from.
#[derive(Clone, Debug)]
pub struct RigidModel(pub Model);
impl Component for RigidModel {
    const NAME: &'static str = "fromsoft.rigid_model";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
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
