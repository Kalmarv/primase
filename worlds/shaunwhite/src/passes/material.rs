//! `lower-shaunwhite-material`: each stored Material (`shaunwhite.material`) as a PBR material.
//!
//! The base colour is the diffuse layer (white where it has none) at its stated UV scale and offset, times the
//! Material's colour; the texture's address modes are the sampler's. The blend and alpha-test fields are the alpha
//! mode: blended is `Blend`, alpha-tested alone is `Mask` at the reference, neither is `Opaque`. The specular power
//! (an exponent n) is the roughness `(2 / (n + 2))^(1/4)` (the Blinn-Phong lobe's width as a GGX roughness); the
//! specular level, colour and map are glTF's specular factor, colour and colour texture; the illumination is the
//! diffuse layer as emission at that strength. Approximations, stated and counted: the specular map, a Blinn-Phong
//! colour, can darken glTF's dielectric highlight and not brighten it. A Material `lower-shaunwhite-layers` made a
//! blend (core.layered, before this pass) is drawn by its layers and takes no PBR material of its own, and its normal
//! map is omitted (counted). The normal map of any other Material is core's normal texture with the scale of its
//! `HeightScale`, its channels the shader's ([`crate::read::texture::NORMAL_CONVENTION`]), drawn with the mesh's own
//! tangents. A Material holding a mask, second or third
//! layer without a blend falls back to its diffuse layer, marked to ignore its vertex
//! colour (a layered Material's vertex colours are saturated and mostly not grey, 58% of 497,307 vertices of level 01,
//! against 13% of 801,135 of the others: weights, which drawn as a colour paint magenta and green across rocks and
//! roofs), so it draws as its diffuse layer; an additive Material (blend 6) is drawn alpha-blended and unlit, its
//! additive blend having no form here; every material is dielectric. A Material stored TwoSided (`read::material`: the
//! bit the native update turns into D3DRS_CULLMODE `NONE`) is `double_sided`, every other one-sided. InvertCulling,
//! which reverses the culled winding, is read but not lowered: it is set on two Materials, both TwoSided, so no
//! drawn surface has it alone; a Material that did would be counted as omitted.
//!
//! The tint's colour space is the texture's stored one. The game never decodes a texture: no effect sets the sampler
//! state `D3DSAMP_SRGBTEXTURE` (11) or `D3DRS_SRGBWRITEENABLE`, the engine's cached SetSamplerState only ever stated type 11 as 0
//! (304 calls of 40,001 in one level), and the shipped pixel shaders have no transfer, so `texture * tint` multiplies stored
//! values whether or not the texture is flagged sRGB. Core's sRGB texture is decoded to linear before the base colour factor
//! multiplies it, so the tint of a Material whose diffuse texture is flagged sRGB is decoded the same way
//! (`decode(texel) * decode(tint) = decode(texel * tint)` for a power-law transfer; the sRGB curve's linear toe differs
//! slightly near black). The alpha, the tint of a linear-flagged diffuse texture (both sides multiply stored values) and the
//! colour of a Material without a diffuse texture (its encoding is not established) are left as stored.
//!
//! No Material takes `COLOR_0` as a colour. The shader (`ekgeneric.fx` `FctDiffuseColor`) uses its red as the weight
//! that interpolates the first texture towards that texture times the tint, which `lower-shaunwhite-layers` builds; a
//! white tint leaves the texture whatever the weight. Only its alpha reaches the output, as the vertex alpha of the
//! blend or test. So every Material is marked [`VertexColor::Ignored`], except one that is blended or alpha-tested
//! and has a vertex alpha below one: core has no mode that takes the alpha alone, so it keeps `COLOR_0` as a
//! multiplier, its red, green and blue then approximate, and the pass counts it.
//! `lower-shaunwhite-vertex-alpha` then rewrites those streams to the alpha alone (red, green and blue 1).

use std::collections::BTreeSet;

