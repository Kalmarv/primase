//! A core world raised into the `skate.*` dialect (pass `raise-skate`), so [`crate::write`] can write it: what any
//! reader's world (core geometry, `pbr.material`, `core.collision`, `core.light`) needs to become a skate scene.
//!
//! - every texture a raised material uses: a `skate.texture` (`textures/<key>.png`; normal maps must already be in
//!   glTF's convention, a normal pass converts them);
//! - every `core.layered` blend a placed prototype's primitive is drawn with: a `skate.material` of class layered
//!   (the blend stays `core.layered`), its layers' materials raised as below;
//! - every `pbr.material` a placed prototype's primitive is drawn with (its own, a variant's or a drawn blend's
//!   layer; the others are left out, counted): a `skate.material`, class by alpha mode (opaque, cutout for a mask, alpha for a blend,
//!   decal for a blend drawn over the surface beneath it, `decal`), the base texture, normal map, the base colour factor as `base_color` without a texture or as `tint` with one
//!   (sRGB bytes; a factor's alpha with a texture is not kept, counted), cutoff, sidedness, metallic, roughness and
//!   the base texture's wrap; texture transforms must already be baked;
//! - packed metallic-roughness and explicit occlusion bindings become linear Studio masks, with factors folded
//!   into the pixels and no conflicting target scalars. All decoded mips are retained. The initial supported
//!   packing uses one image, UV0 and one shared wrap across base/normal/mask; other sampling is refused.
//!   Occlusion is never inferred from an unbound channel. Fractional mask-byte quantization is counted per
//!   material, independently of derived texture counts;
//! - emission texture bindings and nonzero emissive radiance have no established target representation and
//!   are named, counted losses in the returned log (per material, overlapping categories). Source PBR is retained;
//! - every instanced `core.mesh`: a `skate.prototype` (`prototypes/<id>.glb`, a part per primitive; its own
//!   `core.collision`, placed by each instance, as a triangle-mesh soup); every placement a default `skate.placement`;
//! - collision: `core.collision` as it is, its surfaces renamed onto skate surfaces: by the map the caller gives
//!   (a source name onto a standard surface), else by the surface's class ([`class_to_skate`]); a surface with
//!   neither, whose name is no skate surface, is refused (never guessed), as is a water class (no solid skate
//!   surface; skate water is a reader's outline, a named gap);
//! - point and spot `core.light`s: `skate.light`s, the energy in Studio watts by the named policy [`studio_watts`];
//!   directional lights are left out (skate lights itself by its time of day), counted;
//! - a spawn marker where the caller says (a point, or the per-axis median of the placements' positions), snapped to
//!   the ground (`spawn=centre` without placements: no marker, counted);
//! - the `skate.scene` resource, named by the caller.
//!
//! A world with levels of detail (`core.lods`) is refused: one form per prototype is chosen first (`select-lod`); so is
//! one with collision shapes (`core.collider`): they become triangles first (`tessellate-colliders`).
//!
//! Then [`crate::prepare_scene_assets`] writes the GLBs and PNGs and [`crate::write`] the scene.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::json::f32_decimal;
use primase_core::pass::PassLog;
use primase_core::pbr::{AlphaMode as PbrAlpha, PbrMaterial, Wrap as PbrWrap};
use primase_core::{
    Collision, ColorSpace, Entity, Instance, LayeredMaterial, Light, LightKind, MaterialVariants, Mesh, Name,
    NormalConvention, NormalGreen, NormalZ, SurfaceClass, Surfaces, TexRole, Texture, Transform, World,
};

use crate::components::{
    AlphaMode, CollisionMode, Domain, FileRef, MarkerKind, MaterialClass, Other, Part, STANDARD_SURFACES,
    SkateLight, SkateMarker, SkateMaterial, SkateParts, SkatePlacement, SkatePrototype, SkateScene,
    SkateTexture, Snap, SnapMode, SoupRef, TableRef, Wrap,
};
use crate::{Error, Result};

#[path = "raise_materials.rs"]
mod materials;

