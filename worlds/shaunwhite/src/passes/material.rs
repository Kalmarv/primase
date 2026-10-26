//! `lower-shaunwhite-material`: each stored Material (`shaunwhite.material`) as a PBR material.
//!
//! The base colour is the diffuse layer (white where it has none) at its stated UV scale and offset, times the
//! Material's colour; the texture's address modes are the sampler's. The blend and alpha-test fields are the alpha
//! mode: blended is `Blend`, alpha-tested alone is `Mask` at the reference, neither is `Opaque`. The specular power
//! (an exponent n) is the roughness `(2 / (n + 2))^(1/4)` (the Blinn-Phong lobe's width as a GGX roughness); the
//! specular level, colour and map are glTF's specular factor, colour and colour texture; the illumination is the
//! diffuse layer as emission at that strength. Approximations, stated and counted: the specular map, a Blinn-Phong
//! colour, can darken glTF's dielectric highlight and not brighten it. A Material `lower-shaunwhite-layers` made a
//! blend (core.layered, before this pass) is drawn by its layers and takes no PBR material of its own. Otherwise the
//! normal map is not lowered (its green orientation is not established), and a Material holding a mask, second or third
//! layer without a blend falls back to its diffuse layer, marked to ignore its vertex
//! colour (a layered Material's vertex colours are saturated and mostly not grey, 58% of 497,307 vertices of level 01,
//! against 13% of 801,135 of the others: weights, which drawn as a colour paint magenta and green across rocks and
//! roofs), so it draws as its diffuse layer; an additive Material (blend 6) is drawn alpha-blended and unlit, its
//! additive blend having no form here; every material is one-sided and dielectric.
//!
//! No Material takes `COLOR_0` as a colour. The shader (`ekgeneric.fx` `FctDiffuseColor`) uses its red as the weight
//! that interpolates the first texture towards that texture times the tint, which `lower-shaunwhite-layers` builds; a
//! white tint leaves the texture whatever the weight. Only its alpha reaches the output, as the vertex alpha of the
//! blend or test. So every Material is marked [`VertexColor::Ignored`], except one that is blended or alpha-tested
//! and has a vertex alpha below one: core has no mode that takes the alpha alone, so it keeps `COLOR_0` as a
//! multiplier, its red, green and blue then approximate, and the pass counts it.

use std::collections::BTreeSet;

