//! The passes a program runs on an area the reader assembled. The reader keeps the source; each of these interprets
//! part of it, states what it approximates and logs its counts. Choices that change what is produced (which face
//! sets, which material classes, repairs) are options with the faithful reading (the source as stored) as the
//! default; a choice with no faithful reading (which model to lower, a calibration, a quality cap) is required.
//! Each pass is made by a constructor taking its required choices and setters for the rest, from its options
//! through the same calls.
pub mod bake;
pub(super) mod bake_assembly;
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
pub mod pbr;
pub(super) mod reflectance;
pub mod settings;
use crate::rigid::ApplyRigidSkinning;
pub use crate::sekiro::components::lowered::UvTiling;
pub use bake::BakeFromSoftLayers;
pub use far::DropFromSoftFar;
pub use geometry::{Cards, Degenerates, LowerFromSoftGeometry, Pose, Tiling, Winding};
pub use materials::{DropFromSoftTextureTransforms, LowerFromSoftMaterials};
use primase_core::pass::{Options, Pass, PassRegistry};

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

/// `pass`, which takes no options: an option is refused.
fn without_options(options: &Options, pass: impl Pass + 'static) -> Result<Box<dyn Pass>, String> {
    if let Some(k) = options.keys().next() {
        return Err(format!("unknown option {k}"));
    }
    Ok(Box::new(pass))
}

/// Register the FromSoft area passes by name.
pub fn register(registry: &mut PassRegistry) {
    registry.register("fromsoft-apply-rigid-skinning", |o| {
        without_options(o, ApplyRigidSkinning)
    });
    registry.register_requiring(
        "lower-fromsoft-geometry",
        |o| Ok(Box::new(LowerFromSoftGeometry::from_options(o)?)),
        &[
            ("pose", "rigid"),
            ("uv-tiling", "apply"),
            ("winding", "normals"),
            ("degenerates", "drop"),
            ("leave-out", "sky+light-volume+shadow"),
            ("blocker-cards", "record"),
        ],
    );
    registry.register("drop-fromsoft-far", |o| without_options(o, DropFromSoftFar));
    registry.register_requiring(
        "bake-fromsoft-layers",
        |o| Ok(Box::new(BakeFromSoftLayers::from_options(o)?)),
        &[("max-size", "512"), ("sample-time", "0")],
    );
    registry.register("drop-fromsoft-texture-transforms", |o| {
        without_options(o, DropFromSoftTextureTransforms::new())
    });
    registry.register("lower-fromsoft-materials", |o| {
        Ok(Box::new(LowerFromSoftMaterials::from_options(o)?))
    });
    registry.register_requiring(
        "lower-fromsoft-collision",
        |o| Ok(Box::new(collision::LowerFromSoftCollision::from_options(o)?)),
        &[
            ("hit-filters", "8+27"),
            ("water", "22+36"),
            ("leave-out", "29+40"),
            ("reach", "50"),
        ],
    );
    registry.register_requiring(
        "lower-fromsoft-object-collision",
        |o| {
            Ok(Box::new(
                object_collision::LowerFromSoftObjectCollision::from_options(o)?,
            ))
        },
        &[
            ("convex", "tessellate"),
            ("leave-out", "22+29+36+40"),
            ("leave-out-motion", "2"),
            ("convex-surface", "render"),
            ("reach", "50"),
        ],
    );
    registry.register("lower-fromsoft-extras", |o| {
        without_options(o, extras::LowerFromSoftExtras)
    });
    registry.register_requiring(
        "lower-fromsoft-lights",
        |o| Ok(Box::new(lights::LowerFromSoftLights::from_options(o)?)),
        &[("candela-per-power", "1"), ("spot-blend", "0.15")],
    );
}

#[cfg(test)]
mod tests;