/// What [`raise_skate`] needs from the caller.
#[derive(Clone, Debug, Default)]
pub struct RaiseOptions {
    /// The scene's name, `[A-Za-z0-9_-]+`.
    pub name: String,
    /// The world's surface names onto skate surfaces (`"Snow"` -> `"asphalt"`).
    pub surfaces: BTreeMap<String, String>,
    /// Where a spawn marker goes, its height found by snapping to the ground; `None`: no marker.
    pub spawn: Option<Spawn>,
    /// The spawn marker's yaw, degrees (0 faces +Z, clockwise seen from above).
    pub spawn_yaw: f64,
    /// The skate surface for a surface with no class and no entry in `surfaces` (counted per name); `None`: refused.
    pub unclassified: Option<String>,
}

/// Where [`raise_skate`] puts the spawn marker.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spawn {
    /// This scene-space (x, z).
    At([f32; 2]),
    /// The median of the placements' positions (x, z, each its own median); none without placements.
    Centre,
}

/// CLASS_TO_SKATE: the skate surface of a surface class, the named policy of [`raise_skate`] (a target refinement such
/// as `metal_grate` comes only from the caller's map). `None`: no solid skate surface (water).
pub fn class_to_skate(class: SurfaceClass) -> Option<&'static str> {
    Some(match class {
        SurfaceClass::Concrete => "concrete",
        SurfaceClass::Asphalt => "asphalt",
        SurfaceClass::Stone => "marble",
        SurfaceClass::Brick => "brick",
        SurfaceClass::Tile => "roofing",
        SurfaceClass::Metal => "metal",
        SurfaceClass::Wood => "wood",
        SurfaceClass::Glass => "glass",
        SurfaceClass::Plastic => "plastic",
        SurfaceClass::Rubber => "rubber",
        SurfaceClass::Fabric => "default",
        SurfaceClass::Grass => "grass",
        SurfaceClass::Dirt => "earth",
        SurfaceClass::Gravel => "gravel",
        SurfaceClass::Sand => "sand",
        SurfaceClass::Mud => "earth",
        SurfaceClass::Snow => "earth",
        SurfaceClass::Ice => "marble",
        SurfaceClass::Water => return None,
        SurfaceClass::Other => "default",
    })
}

/// Studio watts for a light of `candela`: the Blender power `P` whose Cycles shading of a Lambertian surface,
/// `albedo / pi x P / (4 pi d^2)`, equals glTF's `albedo x I / d^2`: `P = 4 pi^2 I`.
pub fn studio_watts(candela: f32) -> f32 {
    (4.0 * std::f64::consts::PI * std::f64::consts::PI * f64::from(candela)) as f32
}

