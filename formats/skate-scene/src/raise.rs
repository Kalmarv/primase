//! A core world raised into the `skate.*` dialect (pass `raise-skate`), so [`crate::write`] can write it: what any
//! reader's world (core geometry, `pbr.material`, `core.collision`, `core.light`) needs to become a skate scene.
//!
//! - every texture a raised material uses: a `skate.texture` (`textures/<key>.png`; normal maps must already be in
//!   glTF's convention, a normal pass converts them);
//! - every `pbr.material` a placed prototype's primitive is drawn with (its own or a variant's; the others are left
//!   out, counted): a `skate.material`, class by alpha mode (opaque, cutout for a mask, alpha for a blend),
//!   the base texture, normal map, the base colour factor as `base_color` without a texture or as `tint` with one
//!   (sRGB bytes; a factor's alpha with a texture is not kept, counted), cutoff, sidedness, metallic, roughness and
//!   the base texture's wrap; texture transforms must already be baked;
//! - every instanced `core.mesh`: a `skate.prototype` (`prototypes/<id>.glb`, a part per primitive); every placement
//!   a default `skate.placement`;
//! - collision: `core.collision` as it is, its surfaces renamed onto skate surfaces by the map the caller gives
//!   (a game's physic materials onto the standard surfaces: never guessed); a name neither mapped nor a skate surface
//!   is refused;
//! - point and spot `core.light`s: `skate.light`s, the energy in Studio watts by the named policy [`studio_watts`];
//!   directional lights are left out (skate lights itself by its time of day), counted;
//! - the `skate.scene` resource, named by the caller.
//!
//! Then [`crate::prepare_scene_assets`] writes the GLBs and PNGs and [`crate::write`] the scene.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::json::f32_decimal;
use primase_core::pass::PassLog;
use primase_core::pbr::{AlphaMode as PbrAlpha, PbrMaterial, Wrap as PbrWrap};
use primase_core::{
    ColorSpace, Entity, Instance, Light, LightKind, MaterialVariants, Mesh, Name, NormalConvention,
    NormalGreen, NormalZ, Surfaces, TexRole, Texture, Transform, World,
};

use crate::components::{
    AlphaMode, FileRef, MaterialClass, Other, Part, STANDARD_SURFACES, SkateLight, SkateMaterial,
    SkatePlacement, SkatePrototype, SkateScene, SkateTexture, TableRef, Wrap,
};
use crate::{Error, Result};

/// What [`raise_skate`] needs from the caller.
#[derive(Clone, Debug, Default)]
pub struct RaiseOptions {
    /// The scene's name, `[A-Za-z0-9_-]+`.
    pub name: String,
    /// The world's surface names onto skate surfaces (`"Snow"` -> `"asphalt"`).
    pub surfaces: BTreeMap<String, String>,
}

/// Studio watts for a light of `candela`: the Blender power `P` whose Cycles shading of a Lambertian surface,
/// `albedo / pi x P / (4 pi d^2)`, equals glTF's `albedo x I / d^2`: `P = 4 pi^2 I`.
pub fn studio_watts(candela: f32) -> f32 {
    (4.0 * std::f64::consts::PI * std::f64::consts::PI * f64::from(candela)) as f32
}

