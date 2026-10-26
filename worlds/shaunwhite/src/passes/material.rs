//! `lower-shaunwhite-material`: each stored Material (`shaunwhite.material`) as a PBR material.
//!
//! The base colour is the diffuse layer (white where it has none) at its stated UV scale and offset, times the
//! Material's colour; the texture's address modes are the sampler's. The blend and alpha-test fields are the alpha
//! mode: blended is `Blend`, alpha-tested alone is `Mask` at the reference, neither is `Opaque`. Approximations, stated
//! and counted: the specular map, the normal map and the second and third layers are not lowered (the normal map's
//! green orientation is not established and how the mask weights the layers is not read); an additive Material (blend
//! 6) is drawn alpha-blended and unlit, its additive blend having no form here; every material is one-sided, rough and
//! dielectric.

use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::pbr::{AlphaMode, PbrMaterial, Shading, TextureRef, UvTransform, Wrap};
use primase_core::{Component, Entity, World};

use crate::components::{MaterialRef, MaterialState, Sampler, SlotRef};

/// The blend value of the Materials whose light adds to what is behind (`GFX_*` glows and light paint).
const ADDITIVE: u8 = 6;

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LowerShaunwhiteMaterial;

/// `lower-shaunwhite-material`'s declaration.
pub const LOWER_SHAUNWHITE_MATERIAL: PassSpec = PassSpec {
    name: "lower-shaunwhite-material",
    options: &[],
    make: |_| Ok(Box::new(LowerShaunwhiteMaterial)),
};

/// How the sampler states a texture's address along one axis, in core's: wrap is repeat, mirror is mirror, clamp is
/// clamp; border (a colour outside, none stated) is clamp and mirror-once is mirror, each counted.
fn wrap(code: u32, log: &mut PassLog) -> Wrap {
    match code {
        0 => Wrap::Repeat,
        1 => Wrap::Mirror,
        2 => Wrap::Clamp,
        3 => {
            log.approximate("border address modes drawn clamped", 1);
            Wrap::Clamp
        }
        _ => {
            log.approximate("mirror-once address modes drawn mirrored", 1);
            Wrap::Mirror
        }
    }
}

/// The slot as a texture reference: its texture, the sampler's address modes, the slot's scale and offset.
fn texture_ref(w: &World, slot: &SlotRef, log: &mut PassLog) -> TextureRef {
    let mut r = TextureRef::new(slot.texture);
    if let Some(s) = w.get::<Sampler>(slot.texture) {
        r.wrap = [wrap(s.address[0], log), wrap(s.address[1], log)];
    } else {
        log.count("textures without a sampler (repeat)", 1);
    }
    if slot.scale != [1.0; 2] || slot.offset != [0.0; 2] {
        r.transform = Some(UvTransform {
            offset: slot.offset,
            scale: slot.scale,
            rotation: 0.0,
        });
    }
    r
}

