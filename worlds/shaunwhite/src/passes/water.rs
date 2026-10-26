//! Stored water materials as core water, with the authored mesh, tint and placement unchanged. Run after collision
//! lowering (to classify source Water surfaces) and before generic material lowering. Static shading is an explicit
//! approximation: the source's wave normal, reflection cube, specular model and animation are not reproduced.

use primase_core::pass::{OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{Component, MaterialUsage, SurfaceClass, Surfaces, World};

use crate::components::MaterialRef;

/// Static preview roughness is the program's choice, not a field decoded from the game.
#[derive(Clone, Copy, Debug)]
pub struct LowerShaunwhiteWater {
    pub roughness: f32,
}

pub const LOWER_SHAUNWHITE_WATER: PassSpec = PassSpec {
    name: "lower-shaunwhite-water",
    options: &[OptionSpec::required(
        "roughness",
        "0.05",
        "static PBR preview roughness in 0..1; source wave shading is not reproduced",
    )],
    make: |g| {
        let roughness: f32 = g
            .required("roughness")
            .parse()
            .map_err(|_| "roughness must be a number")?;
        if !(roughness.is_finite() && (0.0..=1.0).contains(&roughness)) {
            return Err("roughness must be finite and in 0..1".into());
        }
        Ok(Box::new(LowerShaunwhiteWater { roughness }))
    },
};

/// A source surface name's shared meaning. The game names this SWCollisionMaterial Water; Bail/RobotBail,
/// InvisibleWall, Unsafe and NonRidable are kept distinct. No shared surface class encodes a respawn/death volume.
pub const WATER_SURFACES: &[&str] = &["Water"];

impl Pass for LowerShaunwhiteWater {
    fn name(&self) -> &'static str {
        "lower-shaunwhite-water"
    }
    fn options(&self) -> Options {
        Options::from([("roughness".into(), self.roughness.to_string())])
    }
    fn requires(&self) -> &[&'static str] {
        &[MaterialRef::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[PbrMaterial::NAME, MaterialUsage::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[MaterialRef::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.surfaces"]
    }
    fn approximates(&self) -> &'static str {
        "static water tint with chosen roughness; source wave normals, reflections, specular shading and animation omitted"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        if !(self.roughness.is_finite() && (0.0..=1.0).contains(&self.roughness)) {
            return Err("roughness must be finite and in 0..1".into());
        }
        let water: Vec<_> = w
            .iter::<MaterialRef>()
            .filter(|(_, m)| m.state.is_some_and(|s| s.water))
            .map(|(e, m)| (e, m.clone()))
            .collect();
        for (e, m) in water {
            let state = m.state.ok_or("water material lost its source state")?;
            w.insert(
                e,
                PbrMaterial {
                    base_color: state.color,
                    metallic: 0.,
                    roughness: self.roughness,
                    alpha_mode: if state.blended {
                        AlphaMode::Blend
                    } else {
                        AlphaMode::Opaque
                    },
                    ..PbrMaterial::default()
                },
            );
            w.insert(e, MaterialUsage::Water);
            w.remove::<MaterialRef>(e);
            log.count("stored water shader flags lowered", 1);
            for name in m.slots.keys().chain(m.unread.iter()) {
                log.omit(&format!("water {name} slot (source shading not reproduced)"), 1);
            }
            log.approximate("water as static authored tint with chosen roughness", 1);
        }
        if let Some(surfaces) = w.resource_mut::<Surfaces>() {
            for s in &mut surfaces.0 {
                if WATER_SURFACES.contains(&s.name.as_str()) {
                    s.class = Some(SurfaceClass::Water);
                    log.count("source Water collision surfaces classified", 1);
                }
            }
        }
        Ok(())
    }
}
