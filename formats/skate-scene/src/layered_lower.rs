//! Explicit strongest-triangle approximation for PBR consumers of layered scenes.
use crate::SkatePrototype;
use primase_core::World;
use primase_core::pass::{Pass, PassLog};

/// Requires loaded meshes and ordinary layers lowered to PBR. Every triangle
/// chooses the largest sum of corner weights, with (1-r) omitted for
/// rgb_product_4, as the reference skate writer does. No contour splitting.
/// Null slots choose opaque black. Logs triangles per slot, not per material.
#[derive(Debug, Default)]
pub struct LowerLayeredPbr;
impl Pass for LowerLayeredPbr {
    fn name(&self) -> &'static str {
        "lower-layered-pbr"
    }
    fn requires(&self) -> &[&'static str] {
        &["core.layered", "core.mesh", "pbr.material"]
    }
    fn provides(&self) -> &[&'static str] {
        &["pbr.material"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh", "core.material_variants", "skate.prototype"]
    }
    fn removes(&self) -> &[&'static str] {
        &["core.mesh_file"]
    }
    fn approximates(&self) -> &'static str {
        "strongest summed corner layer per triangle; no per-pixel blend, darkening or contour splitting"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut prototypes = Vec::new();
        primase_meshops::lower_layered_pbr(
            world,
            primase_meshops::SelectionPolicy::OmitRgbProduct4Darkening,
            log,
            |world, sources| {
                // Prepare every dialect update before the generic changes are applied.
                for source in sources {
                    if let Some(original) = world.get::<SkatePrototype>(source.entity) {
                        if original.parts.len() != source.primitive_count {
                            return Err(primase_meshops::Error(
                                "scene part metadata differs from primitives".into(),
                            ));
                        }
                        let mut prototype = original.clone();
                        prototype.parts = source
                            .parts
                            .iter()
                            .map(|part| original.parts[part.primitive].clone())
                            .collect();
                        prototypes.push((source.entity, prototype));
                    }
                }
                Ok(())
            },
        )
        .map_err(|e| e.to_string())?;
        for (entity, prototype) in prototypes {
            world.insert(entity, prototype);
        }
        Ok(())
    }
}
