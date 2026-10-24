//! The passes a program runs on an area the reader assembled. The reader keeps the source; each of these interprets
//! part of it, states what it approximates and logs its counts. Choices that change what is produced (which face
//! sets, which material classes, repairs) are options with the faithful reading (the source as stored) as the
//! default; a choice with no faithful reading (which model to lower, a calibration, a quality cap) is required.
//! Each pass is made by a constructor taking its required choices and setters for the rest, from its options
//! through the same calls.
pub mod bake;
pub(super) mod bake_assembly;
pub mod bake_materials;
pub mod blockers;
pub mod collision;
pub mod extras;
pub mod far;
pub mod geometry;
pub mod layers;
pub mod lights;
pub mod materials;
pub mod normals;
pub mod object_collision;
pub mod outfits;
pub mod pbr;
pub(super) mod reflectance;
pub mod resolve_shaders;
pub mod settings;
pub use crate::layer_bake::resolve::Unbound;
pub use crate::sekiro::components::lowered::UvTiling;
pub use bake::BakeFromSoftLayers;
pub use far::DropFromSoftFar;
pub use geometry::{Cards, Degenerates, LowerFromSoftGeometry, Pose, SpeedTreeFrame, Tiling, Winding};
pub use materials::{DropFromSoftTextureTransforms, LowerFromSoftMaterials};
use primase_core::pass::PassRegistry;
pub use resolve_shaders::ResolveFromSoftShaders;

/// A condition holding when any of the clauses holds; None (always present) when any input is None.
pub(crate) fn any_of(
    clauses: &[Option<primase_core::state::Clause>],
) -> Option<primase_core::state::Condition> {
    let mut out: Vec<primase_core::state::Clause> = Vec::new();
    for c in clauses {
        let c = c.clone()?;
        if !out.contains(&c) {
            out.push(c);
        }
    }
    // one order for the same set of clauses
    out.sort_by_key(|c| c.0.iter().map(|l| (l.axis, l.values)).collect::<Vec<_>>());
    Some(primase_core::state::Condition(out))
}

/// A condition's stable text, for grouping soups by condition.
pub(crate) fn condition_key(c: &primase_core::state::Condition) -> String {
    c.0.iter()
        .map(|cl| {
            cl.0.iter()
                .map(|l| format!("{:?}:{:x}", l.axis, l.values))
                .collect::<Vec<_>>()
                .join("&")
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// Register the FromSoft area passes by their declarations.
pub fn register(registry: &mut PassRegistry) {
    for spec in [
        &crate::rigid::FROMSOFT_APPLY_RIGID_SKINNING,
        &geometry::LOWER_FROMSOFT_GEOMETRY,
        &outfits::RAISE_FROMSOFT_DISPLAY_MASKS,
        &outfits::SELECT_FROMSOFT_OUTFIT,
        &far::DROP_FROMSOFT_FAR,
        &resolve_shaders::RESOLVE_FROMSOFT_SHADERS,
        &bake::BAKE_FROMSOFT_LAYERS,
        &bake_materials::BAKE_FROMSOFT_MATERIALS,
        &materials::DROP_FROMSOFT_TEXTURE_TRANSFORMS,
        &materials::LOWER_FROMSOFT_MATERIALS,
        &collision::LOWER_FROMSOFT_COLLISION,
        &object_collision::LOWER_FROMSOFT_OBJECT_COLLISION,
        &extras::LOWER_FROMSOFT_EXTRAS,
        &lights::LOWER_FROMSOFT_LIGHTS,
    ] {
        registry.add(spec);
    }
}

#[cfg(test)]
mod tests;