use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::pbr::{AlphaMode, PbrMaterial, Shading, Specular, TextureRef, UvTransform, Wrap};
use primase_core::{
    ColorSpace, Component, Entity, MaterialVariants, Mesh, Primitive, Semantic, Stream, Texture, VertexColor,
    World,
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

/// One channel of the sRGB transfer's decode, stored 0..1 to linear 0..1.
fn srgb_decode(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
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
    match uv_transform(slot) {
        Some(t) => r.transform = (t != UvTransform::IDENTITY).then_some(t),
        None => {
            log.omit(
                "authored TexCoordRotation with unequal scale (a transform core's rotation after scale cannot state)",
                1,
            );
            if slot.scale != [1.0; 2] || slot.offset != [0.0; 2] {
                r.transform = Some(UvTransform {
                    offset: slot.offset,
                    scale: slot.scale,
                    rotation: 0.0,
                });
            }
        }
    }
    r
}

/// The native UV setter's matrix of a slot as core's [`UvTransform`]: the game rotates the coordinates by the stored
/// degrees about the UV origin, then scales and offsets them (`uv' = S R(a) uv + o`), while core scales, then rotates
/// (`uv' = R(r) (S' uv) + o`). The two agree where the matrix's columns are orthogonal (a uniform scale, or a quarter
/// turn): `None` where they do not, which core states only as two transforms in a row (a layer's, then the slot's: `lower-shaunwhite-layers`).
pub(super) fn uv_transform(slot: &SlotRef) -> Option<UvTransform> {
    if slot.rotation == 0. {
        return Some(UvTransform {
            offset: slot.offset,
            scale: slot.scale,
            rotation: 0.0,
        });
    }
    let (sin, cos) = radians(slot.rotation).sin_cos();
    let [sx, sy] = slot.scale.map(f64::from);
    let (sin, cos) = (f64::from(sin), f64::from(cos));
    let first = [sx * cos, sy * sin];
    let second = [-sx * sin, sy * cos];
    let (n1, n2) = (first[0].hypot(first[1]), second[0].hypot(second[1]));
    let dot = first[0] * second[0] + first[1] * second[1];
    if n1 == 0. || n2 == 0. || dot.abs() > 1e-6 * n1 * n2 {
        return None;
    }
    // columns n1 (cos b, sin b) and q (-sin b, cos b): q is signed, so a mirrored matrix is a negative scale
    let b = first[1].atan2(first[0]);
    let q = -second[0] * b.sin() + second[1] * b.cos();
    #[allow(
        clippy::cast_possible_truncation,
        reason = "f64 results of f32 inputs, rounded once"
    )]
    let t = UvTransform {
        offset: slot.offset,
        scale: [n1 as f32, q as f32],
        rotation: b as f32,
    };
    Some(t)
}

/// A stored rotation in degrees as radians, in the native setter's way (a double product, rounded to f32 once).
pub(super) fn radians(degrees: f32) -> f32 {
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the native setter stores the f64 product as f32"
    )]
    let r = (f64::from(degrees) * std::f64::consts::PI / 180.0) as f32;
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

