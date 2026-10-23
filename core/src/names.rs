//! Every component and resource core declares, by the name each declares for itself ([`Component::NAME`],
//! [`Resource::NAME`]): what a coverage report lists, whether or not a reader, pass or writer mentions it. The types
//! are listed once here; an architecture test holds the list to every `Component` and `Resource` core implements.

use crate::asset::Asset;
use crate::clip::{Clip, ClipBinding};
use crate::collider::{Collider, Heightfield};
use crate::components::{
    Collision, Instance, Light, MaterialVariants, Mesh, MeshFile, Name, Shadows, Source, Surfaces, Texture,
    TextureFile, Tile, Transform,
};
use crate::convert::{PartScope, PartitionPlan};
use crate::draw_order::DrawOrder;
use crate::environment::Environment;
use crate::layered::LayeredMaterial;
use crate::lod::Lods;
use crate::marker::{Curve, Marker};
use crate::material_usage::MaterialUsage;
use crate::morphs::Morphs;
use crate::parallel::Threads;
use crate::pbr::PbrMaterial;
use crate::priority::Priority;
use crate::provenance::Provenance;
use crate::rig::{
    Attachment, JointMap, JointPlacement, MorphWeights, RigInstance, RigPose, SkinInstance, Sockets,
};
use crate::shader::{Shader, ShaderBinding};
use crate::skeleton::Skeleton;
use crate::skin::Skin;
use crate::state::{Condition, PrimitiveCondition, StateAssignment, StateAxis, StateOverride};
use crate::texture_layers::TextureLayers;
use crate::vertex_color::VertexColor;
use crate::world::{Component, Resource};

/// What a name of core's names: a component (on entities) or a resource (one per world).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Declared {
    Component,
    Resource,
}

macro_rules! declared {
    (components: $($c:ident),* $(,)?; resources: $($r:ident),* $(,)?) => {
        /// Every component and resource core declares, by its name, in the order listed.
        pub fn names() -> Vec<(&'static str, Declared)> {
            vec![
                $((<$c as Component>::NAME, Declared::Component),)*
                $((<$r as Resource>::NAME, Declared::Resource),)*
            ]
        }

        /// The types [`names`] lists, by their names in the source (for the architecture test that holds the list
        /// to core's implementations).
        pub const LISTED: &[&str] = &[$(stringify!($c),)* $(stringify!($r),)*];
    };
}

declared! {
    components: Collider, Heightfield, Name, Source, Transform, Instance, Shadows, Mesh, Texture, Light, Collision,
        TextureFile, MeshFile, MaterialVariants, Tile, DrawOrder, LayeredMaterial, Lods, Marker, Curve, MaterialUsage,
        PbrMaterial, Priority, Provenance, StateAxis, Condition, PrimitiveCondition, StateOverride, VertexColor, Skeleton, Skin, Morphs, RigInstance,
        RigPose, SkinInstance, MorphWeights, Sockets, Attachment, JointMap, JointPlacement, Clip, ClipBinding, Asset, Shader, ShaderBinding,
        TextureLayers;
    resources: PartScope, PartitionPlan, Environment, StateAssignment, Surfaces, Threads,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each name is listed once, and every listed type is a distinct name.
    #[test]
    fn every_name_once() {
        let names = names();
        let set: std::collections::BTreeSet<&str> = names.iter().map(|(n, _)| *n).collect();
        assert_eq!(set.len(), names.len());
        assert_eq!(names.len(), LISTED.len());
        assert!(
            set.contains("core.mesh") && set.contains("pbr.material") && set.contains("core.environment")
        );
    }
}