fn byte(linear: f32) -> u8 {
    primase_image::linear_to_srgb8(f64::from(linear))
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
/// Refused input leaves the world unchanged. Callers must retain the returned conversion/loss report;
/// successful conversion does not imply that every PBR feature is represented by the target.
pub fn raise_skate(world: &mut World, options: &RaiseOptions) -> Result<PassLog> {
    let bad = |m: String| Error::Scene(format!("raise-skate: {m}"));
    if world.resource::<SkateScene>().is_some() {
        return Err(bad("the world is already a skate scene".into()));
    }
    if let Some((e, _)) = world.iter::<primase_core::Lods>().next() {
        return Err(bad(format!(
            "{e:?} has levels of detail: choose one form first (select-lod)"
        )));
    }
    if let Some((e, _)) = world.iter::<primase_core::Collider>().next() {
        return Err(bad(format!(
            "{e:?} has a collision shape: skate collides with triangles (tessellate-colliders first)"
        )));
    }
    // surfaces onto skate's: the caller's map, else the class, else a name that is a skate surface already, else the
    // caller's surface for unclassified ones
    if let Some(u) = &options.unclassified
        && !STANDARD_SURFACES.contains(&u.as_str())
    {
        return Err(bad(format!("unclassified={u} is not a skate surface")));
    }
    let mut unclassified: Vec<String> = Vec::new();
    let mut surfaces = world.resource::<Surfaces>().cloned();
    if let Some(surfaces) = surfaces.as_mut() {
        let mut unmapped = Vec::new();
        for s in &mut surfaces.0 {
            match (options.surfaces.get(&s.name), s.class) {
                (Some(to), _) if STANDARD_SURFACES.contains(&to.as_str()) => s.name.clone_from(to),
                (Some(to), _) => unmapped.push(format!("{} -> {to} (not a skate surface)", s.name)),
                (None, Some(class)) => match class_to_skate(class) {
                    Some(to) => s.name = to.to_owned(),
                    None => unmapped.push(format!(
                        "{} (class {}: no solid skate surface)",
                        s.name,
                        class.as_str()
                    )),
                },
                (None, None) if STANDARD_SURFACES.contains(&s.name.as_str()) => {}
                (None, None) => match &options.unclassified {
                    Some(to) => {
                        unclassified.push(std::mem::replace(&mut s.name, to.clone()));
                    }
                    None => unmapped.push(s.name.clone()),
                },
            }
        }
        if !unmapped.is_empty() {
            return Err(bad(format!(
                "surfaces with no skate surface: {}",
                unmapped.join(", ")
            )));
        }
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
            drawn.extend(mesh.primitives().iter().filter_map(|q| q.material));
        }
        if let Some(v) = world.get::<MaterialVariants>(p) {
            drawn.extend(v.0.iter().flat_map(|o| o.values().copied()));
        }
    }
    // a drawn blend (core.layered) is a layered material; its layers' materials are drawn through it
    let layered: Vec<Entity> = world
        .iter::<LayeredMaterial>()
        .filter(|(e, _)| drawn.contains(e))
        .map(|(e, _)| e)
        .collect();
    for &e in &layered {
        if world.has::<PbrMaterial>(e) {
            return Err(bad(format!(
                "material {e:?} is both a blend (core.layered) and a pbr.material"
            )));
        }
        let l = world.get::<LayeredMaterial>(e).expect("listed");
        for layer in l.layers.iter().flatten() {
            if !world.has::<PbrMaterial>(layer.material) {
                return Err(bad(format!(
                    "blend {e:?}: layer material {:?} has no pbr.material",
                    layer.material
                )));
            }
            drawn.insert(layer.material);
        }
    }
    let materials: Vec<(Entity, PbrMaterial)> = world
        .iter::<PbrMaterial>()
        .filter(|(e, _)| drawn.contains(e))
        .map(|(e, m)| (e, m.clone()))
        .collect();
    let plan = materials::prepare(world, &materials)?;
    let mut log = plan.log;
    if let Some(to) = &options.unclassified {
        for name in &unclassified {
            log.count(&format!("unclassified surfaces made {to}: {name}"), 1);
        }
    }
    log.count(
        "materials no placed primitive is drawn with, not raised",
        (world.iter::<PbrMaterial>().count() - materials.len()) as u64,
    );
    let mut used: BTreeSet<Entity> = BTreeSet::new();
    for (_, m) in &materials {
        for r in [m.base_color_texture, m.normal_texture].into_iter().flatten() {
            used.insert(r.texture);
        }
    }
    let mut taken = BTreeSet::new();
    let mut textures = Vec::new();
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
        textures.push((t, skate));
    }
    // Everything that can refuse the input is prepared before the first mutation.
    if let Some(surfaces) = surfaces {
        world.insert_resource(surfaces);
    }
    for (t, skate) in textures {
        world.insert(t, skate);
        log.count("textures", 1);
    }
    let mut mask_entities = Vec::new();
    for (index, pixels) in plan.masks.into_iter().enumerate() {
        let t = world.spawn();
        let k = key(&format!("mask_{index}"), &mut taken);
        world.insert(t, Name(k.clone()));
        world.insert(t, pixels);
        world.insert(
            t,
            SkateTexture {
                key: k.clone(),
                file: FileRef {
                    path: format!("textures/{k}.png"),
                    sha256: None,
                },
                stem: None,
                source_format: None,
                source_size: None,
                linear: Some(true),
                green: None,
                z: None,
                other: Other::new(),
            },
        );
        mask_entities.push(t);
        log.count("textures", 1);
    }
    // materials
    let mut taken = BTreeSet::new();
    for (e, m) in materials {
        let (mut class, alpha, cutoff) = match m.alpha_mode {
            PbrAlpha::Opaque => (MaterialClass::Opaque, AlphaMode::Opaque, None),
            PbrAlpha::Mask { cutoff } => (MaterialClass::Cutout, AlphaMode::Mask, Some(f32_decimal(cutoff))),
            PbrAlpha::Blend => (MaterialClass::Alpha, AlphaMode::Blend, None),
        };
        // a surface the source draws over the one beneath it is skate's decal (its class and domain)
        let decal = primase_core::DrawOrder::of(world, e).layer == primase_core::DrawLayer::Decal;
        if decal {
            class = MaterialClass::Decal;
            log.count("materials raised as decals (core.draw_order)", 1);
        }
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
        let mask = plan.bindings.get(&e).map(|&i| mask_entities[i]);
        let w = plan
            .wraps
            .get(&e)
            .map(|&w| [w; 2])
            .or_else(|| m.base_color_texture.map(|r| r.wrap));
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
                mask,
                base_color,
                tint,
                alpha: Some(alpha),
                cutoff,
                two_sided: Some(m.double_sided),
                domain: decal.then_some(Domain::Decal),
                metallic: mask.is_none().then(|| f32_decimal(m.metallic)),
                roughness: mask.is_none().then(|| f32_decimal(m.roughness)),
                wrap: w.map(|w| wrap(w[0])),
                ocean: None,
                cast_shadows: m.cast_shadows,
                emissive: None,
                vertex_alpha: None,
                other: Other::new(),
            },
        );
        log.count("materials", 1);
    }
    // blends: the layered class; the blend itself stays core.layered, its layers the materials raised above
    for e in layered {
        let k = key(&name(world, e, "material"), &mut taken);
        world.insert(
            e,
            SkateMaterial {
                key: k,
                class: MaterialClass::Layered,
                base: None,
                normal: None,
                mask: None,
                base_color: None,
                tint: None,
                alpha: None,
                cutoff: None,
                two_sided: None,
                domain: None,
                metallic: None,
                roughness: None,
                wrap: None,
                ocean: None,
                cast_shadows: None,
                emissive: None,
                vertex_alpha: None,
                other: Other::new(),
            },
        );
        log.count("layered materials", 1);
    }
    // prototypes and placements
    let mut taken = BTreeSet::new();
    for p in placed {
        let Some(parts) = world.get::<Mesh>(p).map(|m| m.primitives().len()) else {
            continue;
        };
        let id = key(&name(world, p, "prototype"), &mut taken);
        // its own collision (core.collision on the prototype, placed by each instance): a triangle-mesh soup
        let collision = world.has::<Collision>(p).then(|| SoupRef {
            table: TableRef::single(&format!("collision/{id}.bin")),
            mode: Some(CollisionMode::TriangleMesh),
            ground: None,
            region_cut: None,
            other: Other::new(),
        });
        if collision.is_some() {
            log.count("prototypes with their own collision", 1);
        }
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
                collision,
                other: Other::new(),
            },
        );
        world.insert(p, SkateParts(vec![Part::default(); parts]));
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
                cast_shadow: l.cast_shadows.unwrap_or(false),
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
    let spawn = match options.spawn {
        Some(Spawn::At(p)) => Some(p),
        Some(Spawn::Centre) => {
            // the median position, so a few placements far from the rest do not move it off the map
            let (mut xs, mut zs): (Vec<f32>, Vec<f32>) = world
                .iter::<Instance>()
                .map(|(e, _)| {
                    let [x, _, z] = world
                        .get::<Transform>(e)
                        .copied()
                        .unwrap_or(Transform::IDENTITY)
                        .cols[3];
                    (x, z)
                })
                .unzip();
            xs.sort_by(f32::total_cmp);
            zs.sort_by(f32::total_cmp);
            let median = |v: &[f32]| {
                let n = v.len();
                if n % 2 == 1 {
                    v[n / 2]
                } else {
                    (v[n / 2 - 1] + v[n / 2]) / 2.0
                }
            };
            if xs.is_empty() {
                log.count("spawn=centre without placements: no spawn marker", 1);
            }
            (!xs.is_empty()).then(|| [median(&xs), median(&zs)])
        }
        None => None,
    };
    if let Some([x, z]) = spawn {
        let e = world.spawn();
        world.insert(e, Name("spawn".into()));
        world.insert(
            e,
            SkateMarker {
                kind: MarkerKind::Spawn,
                position: Some(([x, z], None)),
                yaw: Some(options.spawn_yaw),
                m: None,
                blueprint: None,
                snap: Some(Snap::Mode(SnapMode::Ground)),
                lift: None,
                shelter: None,
                trainer: None,
                other: Other::new(),
            },
        );
        log.count("markers", 1);
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

/// `raise-skate{name=N[,spawn=centre | spawn_x=X,spawn_z=Z][,spawn_yaw=DEGREES][,map=NAME:surface;...]
/// [,unclassified=surface]}`:
/// [`raise_skate`] as a pass; `map` is the caller's map of source surface names (it wins over the class,
/// [`class_to_skate`]).
#[derive(Debug)]
pub struct RaiseSkate {
    pub options: RaiseOptions,
}

/// `raise-skate` from its options (a [`primase_core::pass::PassFactory`]).
pub fn raise_skate_pass(
    o: &primase_core::pass::Options,
) -> std::result::Result<Box<dyn primase_core::pass::Pass>, String> {
    let mut options = RaiseOptions {
        name: "scene".into(),
        ..RaiseOptions::default()
    };
    let (mut x, mut z) = (None, None);
    for (k, v) in o {
        let number = |v: &str| {
            v.parse::<f32>()
                .ok()
                .filter(|n| n.is_finite())
                .ok_or_else(|| format!("{k}={v} is not a finite number"))
        };
        match k.as_str() {
            "name"
                if !v.is_empty()
                    && v.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') =>
            {
                options.name.clone_from(v);
            }
            "name" => return Err(format!("name={v} is not [A-Za-z0-9_-]+")),
            "spawn" if v == "centre" => options.spawn = Some(Spawn::Centre),
            "spawn" => return Err(format!("spawn={v}: only spawn=centre (or spawn_x, spawn_z)")),
            "spawn_yaw" => options.spawn_yaw = f64::from(number(v)?),
            "unclassified" => options.unclassified = Some(v.clone()),
            // the caller's map of source surface names onto skate surfaces: `map=NAME:skate;NAME:skate`
            "map" => {
                for pair in v.split(';').filter(|p| !p.is_empty()) {
                    let (from, to) = pair
                        .split_once(':')
                        .ok_or_else(|| format!("map={v}: each entry is NAME:skate_surface"))?;
                    options.surfaces.insert(from.to_owned(), to.to_owned());
                }
            }
            "spawn_x" => x = Some(number(v)?),
            "spawn_z" => z = Some(number(v)?),
            _ => return Err(format!("unknown option {k}")),
        }
    }
    match (x, z, options.spawn) {
        (Some(x), Some(z), None) => options.spawn = Some(Spawn::At([x, z])),
        (None, None, _) => {}
        (Some(_), Some(_), Some(_)) => return Err("spawn=centre or spawn_x, spawn_z: not both".into()),
        _ => return Err("spawn_x and spawn_z go together".into()),
    }
    Ok(Box::new(RaiseSkate { options }))
}

impl primase_core::pass::Pass for RaiseSkate {
    fn name(&self) -> &'static str {
        "raise-skate"
    }
    fn options(&self) -> primase_core::pass::Options {
        let mut o = primase_core::pass::Options::from([("name".to_owned(), self.options.name.clone())]);
        match self.options.spawn {
            Some(Spawn::At([x, z])) => {
                o.insert("spawn_x".into(), x.to_string());
                o.insert("spawn_z".into(), z.to_string());
            }
            Some(Spawn::Centre) => {
                o.insert("spawn".into(), "centre".into());
            }
            None => {}
        }
        if self.options.spawn_yaw != 0.0 {
            o.insert("spawn_yaw".into(), self.options.spawn_yaw.to_string());
        }
        if let Some(u) = &self.options.unclassified {
            o.insert("unclassified".into(), u.clone());
        }
        if !self.options.surfaces.is_empty() {
            let map: Vec<String> = self
                .options
                .surfaces
                .iter()
                .map(|(a, b)| format!("{a}:{b}"))
                .collect();
            o.insert("map".into(), map.join(";"));
        }
        o
    }
    fn provides(&self) -> &[&'static str] {
        &[
            "skate.texture",
            "skate.material",
            "skate.prototype",
            "skate.parts",
            "skate.placement",
            "skate.light",
            "skate.marker",
            "skate.scene",
            "core.name",
            "core.texture",
        ]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.surfaces"]
    }
    fn approximates(&self) -> &'static str {
        "light energy as Studio watts (4 pi^2 candela); base colour alpha factors with a texture not kept; emission not represented (all counted)"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let l = raise_skate(w, &self.options).map_err(|e| e.to_string())?;
        log.counts.extend(l.counts);
        log.approximations.extend(l.approximations);
        Ok(())
    }
}

/// The passes of the skate-scene dialect, by name.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    reg.register("raise-skate", raise_skate_pass);
}
