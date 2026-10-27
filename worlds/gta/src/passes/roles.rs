//! `classify-gta-texture-roles`: what GTA's shaders say a texture is, before any pass resizes or recodes it.
//!
//! A tinted shader's palette (`TintPaletteTex`) is data (`TexRole::Data`): the `_tnt` vertex shaders read it point
//! sampled at a texel indexed per vertex ([`crate::passes::tint`]), so any filtering, resizing or lossy coding changes which
//! colour a vertex gets. `lower-gta-pbr` marks it too, but only as it lowers; a program that caps textures before
//! lowering (`cap-textures`, which keeps data textures whole) runs this first. A texture some shader binds to another
//! parameter as well is left as it is (counted): its other use decides, as `lower-gta-pbr` decides it.
//!
//! The terrain blend lookups (`lookupTexture`) and the sign text's distance fields (`distanceMap`) are read filtered
//! by GTA's shaders: not data, and not marked here. Keeping them whole is a program's choice.

use std::collections::BTreeSet;

use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{Entity, TexRole, Texture, World};

use crate::components::{GtaParam, GtaShader};

/// See the module documentation.
#[derive(Debug, Default)]
pub struct ClassifyGtaTextureRoles;

/// `classify-gta-texture-roles`' declaration ([`primase_core::pass::PassRegistry::add`]): no options, a texture's role is its shaders' own.
pub const CLASSIFY_GTA_TEXTURE_ROLES: PassSpec = PassSpec {
    name: "classify-gta-texture-roles",
    options: &[],
    make: |_| Ok(Box::new(ClassifyGtaTextureRoles)),
};

impl Pass for ClassifyGtaTextureRoles {
    fn name(&self) -> &'static str {
        "classify-gta-texture-roles"
    }
    fn requires(&self) -> &[&'static str] {
        &["gta.shader"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.texture"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let palette = primase_foundations::hash::joaat(b"TintPaletteTex");
        let (mut palettes, mut other): (BTreeSet<Entity>, BTreeSet<Entity>) = Default::default();
        for (_, s) in w.iter::<GtaShader>() {
            for (name, p) in &s.params {
                if let GtaParam::Texture { texture: Some(t), .. } = p {
                    if *name == palette {
                        palettes.insert(*t);
                    } else {
                        other.insert(*t);
                    }
                }
            }
        }
        for t in palettes {
            if other.contains(&t) {
                log.count(
                    "tint palettes bound to another parameter too: left as they are",
                    1,
                );
                continue;
            }
            match w.get_mut::<Texture>(t) {
                Some(tex) if tex.role != TexRole::Data => {
                    tex.role = TexRole::Data;
                    log.count("tint palettes marked data (read point sampled per vertex)", 1);
                }
                Some(_) => log.count("tint palettes already data", 1),
                None => log.count("tint palettes without pixels", 1),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::{ColorSpace, TexFormat};

    fn texture(w: &mut World) -> Entity {
        let e = w.spawn();
        w.insert(
            e,
            Texture {
                width: 4,
                height: 4,
                format: TexFormat::Rgba8Unorm,
                levels: vec![vec![0; 64]],
                color_space: ColorSpace::Srgb,
                role: TexRole::Color,
                normal: None,
            },
        );
        e
    }

    fn shader(w: &mut World, params: &[(&str, Entity)]) {
        let e = w.spawn();
        w.insert(
            e,
            GtaShader {
                drawable: "d".into(),
                index: 0,
                name: 0,
                preset: 0,
                render_bucket: 0,
                bucket_mask: 0,
                params: params
                    .iter()
                    .map(|(n, t)| {
                        (
                            primase_foundations::hash::joaat(n.as_bytes()),
                            GtaParam::Texture {
                                name: Some((*n).to_owned()),
                                texture: Some(*t),
                            },
                        )
                    })
                    .collect(),
            },
        );
    }

    /// A palette only a palette parameter binds is data; one also bound as a diffuse map stays as it is; the
    /// lookups and distance maps GTA reads filtered are not touched.
    #[test]
    fn palettes_are_data_unless_bound_otherwise() {
        let mut w = World::new();
        let (palette, shared, diffuse, lookup) =
            (texture(&mut w), texture(&mut w), texture(&mut w), texture(&mut w));
        shader(&mut w, &[("TintPaletteTex", palette), ("DiffuseTex", diffuse)]);
        shader(&mut w, &[("TintPaletteTex", shared), ("lookupTexture", lookup)]);
        shader(&mut w, &[("DiffuseTex", shared)]);
        let mut log = PassLog::default();
        ClassifyGtaTextureRoles.run(&mut w, &mut log).unwrap();
        let role = |e| w.get::<Texture>(e).unwrap().role;
        assert_eq!(
            [role(palette), role(shared), role(diffuse), role(lookup)],
            [TexRole::Data, TexRole::Color, TexRole::Color, TexRole::Color]
        );
        assert_eq!(
            log.counts["tint palettes marked data (read point sampled per vertex)"],
            1
        );
        assert_eq!(
            log.counts["tint palettes bound to another parameter too: left as they are"],
            1
        );
    }
}
