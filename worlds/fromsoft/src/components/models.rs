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
