//! Anvil's lowerings: its dialect to core meanings, each choice the program's.

use primase_core::pass::{Options, Pass, PassLog, PassRegistry};
use primase_core::world::Resource;
use primase_core::{Collision, Component, Entity, Surfaces, World};

use crate::components::AnvilMeshShape;

/// `lower-anvil-collision{surface=NAME}`: every MeshShape's geometry ([`AnvilMeshShape`]) becomes `core.collision` on
/// its prototype, every triangle of the surface `surface` names (unclassified: what a MeshShape's surfaces are isn't
/// established, so the program names one, and its class is left unstated). The geometry is consumed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowerAnvilCollision {
    pub surface: String,
}

impl LowerAnvilCollision {
    /// From its options: `surface`, required.
    pub fn from_options(o: &Options) -> Result<LowerAnvilCollision, String> {
        if let Some(k) = o.keys().find(|k| k.as_str() != "surface") {
            return Err(format!("unknown option {k}"));
        }
        let surface = o
            .get("surface")
            .filter(|s| !s.is_empty())
            .ok_or("surface=NAME is required: what a MeshShape's surfaces are isn't established")?;
        Ok(LowerAnvilCollision {
            surface: surface.clone(),
        })
    }
}

impl Pass for LowerAnvilCollision {
    fn name(&self) -> &'static str {
        "lower-anvil-collision"
    }
    fn options(&self) -> Options {
        Options::from([("surface".to_owned(), self.surface.clone())])
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilMeshShape::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Collision::NAME, Surfaces::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Surfaces::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilMeshShape::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let shapes: Vec<Entity> = w.iter::<AnvilMeshShape>().map(|(e, _)| e).collect();
        if shapes.is_empty() {
            return Ok(());
        }
        let mut surfaces = w.remove_resource::<Surfaces>().unwrap_or_default();
        let surface = surfaces.index(&self.surface).map_err(|e| e.to_string())?;
        for e in shapes {
            let Some(shape) = w.remove::<AnvilMeshShape>(e) else {
                continue;
            };
            let triangles = shape.faces.len();
            w.insert(
                e,
                Collision {
                    positions: shape.positions,
                    triangles: shape.faces,
                    surfaces: vec![surface; triangles],
                },
            );
            log.count("MeshShapes lowered to collision", 1);
            log.count(
                &format!("collision triangles of surface {}", self.surface),
                triangles as u64,
            );
        }
        w.insert_resource(surfaces);
        Ok(())
    }
}

/// Register Anvil's passes by name.
pub fn register(reg: &mut PassRegistry) {
    reg.register_requiring(
        "lower-anvil-collision",
        |o| Ok(Box::new(LowerAnvilCollision::from_options(o)?)),
        &[("surface", "rr-diagnostic-unknown")],
    );
}