use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::pbr::{AlphaMode, PbrMaterial, Shading, Specular, TextureRef, UvTransform, Wrap};
use primase_core::{
    Component, Entity, MaterialVariants, Mesh, Primitive, Semantic, Stream, VertexColor, World,
};

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
pub(super) fn texture_ref(w: &World, slot: &SlotRef, log: &mut PassLog) -> TextureRef {
    let mut r = TextureRef::new(slot.texture);
    if let Some(s) = w.get::<Sampler>(slot.texture) {
        r.wrap = [wrap(s.address[0], log), wrap(s.address[1], log)];
    } else {
        log.count("textures without a sampler (repeat)", 1);
    }
    r.texcoord = slot.texcoord;
    if let Some(address) = slot.address {
        r.wrap = address.map(|mode| match mode {
            0 => Wrap::Repeat,
            1 => Wrap::Clamp,
            _ => {
                log.approximate("material map border address drawn clamped", 1);
                Wrap::Clamp
            }
        });
    }
    if slot.rotation != 0. {
        log.omit("authored TexCoordRotation (matrix convention not lowered)", 1);
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

/// Every material some drawing of `primitive` (the `index`th of `mesh`) uses: its own and its variants'.
pub(super) fn drawn_with(w: &World, mesh: Entity, index: usize, primitive: &Primitive) -> Vec<Entity> {
    primitive
        .material
        .into_iter()
        .chain(
            w.get::<MaterialVariants>(mesh)
                .and_then(|v| v.0.get(index))
                .into_iter()
                .flat_map(|v| v.values().copied()),
        )
        .collect()
}

/// The materials some drawing of which has a vertex whose `COLOR_0` alpha is below one.
fn with_vertex_alpha(w: &World) -> BTreeSet<Entity> {
    let mut out = BTreeSet::new();
    for (e, mesh) in w.iter::<Mesh>() {
        for (i, p) in mesh.primitives().iter().enumerate() {
            let varies = match p.streams.get(&Semantic::Color(0)) {
                Some(Stream::F32x4(c)) => c.iter().any(|v| v[3] < 1.),
                Some(Stream::U8x4(c)) => c.iter().any(|v| v[3] < u8::MAX),
                Some(Stream::U16x4(c)) => c.iter().any(|v| v[3] < u16::MAX),
                _ => false,
            };
            if varies {
                out.extend(drawn_with(w, e, i, p));
            }
        }
    }
    out
}

/// The roughness of a Blinn-Phong exponent `power`: the lobe's GGX roughness `sqrt(2 / (n + 2))`, as glTF's perceptual
/// roughness (its square root), within 0.05 to 1.
fn roughness(power: f32) -> f32 {
    (2.0 / (power.max(0.0) + 2.0)).powf(0.25).clamp(0.05, 1.0)
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
        &[PbrMaterial::NAME, VertexColor::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[MaterialRef::NAME]
    }
    fn approximates(&self) -> &'static str {
        "PBR specular approximates the source, normal maps omitted; layered sources without core.layered use the diffuse fallback; additive materials alpha-blended and \
         unlit; every material one-sided, rough and dielectric"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let all: Vec<(Entity, MaterialRef)> = w.iter::<MaterialRef>().map(|(e, m)| (e, m.clone())).collect();
        let vertex_alpha = with_vertex_alpha(w);
        for (e, m) in all {
            // a blend (core.layered, lower-shaunwhite-layers') is drawn by its layers, each a material of its own: it
            // takes no pbr.material itself (core's and the targets' rule)
            if w.has::<primase_core::LayeredMaterial>(e) {
                w.remove::<MaterialRef>(e);
                log.count("blends drawn by their layers (no material of their own)", 1);
                continue;
            }
            let base = m.slots.get("diffuse").map(|s| texture_ref(w, s, log));
            let specular_map = m.slots.get("specular").map(|s| texture_ref(w, s, log));
            if base.is_none() {
                log.count("materials without a diffuse texture (white)", 1);
            }
            let layered = ["mask", "diffuse_b", "diffuse_c"]
                .iter()
                .any(|k| m.slots.contains_key(*k));
            if layered {
                log.approximate(
                    "layered materials drawn as their diffuse layer, vertex colour ignored",
                    1,
                );
            }
            for slot in ["normal", "mask", "diffuse_b", "specular_b", "diffuse_c"] {
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
            if let Some(map) = specular_map {
                log.count("specular maps lowered (colour texture)", 1);
                pbr.specular = Some(Specular {
                    color_texture: Some(map),
                    ..Specular::default()
                });
            }
            match &m.state {
                Some(s) => {
                    pbr.base_color = s.color.map(|c| c.clamp(0.0, 1.0));
                    if s.color.iter().any(|c| !(0.0..=1.0).contains(c)) {
                        log.approximate("material colours outside 0-1 clamped", 1);
                    }
                    (pbr.alpha_mode, pbr.shading) = alpha(s, log);
                    pbr.roughness = roughness(s.specular_power);
                    let level = s.specular_level.clamp(0.0, 1.0);
                    let color = [1.; 3]; // Body 332 is Layer2_ModulationColor, not a specular colour.
                    if level != 1.0 || color != [1.0; 3] || pbr.specular.is_some() {
                        let specular = pbr.specular.get_or_insert_with(Specular::default);
                        specular.factor = level;
                        specular.color = color;
                    }
                    if s.illumination > 0.0 {
                        pbr.emissive = [0, 1, 2].map(|k| pbr.base_color.get(k).copied().unwrap_or(1.0));
                        pbr.emissive_texture = pbr.base_color_texture;
                        pbr.emissive_strength = s.illumination;
                        log.count("materials lowered as emissive (illumination)", 1);
                    }
                }
                None => log.count("materials without fields (opaque, white)", 1),
            }
            if layered || pbr.alpha_mode == AlphaMode::Opaque || !vertex_alpha.contains(&e) {
                w.insert(e, VertexColor::Ignored);
            } else {
                log.approximate_on(
                    e,
                    "blended or alpha-tested materials keep COLOR_0 as a colour for its alpha (the shader takes only the alpha)",
                    1,
                );
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
            water: false,
            color: [0.5, 0.25, 1.0, 1.0],
            illumination: 0.0,
            specular_level: 1.0,
            specular_power: 2.0,
            layering: crate::components::LayerState::default(),
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
            rotation: 0.,
            texcoord: 0,
            address: None,
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

    /// The specular power is the roughness, the level the specular factor, the map its colour
    /// texture, the illumination the diffuse layer as emission; a default specular states nothing. The layer-2 tint
    /// does not colour the specular highlight.
    #[test]
    fn specular_and_illumination_are_lowered() {
        let mut w = World::new();
        let t = texture(&mut w, [0, 0, 0]);
        let slot = SlotRef {
            texture: t,
            scale: [1.0; 2],
            offset: [0.0; 2],
            rotation: 0.,
            texcoord: 0,
            address: None,
        };
        let mut s = state(0, false, false, 128);
        s.color = [1.0; 4];
        s.specular_power = 30.0;
        s.specular_level = 0.5;
        s.layering.colors[0] = [1.0, 0.5, 0.25, 1.0];
        s.illumination = 6.0;
        let lit = material(&mut w, Some(s), Some(slot));
        w.get_mut::<MaterialRef>(lit)
            .unwrap()
            .slots
            .insert("specular".into(), slot);
        let mut plain = state(0, false, false, 128);
        plain.specular_power = 2.0;
        plain.specular_level = 1.0;
        plain.layering.colors[0] = [1.0; 4];
        let plain = material(&mut w, Some(plain), Some(slot));
        let layered = material(&mut w, Some(state(0, false, false, 128)), Some(slot));
        w.get_mut::<MaterialRef>(layered)
            .unwrap()
            .slots
            .insert("diffuse_b".into(), slot);
        let mut log = PassLog::default();
        LowerShaunwhiteMaterial.run(&mut w, &mut log).unwrap();
        assert_eq!(w.get::<VertexColor>(layered), Some(&VertexColor::Ignored));

        let p = w.get::<PbrMaterial>(lit).unwrap();
        assert!((p.roughness - 0.5).abs() < 1e-6, "{}", p.roughness);
        let spec = p.specular.unwrap();
        assert_eq!((spec.factor, spec.color), (0.5, [1.0; 3]));
        assert_eq!(spec.color_texture.map(|r| r.texture), Some(t));
        assert_eq!((p.emissive, p.emissive_strength), ([1.0; 3], 6.0));
        assert_eq!(p.emissive_texture, p.base_color_texture);
        assert_eq!(w.get::<VertexColor>(lit), Some(&VertexColor::Ignored));
        let q = w.get::<PbrMaterial>(plain).unwrap();
        assert!(q.specular.is_none() && q.emissive_texture.is_none() && q.emissive_strength == 1.0);
        assert!((q.roughness - 0.8409).abs() < 1e-3);
        assert!(w.verify_components().is_empty());
    }
}