/// The alpha mode and shading the Material's fields state.
fn alpha(state: &MaterialState, log: &mut PassLog) -> (AlphaMode, Shading) {
    let mode = if state.blended {
        if state.alpha_test {
            log.approximate(
                "blended and alpha-tested materials drawn blended (the test not applied)",
                1,
            );
        }
        AlphaMode::Blend
    } else if state.alpha_test {
        AlphaMode::Mask {
            cutoff: f32::from(state.alpha_ref) / 255.0,
        }
    } else {
        AlphaMode::Opaque
    };
    let shading = if state.blended && state.blend == ADDITIVE {
        log.approximate("additive materials drawn alpha-blended and unlit", 1);
        Shading::Unlit
    } else {
        Shading::Lit
    };
    if !state.blended && state.blend != 0 {
        log.approximate(
            &format!(
                "materials of blend {} not blended (their meaning is not read)",
                state.blend
            ),
            1,
        );
    }
    (mode, shading)
}

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
        "specular and normal maps and the second and third layers not lowered; additive materials alpha-blended and \
         unlit; every material one-sided, rough and dielectric"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let all: Vec<(Entity, MaterialRef)> = w.iter::<MaterialRef>().map(|(e, m)| (e, m.clone())).collect();
        for (e, m) in all {
            let base = m.slots.get("diffuse").map(|s| texture_ref(w, s, log));
            if base.is_none() {
                log.count("materials without a diffuse texture (white)", 1);
            }
            for slot in [
                "normal",
                "specular",
                "mask",
                "diffuse_b",
                "specular_b",
                "diffuse_c",
            ] {
                if m.slots.contains_key(slot) {
                    log.omit(&format!("{slot} maps (not lowered)"), 1);
                }
            }
            for slot in &m.unread {
                log.omit(&format!("{slot} slots (not read)"), 1);
            }
            let mut pbr = PbrMaterial {
                base_color_texture: base,
                metallic: 0.0,
                roughness: 0.9,
                ..PbrMaterial::default()
            };
            match &m.state {
                Some(s) => {
                    pbr.base_color = s.color.map(|c| c.clamp(0.0, 1.0));
                    if s.color.iter().any(|c| !(0.0..=1.0).contains(c)) {
                        log.approximate("material colours outside 0-1 clamped", 1);
                    }
                    (pbr.alpha_mode, pbr.shading) = alpha(s, log);
                }
                None => log.count("materials without fields (opaque, white)", 1),
            }
            w.remove::<MaterialRef>(e);
            w.insert(e, pbr);
            log.count("materials lowered", 1);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::{ColorSpace, TexFormat, TexRole, Texture};

    fn texture(w: &mut World, address: [u32; 3]) -> Entity {
        let t = w.spawn();
        w.insert(
            t,
            Texture {
                width: 1,
                height: 1,
                format: TexFormat::Rgba8Unorm,
                levels: vec![vec![255; 4]],
                color_space: ColorSpace::Srgb,
                role: TexRole::Color,
                normal: None,
            },
        );
        w.insert(
            t,
            Sampler {
                address,
                filter: [1; 3],
            },
        );
        t
    }

    fn material(w: &mut World, state: Option<MaterialState>, diffuse: Option<SlotRef>) -> Entity {
        let m = w.spawn();
        w.insert(
            m,
            MaterialRef {
                id: 1,
                name: "m".into(),
                state,
                slots: diffuse.into_iter().map(|s| ("diffuse".to_owned(), s)).collect(),
                unread: vec!["generic".into()],
            },
        );
        m
    }

    fn state(blend: u8, blended: bool, alpha_test: bool, alpha_ref: u8) -> MaterialState {
        MaterialState {
            blend,
            blended,
            alpha_test,
            alpha_ref,
            color: [0.5, 0.25, 1.0, 1.0],
        }
    }

    /// The diffuse layer is the base colour at its scale and offset with the sampler's address modes, the colour its
    /// factor; the alpha fields choose the mode; a blended additive material is unlit.
    #[test]
    fn diffuse_tiling_colour_and_alpha_are_lowered() {
        let mut w = World::new();
        let t = texture(&mut w, [0, 2, 0]);
        let slot = SlotRef {
            texture: t,
            scale: [4.0, 2.0],
            offset: [0.5, 0.0],
        };
        let opaque = material(&mut w, Some(state(0, false, false, 128)), Some(slot));
        let tested = material(&mut w, Some(state(0, false, true, 51)), Some(slot));
        let blended = material(&mut w, Some(state(4, true, true, 128)), Some(slot));
        let additive = material(&mut w, Some(state(ADDITIVE, true, false, 128)), None);
        let bare = material(&mut w, None, None);
        let mut log = PassLog::default();
        LowerShaunwhiteMaterial.run(&mut w, &mut log).unwrap();

        let p = w.get::<PbrMaterial>(opaque).unwrap();
        assert_eq!(p.base_color, [0.5, 0.25, 1.0, 1.0]);
        let r = p.base_color_texture.unwrap();
        assert_eq!(r.texture, t);
        assert_eq!(r.wrap, [Wrap::Repeat, Wrap::Clamp]);
        let tr = r.transform.unwrap();
        assert_eq!((tr.scale, tr.offset, tr.rotation), ([4.0, 2.0], [0.5, 0.0], 0.0));
        assert_eq!(p.alpha_mode, AlphaMode::Opaque);
        assert_eq!(
            w.get::<PbrMaterial>(tested).unwrap().alpha_mode,
            AlphaMode::Mask { cutoff: 0.2 }
        );
        assert_eq!(
            w.get::<PbrMaterial>(blended).unwrap().alpha_mode,
            AlphaMode::Blend
        );
        let a = w.get::<PbrMaterial>(additive).unwrap();
        assert_eq!((a.alpha_mode, a.shading), (AlphaMode::Blend, Shading::Unlit));
        assert!(a.base_color_texture.is_none());
        let b = w.get::<PbrMaterial>(bare).unwrap();
        assert_eq!(b.base_color, [1.0; 4]);
        assert!(w.get::<MaterialRef>(bare).is_none());
        assert_eq!(log.counts.get("materials lowered"), Some(&5));
        assert_eq!(
            log.counts.get("materials without fields (opaque, white)"),
            Some(&1)
        );
        assert!(w.verify_components().is_empty());
    }
}