/// The materials some drawing of which has no tangent stream.
fn without_tangent(w: &World) -> BTreeSet<Entity> {
    let mut out = BTreeSet::new();
    for (e, mesh) in w.iter::<Mesh>() {
        for (i, p) in mesh.primitives().iter().enumerate() {
            if !p.streams.contains_key(&Semantic::Tangent(0)) {
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
        "PBR specular approximates the source, normal maps of blends omitted; layered sources without core.layered use the diffuse fallback; additive materials alpha-blended and \
         unlit; blended or alpha-tested materials with a vertex alpha below one keep COLOR_0 as a colour multiplier, as core has no alpha-only mode; InvertCulling without TwoSided not lowered (none in the game's data); \
         every material rough and dielectric; the tint of an sRGB-flagged diffuse texture decoded per channel with the sRGB curve (the game multiplies stored values, exact for a power-law transfer)"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let all: Vec<(Entity, MaterialRef)> = w.iter::<MaterialRef>().map(|(e, m)| (e, m.clone())).collect();
        let vertex_alpha = with_vertex_alpha(w);
        let untangented = without_tangent(w);
        for (e, m) in all {
            // a blend (core.layered, lower-shaunwhite-layers') is drawn by its layers, each a material of its own: it
            // takes no pbr.material itself (core's and the targets' rule)
            if w.has::<primase_core::LayeredMaterial>(e) {
                w.remove::<MaterialRef>(e);
                log.count("blends drawn by their layers (no material of their own)", 1);
                if m.slots.contains_key("normal") {
                    log.omit("normal maps of blends drawn by their layers", 1);
                }
                continue;
            }
            let base = m.slots.get("diffuse").map(|s| texture_ref(w, s, log));
            let diffuse_srgb = m.slots.get("diffuse").is_some_and(|s| {
                w.get::<Texture>(s.texture)
                    .is_some_and(|t| t.color_space == ColorSpace::Srgb)
            });
            let specular_map = m.slots.get("specular").map(|s| texture_ref(w, s, log));
            let normal_map = m.slots.get("normal").map(|s| texture_ref(w, s, log));
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
            for slot in ["mask", "diffuse_b", "specular_b", "diffuse_c"] {
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
            if let Some(map) = normal_map {
                pbr.normal_texture = Some(map);
                log.count("normal maps lowered (X in alpha, Y in green, Z rebuilt)", 1);
                if untangented.contains(&e) {
                    log.approximate(
                        "normal maps drawn on primitives without a tangent stream (the consumer derives the frame)",
                        1,
                    );
                }
            }
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
                    if diffuse_srgb && pbr.base_color[..3].iter().any(|c| *c != 1.0) {
                        for c in &mut pbr.base_color[..3] {
                            *c = srgb_decode(*c);
                        }
                        log.count("tints of sRGB-flagged diffuse textures decoded to linear (the game multiplies stored values)", 1);
                    }
                    (pbr.alpha_mode, pbr.shading) = alpha(s, log);
                    pbr.double_sided = s.two_sided;
                    if s.two_sided {
                        log.count("materials lowered two-sided (TwoSided)", 1);
                    } else if s.invert_culling {
                        log.omit("InvertCulling without TwoSided (winding not reversed)", 1);
                    }
                    pbr.roughness = roughness(s.specular_power);
                    if pbr.normal_texture.is_some() {
                        pbr.normal_scale = s.normal_scale;
                    }
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
            two_sided: false,
            invert_culling: false,
            color: [0.5, 0.25, 1.0, 1.0],
            illumination: 0.0,
            specular_level: 1.0,
            specular_power: 2.0,
            normal_scale: 1.,
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
        // the test texture is flagged sRGB: its tint is decoded (0.5 -> 0.21404, 0.25 -> 0.05088), white and alpha stay
        for (got, want) in p.base_color.iter().zip([0.214_04, 0.050_876, 1.0, 1.0]) {
            assert!((got - want).abs() < 1e-5, "{:?}", p.base_color);
        }
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

    /// The game multiplies stored texel values by the tint whatever the texture's gamma flag, and core multiplies the
    /// decoded texel of an sRGB texture by the factor: only the tint of an sRGB-flagged diffuse texture is decoded, its
    /// alpha, a white tint, a linear-flagged texture's tint and an untextured colour stay as stored, and the emission
    /// (the tinted diffuse layer) follows the base colour.
    #[test]
    fn the_tint_is_decoded_only_for_srgb_flagged_diffuse_textures() {
        let mut w = World::new();
        let srgb = texture(&mut w, [0, 0, 0]);
        let linear = texture(&mut w, [0, 0, 0]);
        w.get_mut::<Texture>(linear).unwrap().color_space = ColorSpace::Linear;
        let slot = |texture| SlotRef {
            texture,
            scale: [1.0; 2],
            offset: [0.0; 2],
            rotation: 0.,
            texcoord: 0,
            address: None,
        };
        let tinted = |alpha, illumination| MaterialState {
            color: [0.5, 1.0, 0.04, alpha],
            illumination,
            ..state(0, false, false, 128)
        };
        let on_srgb = material(&mut w, Some(tinted(0.7, 2.0)), Some(slot(srgb)));
        let on_linear = material(&mut w, Some(tinted(0.7, 0.0)), Some(slot(linear)));
        let untextured = material(&mut w, Some(tinted(0.7, 0.0)), None);
        let white = material(
            &mut w,
            Some(MaterialState {
                color: [1.0; 4],
                ..state(0, false, false, 128)
            }),
            Some(slot(srgb)),
        );
        let mut log = PassLog::default();
        LowerShaunwhiteMaterial.run(&mut w, &mut log).unwrap();

        let decoded = [0.214_04, 1.0, 0.04 / 12.92, 0.7];
        let p = w.get::<PbrMaterial>(on_srgb).unwrap();
        for (got, want) in p.base_color.iter().zip(decoded) {
            assert!((got - want).abs() < 1e-5, "{:?}", p.base_color);
        }
        assert_eq!(p.emissive, [p.base_color[0], p.base_color[1], p.base_color[2]]);
        assert_eq!(
            w.get::<PbrMaterial>(on_linear).unwrap().base_color,
            [0.5, 1.0, 0.04, 0.7]
        );
        assert_eq!(
            w.get::<PbrMaterial>(untextured).unwrap().base_color,
            [0.5, 1.0, 0.04, 0.7]
        );
        assert_eq!(w.get::<PbrMaterial>(white).unwrap().base_color, [1.0; 4]);
        assert_eq!(
            log.counts.get(
                "tints of sRGB-flagged diffuse textures decoded to linear (the game multiplies stored values)"
            ),
            Some(&1)
        );
    }

    /// TwoSided is the PBR material's `double_sided` and nothing else is; InvertCulling reverses a winding the target
    /// has no form for, so it is counted as left out and does not make a one-sided material two-sided, and where
    /// TwoSided is set it changes nothing (no face is culled).
    #[test]
    fn two_sided_is_double_sided_and_invert_culling_alone_is_omitted() {
        let mut w = World::new();
        let sided = |two_sided, invert_culling| MaterialState {
            two_sided,
            invert_culling,
            ..state(0, false, false, 128)
        };
        let one = material(&mut w, Some(sided(false, false)), None);
        let two = material(&mut w, Some(sided(true, false)), None);
        let both = material(&mut w, Some(sided(true, true)), None);
        let inverted = material(&mut w, Some(sided(false, true)), None);
        let bare = material(&mut w, None, None);
        let mut log = PassLog::default();
        LowerShaunwhiteMaterial.run(&mut w, &mut log).unwrap();
        let sides = |e| w.get::<PbrMaterial>(e).unwrap().double_sided;
        assert_eq!(
            [one, two, both, inverted, bare].map(sides),
            [false, true, true, false, false]
        );
        assert_eq!(log.counts.get("materials lowered two-sided (TwoSided)"), Some(&2));
        assert_eq!(
            log.omitted
                .get("InvertCulling without TwoSided (winding not reversed)"),
            Some(&1)
        );
    }

    /// The normal slot is core's normal texture at its own tiling with the `HeightScale` as its scale; the texture
    /// keeps the convention the reader gave it, and a primitive without tangents is counted.
    #[test]
    fn normal_maps_are_lowered_with_their_scale_and_tiling() {
        use primase_core::{Mesh, Primitive, Topology};
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
        s.normal_scale = 1.5;
        let bumped = material(&mut w, Some(s), Some(slot));
        let normal = SlotRef {
            scale: [4.0, 2.0],
            offset: [0.25, 0.],
            ..slot
        };
        w.get_mut::<MaterialRef>(bumped)
            .unwrap()
            .slots
            .insert("normal".into(), normal);
        let plain = material(&mut w, Some(state(0, false, false, 128)), Some(slot));
        let mesh = w.spawn();
        w.insert(
            mesh,
            Mesh::new(vec![Primitive {
                topology: Topology::Triangles,
                streams: std::collections::BTreeMap::from([(
                    Semantic::Position,
                    Stream::F32x3(vec![[0.; 3], [1., 0., 0.], [0., 1., 0.]]),
                )]),
                indices: Some(vec![0, 1, 2]),
                material: Some(bumped),
            }]),
        );
        let mut log = PassLog::default();
        LowerShaunwhiteMaterial.run(&mut w, &mut log).unwrap();
        let p = w.get::<PbrMaterial>(bumped).unwrap();
        let n = p.normal_texture.unwrap();
        assert_eq!((n.texture, n.texcoord), (t, 0));
        let tr = n.transform.unwrap();
        assert_eq!((tr.scale, tr.offset), ([4.0, 2.0], [0.25, 0.0]));
        assert_eq!(p.normal_scale, 1.5);
        assert!(w.get::<PbrMaterial>(plain).unwrap().normal_texture.is_none());
        assert_eq!(
            log.counts
                .get("normal maps lowered (X in alpha, Y in green, Z rebuilt)"),
            Some(&1)
        );
        assert_eq!(
            log.approximations.get(
                "normal maps drawn on primitives without a tangent stream (the consumer derives the frame)"
            ),
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