fn byte(linear: f32) -> u8 {
    let c = f64::from(linear).clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

/// A manifest key from a name: `[A-Za-z0-9_-]` kept, other characters `_`, unique by a number suffix.
fn key(name: &str, taken: &mut BTreeSet<String>) -> String {
    let base: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let base = if base.is_empty() { "x".to_owned() } else { base };
    let mut k = base.clone();
    let mut n = 1;
    while !taken.insert(k.to_lowercase()) {
        n += 1;
        k = format!("{base}_{n}");
    }
    k
}

fn wrap(w: PbrWrap) -> Wrap {
    match w {
        PbrWrap::Repeat => Wrap::Repeat,
        PbrWrap::Clamp => Wrap::Extend,
        PbrWrap::Mirror => Wrap::Mirror,
    }
}

/// Raise `world` into the skate dialect (see the module documentation).
pub fn raise_skate(world: &mut World, options: &RaiseOptions) -> Result<PassLog> {
    let bad = |m: String| Error::Scene(format!("raise-skate: {m}"));
    if world.resource::<SkateScene>().is_some() {
        return Err(bad("the world is already a skate scene".into()));
    }
    let mut log = PassLog::default();
    // surfaces onto skate's
    if let Some(mut surfaces) = world.remove_resource::<Surfaces>() {
        let mut unmapped = Vec::new();
        for s in &mut surfaces.0 {
            match options.surfaces.get(&s.name) {
                Some(to) if STANDARD_SURFACES.contains(&to.as_str()) => s.name.clone_from(to),
                Some(to) => unmapped.push(format!("{} -> {to} (not a skate surface)", s.name)),
                None if STANDARD_SURFACES.contains(&s.name.as_str()) => {}
                None => unmapped.push(s.name.clone()),
            }
        }
        if !unmapped.is_empty() {
            log.count("surfaces not mapped onto skate surfaces", unmapped.len() as u64);
            return Err(bad(format!(
                "surfaces with no skate surface: {}",
                unmapped.join(", ")
            )));
        }
        world.insert_resource(surfaces);
    }
    let name = |w: &World, e: Entity, kind: &str| {
        w.get::<Name>(e)
            .map_or_else(|| format!("{kind}_{e:?}"), |n| n.0.clone())
    };
    // the materials placed primitives are drawn with, and the textures they use
    let placed: BTreeSet<Entity> = world.iter::<Instance>().map(|(_, i)| i.prototype).collect();
    let mut drawn: BTreeSet<Entity> = BTreeSet::new();
    for &p in &placed {
        if let Some(mesh) = world.get::<Mesh>(p) {
            drawn.extend(mesh.primitives.iter().filter_map(|q| q.material));
        }
        if let Some(v) = world.get::<MaterialVariants>(p) {
            drawn.extend(v.0.iter().flat_map(|o| o.values().copied()));
        }
    }
    let materials: Vec<(Entity, PbrMaterial)> = world
        .iter::<PbrMaterial>()
        .filter(|(e, _)| drawn.contains(e))
        .map(|(e, m)| (e, m.clone()))
        .collect();
    log.count(
        "materials no placed primitive is drawn with, not raised",
        (world.iter::<PbrMaterial>().count() - materials.len()) as u64,
    );
    let mut used: BTreeSet<Entity> = BTreeSet::new();
    for (_, m) in &materials {
        for r in [m.base_color_texture, m.normal_texture].into_iter().flatten() {
            if r.transform.is_some() {
                return Err(bad(
                    "a texture slot still has a transform: bake it first (bake-texture-transforms)".into(),
                ));
            }
            used.insert(r.texture);
        }
    }
    let mut taken = BTreeSet::new();
    for &t in &used {
        let tex = world
            .get::<Texture>(t)
            .ok_or_else(|| bad(format!("texture {t:?} has no pixels")))?;
        let normal = tex.role == TexRole::Normal;
        if normal && tex.normal != Some(NormalConvention::GLTF) {
            return Err(bad(format!(
                "normal map {t:?} not in glTF's convention: run normalize-normals first"
            )));
        }
        let k = key(&name(world, t, "texture"), &mut taken);
        let skate = SkateTexture {
            key: k.clone(),
            file: FileRef {
                path: format!("textures/{k}.png"),
                sha256: None,
            },
            stem: None,
            source_format: None,
            source_size: None,
            linear: Some(tex.color_space == ColorSpace::Linear),
            green: normal.then_some(NormalGreen::Up),
            z: normal.then_some(NormalZ::Stored),
            other: Other::new(),
        };
        world.insert(t, skate);
        log.count("textures", 1);
    }
    // materials
    let mut taken = BTreeSet::new();
    for (e, m) in materials {
        let (class, alpha, cutoff) = match m.alpha_mode {
            PbrAlpha::Opaque => (MaterialClass::Opaque, AlphaMode::Opaque, None),
            PbrAlpha::Mask { cutoff } => (MaterialClass::Cutout, AlphaMode::Mask, Some(f32_decimal(cutoff))),
            PbrAlpha::Blend => (MaterialClass::Alpha, AlphaMode::Blend, None),
        };
        let rgb = [m.base_color[0], m.base_color[1], m.base_color[2]];
        let (base_color, tint) = match m.base_color_texture {
            None => (
                Some([
                    byte(rgb[0]),
                    byte(rgb[1]),
                    byte(rgb[2]),
                    (m.base_color[3].clamp(0.0, 1.0) * 255.0).round() as u8,
                ]),
                None,
            ),
            Some(_) => {
                if m.base_color[3] < 1.0 {
                    log.approximate("base colour alpha factors with a texture not kept", 1);
                }
                (None, (rgb != [1.0; 3]).then(|| rgb.map(byte)))
            }
        };
        let w = m.base_color_texture.map(|r| r.wrap);
        if w.is_some_and(|w| w[0] != w[1]) {
            log.approximate("base textures wrapping otherwise along u and v: u's kept", 1);
        }
        let k = key(&name(world, e, "material"), &mut taken);
        world.insert(
            e,
            SkateMaterial {
                key: k,
                class,
                base: m.base_color_texture.map(|r| r.texture),
                normal: m.normal_texture.map(|r| r.texture),
                mask: None,
                base_color,
                tint,
                alpha: Some(alpha),
                cutoff,
                two_sided: Some(m.double_sided),
                domain: None,
                metallic: Some(f32_decimal(m.metallic)),
                roughness: Some(f32_decimal(m.roughness)),
                wrap: w.map(|w| wrap(w[0])),
                ocean: None,
                cast_shadows: None,
                emissive: None,
                other: Other::new(),
            },
        );
        log.count("materials", 1);
    }
    // prototypes and placements
    let mut taken = BTreeSet::new();
    for p in placed {
        let Some(parts) = world.get::<Mesh>(p).map(|m| m.primitives.len()) else {
            continue;
        };
        let id = key(&name(world, p, "prototype"), &mut taken);
        world.insert(
            p,
            SkatePrototype {
                glb: Some(FileRef {
                    path: format!("prototypes/{id}.glb"),
                    sha256: None,
                }),
                id,
                key: None,
                foliage: None,
                ground: None,
                parts: vec![Part::default(); parts],
                collision: None,
                other: Other::new(),
            },
        );
        log.count("prototypes", 1);
    }
    let placements: Vec<Entity> = world.iter::<Instance>().map(|(e, _)| e).collect();
    for e in placements {
        if !world.has::<SkatePlacement>(e) {
            world.insert(e, SkatePlacement::default());
        }
        log.count("placements", 1);
    }
    // lights
    let lights: Vec<(Entity, Light)> = world.iter::<Light>().map(|(e, l)| (e, l.clone())).collect();
    for (e, l) in &lights {
        let (spot, size, blend) = match l.kind {
            LightKind::Directional => {
                log.count(
                    "directional lights not raised (skate lights itself by its time of day)",
                    1,
                );
                continue;
            }
            LightKind::Point => (false, 0.0, 0.0),
            LightKind::Spot {
                inner_cone,
                outer_cone,
            } => (
                true,
                2.0 * outer_cone,
                if outer_cone > 0.0 {
                    1.0 - inner_cone / outer_cone
                } else {
                    0.0
                },
            ),
        };
        let t = world.get::<Transform>(*e).copied().unwrap_or(Transform::IDENTITY);
        let d = t.cols[2].map(|x| -x);
        let len = d.iter().map(|x| x * x).sum::<f32>().sqrt();
        world.insert(
            *e,
            SkateLight {
                spot,
                position: t.cols[3],
                direction: if len > 0.0 {
                    d.map(|x| x / len)
                } else {
                    [0.0, 0.0, -1.0]
                },
                tangent: [0.0; 3],
                color: l.color,
                energy: studio_watts(l.intensity),
                range: l.range.unwrap_or(0.0),
                radius: 0.0,
                spot_size: size,
                spot_blend: blend,
                time_of_day: 0x7F,
                cast_shadow: false,
                priority: None,
                group: String::new(),
                unknown_flags: 0,
            },
        );
        log.count("lights", 1);
    }
    if !lights.is_empty() {
        log.approximate("light energy as Studio watts (4 pi^2 candela)", 1);
    }
    let has_lights = world.iter::<SkateLight>().next().is_some();
    world.insert_resource(SkateScene {
        name: options.name.clone(),
        generator: Some("primase".into()),
        placements: Some(TableRef::single("tables/placements.bin")),
        lights: has_lights.then(|| TableRef::single("tables/lights.bin")),
        ..Default::default()
    });
    Ok(log)
}
