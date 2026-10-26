//! `lower-shaunwhite-material`: each stored Material (`shaunwhite.material`) as a PBR material: its diffuse texture
//! the base colour (white where it has none), rough and dielectric. Approximations, stated and counted: the
//! specular map and the normal map are not lowered (the normal map's green orientation is not established), and
//! the material's blend and alpha-test fields are not read, so every material is opaque.

use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::{Component, Entity, World};

use crate::components::MaterialRef;

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LowerShaunwhiteMaterial;

/// `lower-shaunwhite-material`'s declaration.
pub const LOWER_SHAUNWHITE_MATERIAL: PassSpec = PassSpec {
    name: "lower-shaunwhite-material",
    options: &[],
    make: |_| Ok(Box::new(LowerShaunwhiteMaterial)),
};

impl Pass for LowerShaunwhiteMaterial {
    fn name(&self) -> &'static str {
        "lower-shaunwhite-material"
    }
    fn requires(&self) -> &[&'static str] {
        &[MaterialRef::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[PbrMaterial::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[MaterialRef::NAME]
    }
    fn approximates(&self) -> &'static str {
        "specular and normal maps not lowered; every material opaque, rough and dielectric"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let all: Vec<(Entity, MaterialRef)> = w.iter::<MaterialRef>().map(|(e, m)| (e, m.clone())).collect();
        for (e, m) in all {
            let base = m.textures.get("diffuse").copied();
            if base.is_none() {
                log.count("materials without a diffuse texture (white)", 1);
            }
            for slot in ["normal", "specular"] {
                if m.textures.contains_key(slot) {
                    log.omit(&format!("{slot} maps (not lowered)"), 1);
                }
            }
            let pbr = PbrMaterial {
                base_color_texture: base.map(TextureRef::new),
                metallic: 0.0,
                roughness: 0.9,
                ..PbrMaterial::default()
            };
            w.remove::<MaterialRef>(e);
            w.insert(e, pbr);
            log.count("materials lowered", 1);
        }
        log.approximate(
            "materials lowered opaque, rough and dielectric from their diffuse map alone",
            1,
        );
        Ok(())
    }
}
