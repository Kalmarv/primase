//! A core world raised into the `skate.*` dialect (pass `raise-skate`), so [`crate::interchange::write`] can write it: what any
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
//! - specular reflectance (`KHR_materials_specular`): skate's MSK has no reflectance channel, so every dielectric
//!   reflects at F0 0.04 ([`SKATE_F0`]); a dielectric reflecting less (0.04 x colour x factor, x the strength texture's
//!   A per texel when it is the mask source) is folded into roughness so its highlight peaks no higher
//!   ([`fold_reflectance`]): a named, counted approximation; metals and reflectance of 0.04 or more are kept as they are;
//! - every instanced `core.mesh`: a `skate.prototype` (`prototypes/<id>.glb`, a part per primitive; its own
//!   `core.collision`, placed by each instance, as a triangle-mesh soup); an instanced `core.collision` without a mesh
//!   a `skate.prototype` that collides without drawing (no GLB, no parts), counted; every placement a default
//!   `skate.placement`;
//! - collision: `core.collision` as it is, its surfaces renamed onto skate surfaces: by the map the caller gives
//!   (a source name onto a standard surface), else by the surface's class ([`class_to_skate`]), else onto the
//!   caller's surface for unclassified ones (`unclassified`); a surface with none of these, whose name is no skate
//!   surface, is never guessed: its triangles are left out, reported by surface name ([`PassLog::omit`]), and a
//!   collision left with no triangle is removed; a water class has no solid skate surface: a prototype whose
//!   collision is all water gets skate's water collision (Studio's water mode); water triangles in a solid soup or
//!   the world's collision stay solid, counted;
//! - what the source says a material's surfaces are (`core.material_usage`): water is skate's water class (Studio's
//!   ocean); foliage is skate's leaves class in the foliage domain, except a material the source draws over the
//!   surface beneath (`core.draw_order` decal), which stays a decal (counted);
//! - point and spot `core.light`s: `skate.light`s, the energy in Studio watts by the named policy [`studio_watts`] of
//!   the intensity times the caller's calibration (`light_scale`: `standard`, skate's fitted [`SKATE_LIGHT_SCALE`],
//!   or a scale of its own; [`LightCalibration`]): an approximation reported with its value for every light, the
//!   standard one included (a fit, not a measurement);
//!   directional lights are left out (skate lights itself by its time of day), counted;
//! - the world's `core.marker` points of a kind skate has (spawn, travel, trainer, vfx, trigger effect):
//!   `skate.marker`s on the same entities; a ground marker (no height) snaps to the ground and keeps its yaw, one
//!   with a transform keeps its position and faces its local +Z; other kinds are left out, counted by kind. A spawn
//!   is placed before the writer plans (`place-spawn` at selection), so one spawn serves every part of a map written
//!   in parts;
//! - the `skate.scene` resource, named by the caller.
//!
//! It chooses no floor, spawn or region: those are the program's (`place-spawn` at selection, the writer's floor and
//! region options) and run apart from raising. It requires no light, collision or marker: a world without them raises
//! without them.
//!
//! A world with levels of detail (`core.lods`) is refused: one form per prototype is chosen first (`select-lod`); so is
//! one with collision shapes (`core.collider`): they become triangles first (`tessellate-colliders`).
//!
//! Then [`crate::interchange::write`] writes the scene, its meshes and textures encoded as GLBs and PNGs.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::json::f32_decimal;
use primase_core::pass::PassLog;
use primase_core::pbr::{AlphaMode as PbrAlpha, PbrMaterial, Wrap as PbrWrap};
use primase_core::{
    Collision, ColorSpace, Entity, Instance, LayeredMaterial, Light, LightKind, MaterialUsage,
    MaterialVariants, Mesh, Name, NormalConvention, NormalGreen, NormalZ, SurfaceClass, Surfaces, TexRole,
    Texture, Transform, World,
};

use crate::components::{
    AlphaMode, CollisionMode, Domain, FileRef, MarkerKind, MaterialClass, Other, Part, STANDARD_SURFACES,
    SkateLight, SkateMarker, SkateMaterial, SkateParts, SkatePlacement, SkatePrototype, SkateRock,
    SkateScene, SkateTexture, Snap, SnapMode, SoupRef, TableRef, Wrap,
};
use crate::{Error, Result};

mod materials;
pub use materials::{SKATE_F0, fold_reflectance};

/// How [`raise_skate`] calibrates lights: every light's intensity (physical candela) times a scale before
/// [`studio_watts`], reported as an approximation with its value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LightCalibration {
    /// skate's fitted calibration, [`SKATE_LIGHT_SCALE`] (`light_scale=standard`).
    Standard,
    /// A scale of the caller's (`light_scale=S`): finite and above 0.
    Scale(f64),
}

impl LightCalibration {
    /// The scale the intensity is multiplied by.
    pub fn scale(self) -> f64 {
        match self {
            LightCalibration::Standard => SKATE_LIGHT_SCALE,
            LightCalibration::Scale(s) => s,
        }
    }
}

/// What [`raise_skate`] needs from the caller: the scene's name and the lights' calibration ([`RaiseOptions::new`]),
/// and optionally the world's surface names onto skate surfaces and the skate surface of an unclassified one.
#[derive(Clone, Debug, PartialEq)]
pub struct RaiseOptions {
    name: String,
    surfaces: BTreeMap<String, String>,
    unclassified: Option<String>,
    lights: LightCalibration,
}

impl RaiseOptions {
    /// For a scene named `name` (`[A-Za-z0-9_-]+`), lights calibrated by `lights` (a scale of the caller's finite and
    /// above 0): no surface mapped, an unclassified surface's triangles left out (reported).
    pub fn new(name: &str, lights: LightCalibration) -> std::result::Result<RaiseOptions, String> {
        if !valid_name(name) {
            return Err(format!("the scene name {name:?} is not [A-Za-z0-9_-]+"));
        }
        if let LightCalibration::Scale(scale) = lights
            && !(scale.is_finite() && scale > 0.0)
        {
            return Err(format!("light_scale={scale} is not a finite number above 0"));
        }
        Ok(RaiseOptions {
            name: name.to_owned(),
            surfaces: BTreeMap::new(),
            unclassified: None,
            lights,
        })
    }

    /// The world's surface names onto skate surfaces (`"Snow"` -> `"asphalt"`), both named.
    pub fn with_surfaces(
        mut self,
        surfaces: BTreeMap<String, String>,
    ) -> std::result::Result<RaiseOptions, String> {
        if let Some((from, to)) = surfaces
            .iter()
            .find(|(from, to)| from.is_empty() || to.is_empty())
        {
            return Err(format!("surface {from:?} -> {to:?}: both are names"));
        }
        self.surfaces = surfaces;
        Ok(self)
    }

    /// The skate surface for a surface with no class and no entry in the surfaces (counted per name).
    pub fn with_unclassified(mut self, surface: &str) -> std::result::Result<RaiseOptions, String> {
        if surface.is_empty() {
            return Err("unclassified: a skate surface name".into());
        }
        self.unclassified = Some(surface.to_owned());
        Ok(self)
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn surfaces(&self) -> &BTreeMap<String, String> {
        &self.surfaces
    }
    pub fn unclassified(&self) -> Option<&str> {
        self.unclassified.as_deref()
    }
    pub fn lights(&self) -> LightCalibration {
        self.lights
    }

    /// From `raise-skate`'s options (`name` and `light_scale=standard|S` required; `map=NAME:skate;...`,
    /// `unclassified`), through [`RaiseOptions::new`] and the setters.
    pub fn from_options(o: &primase_core::pass::Options) -> std::result::Result<RaiseOptions, String> {
        let name = o
            .get("name")
            .ok_or("name=N is required: the scene's name, [A-Za-z0-9_-]+")?;
        let lights = match o.get("light_scale").map(String::as_str) {
            Some("standard") => LightCalibration::Standard,
            Some(v) => LightCalibration::Scale(
                v.parse::<f64>()
                    .map_err(|_| format!("light_scale={v} is not standard or a number"))?,
            ),
            None => {
                return Err(
                    "light_scale is required: standard (skate's fitted calibration) or a scale".into(),
                );
            }
        };
        let mut options = RaiseOptions::new(name, lights)?;
        for (k, v) in o {
            match k.as_str() {
                "name" | "light_scale" => {}
                "spawn" | "spawn_x" | "spawn_z" | "spawn_yaw" => {
                    return Err(format!(
                        "{k}: the spawn is placed before the writer plans, at selection: \
                         place-spawn{{at=centre | east=E,north=N[,yaw=DEGREES]}} (raise-skate raises it)"
                    ));
                }
                "unclassified" => options = options.with_unclassified(v)?,
                // the caller's map of source surface names onto skate surfaces: `map=NAME:skate;NAME:skate`
                "map" => {
                    let mut surfaces = BTreeMap::new();
                    for pair in v.split(';').filter(|p| !p.is_empty()) {
                        let (from, to) = pair
                            .split_once(':')
                            .ok_or_else(|| format!("map={v}: each entry is NAME:skate_surface"))?;
                        surfaces.insert(from.to_owned(), to.to_owned());
                    }
                    options = options.with_surfaces(surfaces)?;
                }
                _ => return Err(format!("unknown option {k}")),
            }
        }
        Ok(options)
    }

    /// Every setting as `raise-skate`'s options ([`RaiseOptions::from_options`] reads them back).
    pub fn options(&self) -> primase_core::pass::Options {
        let mut o = primase_core::pass::Options::from([
            ("name".to_owned(), self.name.clone()),
            (
                "light_scale".to_owned(),
                match self.lights {
                    LightCalibration::Standard => "standard".to_owned(),
                    LightCalibration::Scale(s) => s.to_string(),
                },
            ),
        ]);
        if let Some(u) = &self.unclassified {
            o.insert("unclassified".into(), u.clone());
        }
        if !self.surfaces.is_empty() {
            let map: Vec<String> = self.surfaces.iter().map(|(a, b)| format!("{a}:{b}")).collect();
            o.insert("map".into(), map.join(";"));
        }
        o
    }
}

/// skate's light calibration: Studio watts per candela of a source light are `4 pi^2 x SKATE_LIGHT_SCALE` (Studio
/// writes 683 lm per watt). Fitted against San Vansterdam's own lights (street lamps, plaza lights, wall sconces) by
/// illuminance 4 m along a spot's axis, a source light taken as physical (GTA's sun-calibrated): stock / source
/// geometric mean 0.177, class residuals 1.11, 1.28, 0.70. Named assumptions: skate's engine draws punctual lights
/// by the model of its published physically based rendering course notes (spot I = lumens / pi, E = I / d^2 x
/// (1 - (d/r)^4)^2: Lagarde and de Rousiers, SIGGRAPH 2014), not measured; Studio's LightUnit 0 is
/// lumens (inferred from its 683 x watts); one scale matches at one distance (the source's falloff window and
/// inverse-square differ: 4 m chosen, the ground under a lamp).
pub const SKATE_LIGHT_SCALE: f64 = 3.007e-4;

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

/// A scene name: `[A-Za-z0-9_-]+`.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
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
    // caller's surface for unclassified ones, else none: the surface's triangles are left out
    if let Some(u) = &options.unclassified
        && !STANDARD_SURFACES.contains(&u.as_str())
    {
        return Err(bad(format!("unclassified={u} is not a skate surface")));
    }
    let mut unclassified: Vec<String> = Vec::new();
    // the surfaces of class water: their triangles become skate's water collision (see below)
    let mut water_surfaces: BTreeSet<u16> = BTreeSet::new();
    let mut water_names: Vec<String> = Vec::new();
    // the surfaces with no skate surface, by index, with what to call them: their triangles are left out
    let mut omitted: BTreeMap<u16, String> = BTreeMap::new();
    let index = |i: usize| u16::try_from(i).map_err(|_| bad("more than 65,536 surfaces".into()));
    let mut surfaces = world.resource::<Surfaces>().cloned();
    if let Some(surfaces) = surfaces.as_mut() {
        let mut unmapped = Vec::new();
        for (i, s) in surfaces.0.iter_mut().enumerate() {
            match (options.surfaces.get(&s.name), s.class) {
                (Some(to), _) if STANDARD_SURFACES.contains(&to.as_str()) => s.name.clone_from(to),
                (Some(to), _) => unmapped.push(format!("{} -> {to}", s.name)),
                (None, Some(SurfaceClass::Water)) => {
                    // no solid skate surface: its triangles are water collision; the name a placeholder
                    water_surfaces.insert(index(i)?);
                    water_names.push(std::mem::replace(&mut s.name, "default".to_owned()));
                }
                (None, Some(class)) => match class_to_skate(class) {
                    Some(to) => s.name = to.to_owned(),
                    None => {
                        omitted.insert(index(i)?, format!("{} (class {})", s.name, class.as_str()));
                    }
                },
                (None, None) if STANDARD_SURFACES.contains(&s.name.as_str()) => {}
                (None, None) => match &options.unclassified {
                    Some(to) => {
                        unclassified.push(std::mem::replace(&mut s.name, to.clone()));
                    }
                    None => {
                        omitted.insert(index(i)?, s.name.clone());
                    }
                },
            }
        }
        // the caller's own map naming no skate surface is a refused value, not a feature to leave out
        if !unmapped.is_empty() {
            return Err(bad(format!(
                "the surface map names surfaces skate does not have: {}",
                unmapped.join(", ")
            )));
        }
    }
    // each collision without its triangles on those surfaces (and the matching rock flags), none when nothing is left
    let mut trimmed: BTreeMap<Entity, Option<(Collision, Option<SkateRock>)>> = BTreeMap::new();
    let mut left_out: BTreeMap<u16, u64> = BTreeMap::new();
    if !omitted.is_empty() {
        for (e, c) in world.iter::<Collision>() {
            if !c.surfaces.iter().any(|s| omitted.contains_key(s)) {
                continue;
            }
            let rock = world.get::<SkateRock>(e);
            let mut kept = Collision {
                positions: c.positions.clone(),
                triangles: Vec::new(),
                surfaces: Vec::new(),
            };
            let mut kept_rock = rock.map(|_| SkateRock(Vec::new()));
            for (t, (&tri, &s)) in c.triangles.iter().zip(&c.surfaces).enumerate() {
                if omitted.contains_key(&s) {
                    *left_out.entry(s).or_default() += 1;
                    continue;
                }
                kept.triangles.push(tri);
                kept.surfaces.push(s);
                if let (Some(out), Some(r)) = (kept_rock.as_mut(), rock) {
                    out.0.push(r.0.get(t).copied().unwrap_or(false));
                }
            }
            trimmed.insert(e, (!kept.triangles.is_empty()).then_some((kept, kept_rock)));
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
    // a prototype whose collision is all water is skate water (its soup in water mode)
    let all_water =
        |c: &Collision| !c.surfaces.is_empty() && c.surfaces.iter().all(|s| water_surfaces.contains(s));
    // an entity's collision as it will be raised: without the triangles left out
    let kept_collision = |e: Entity| match trimmed.get(&e) {
        Some(t) => t.as_ref().map(|(c, _)| c),
        None => world.get::<Collision>(e),
    };
    let water_protos: BTreeSet<Entity> = placed
        .iter()
        .copied()
        .filter(|&p| kept_collision(p).is_some_and(all_water))
        .collect();
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
    for name in &water_names {
        log.count(&format!("water surfaces kept as water collision: {name}"), 1);
    }
    // collision that is no prototype's (the world's soups) stays solid: its water triangles are counted
    let world_wet: usize = world
        .iter::<Collision>()
        .filter(|(e, _)| !placed.contains(e))
        .filter_map(|(e, _)| kept_collision(e))
        .map(|c| c.surfaces.iter().filter(|s| water_surfaces.contains(s)).count())
        .sum();
    for (s, n) in &left_out {
        log.omit(
            &format!(
                "collision triangles on a surface with no skate surface, left out: {}",
                omitted[s]
            ),
            *n,
        );
    }
    if world_wet > 0 {
        log.approximate(
            "water triangles in the world's collision kept solid",
            world_wet as u64,
        );
    }
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
    for (e, kept) in trimmed {
        match kept {
            Some((c, rock)) => {
                world.insert(e, c);
                if let Some(rock) = rock {
                    world.insert(e, rock);
                }
            }
            None => {
                world.remove::<Collision>(e);
                world.remove::<SkateRock>(e);
                log.count("collisions with no triangle left, removed", 1);
            }
        }
    }
    for (t, skate) in textures {
        world.insert(t, skate);
        log.count("textures", 1);
    }
    let mut mask_entities = Vec::new();
    for (pixels, name) in plan.masks.into_iter().zip(plan.mask_names) {
        let t = world.spawn();
        // the key unique in this world; the name the mask's own (its file's stem wherever it is written)
        let k = key(&name, &mut taken);
        world.insert(t, Name(name));
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
        // what the source says the surface is (core.material_usage): water is skate's water class (Studio's ocean);
        // foliage is skate's leaves class in the foliage domain, unless the source draws it over the surface beneath
        // (a decal), which keeps it layered there
        let usage = world.get::<MaterialUsage>(e).copied();
        let water = usage == Some(MaterialUsage::Water);
        // a surface the source draws over the one beneath it is skate's decal (its class and domain)
        let decal = !water && primase_core::DrawOrder::of(world, e).layer == primase_core::DrawLayer::Decal;
        let foliage = usage == Some(MaterialUsage::Foliage) && !decal;
        if water {
            class = MaterialClass::Water;
            log.count("materials raised as water (core.material_usage)", 1);
        } else if decal {
            class = MaterialClass::Decal;
            log.count("materials raised as decals (core.draw_order)", 1);
            if usage == Some(MaterialUsage::Foliage) {
                log.count(
                    "foliage materials kept as decals (drawn over the surface beneath)",
                    1,
                );
            }
        } else if foliage {
            class = MaterialClass::Leaves;
            log.count("materials raised as foliage (core.material_usage)", 1);
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
                domain: if decal {
                    Some(Domain::Decal)
                } else {
                    foliage.then_some(Domain::Foliage)
                },
                metallic: mask.is_none().then(|| f32_decimal(m.metallic)),
                roughness: mask.is_none().then(|| {
                    // the reflectance skate cannot carry, folded (materials::fold_reflectance)
                    let folded = materials::specular_f0(&m).map_or(f64::from(m.roughness), |f0| {
                        materials::fold_reflectance(f64::from(m.roughness), f0)
                    });
                    f32_decimal(folded as f32)
                }),
                wrap: w.map(|w| wrap(w[0])),
                ocean: water.then_some(true),
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
        // a prototype with collision and no mesh collides without drawing (no GLB); one with neither is left out
        let parts = world.get::<Mesh>(p).map(|m| m.primitives().len());
        if parts.is_none() && !world.has::<Collision>(p) {
            continue;
        }
        let id = key(&name(world, p, "prototype"), &mut taken);
        // its own collision (core.collision on the prototype, placed by each instance): a triangle-mesh soup, or water
        // when every triangle is on a water surface (a mixed soup stays solid, its water triangles counted)
        let water = water_protos.contains(&p);
        let collision = world.get::<Collision>(p).map(|c| {
            let wet = c.surfaces.iter().filter(|s| water_surfaces.contains(s)).count();
            if !water && wet > 0 {
                log.approximate("water triangles in a solid collision soup kept solid", wet as u64);
            }
            SoupRef {
                table: TableRef::single(&format!("collision/{id}.bin")),
                mode: Some(if water {
                    CollisionMode::Water
                } else {
                    CollisionMode::TriangleMesh
                }),
                ground: None,
                region_cut: None,
                other: Other::new(),
            }
        });
        if collision.is_some() {
            log.count("prototypes with their own collision", 1);
        }
        if water {
            log.count("prototypes with water collision", 1);
        }
        if parts.is_none() {
            log.count("prototypes that collide without drawing", 1);
        }
        world.insert(
            p,
            SkatePrototype {
                glb: parts.map(|_| FileRef {
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
        world.insert(p, SkateParts(vec![Part::default(); parts.unwrap_or(0)]));
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
                energy: studio_watts((f64::from(l.intensity) * options.lights.scale()) as f32),
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
    // the calibration is a fit, not a measurement: every light raised is an approximation by its value
    let raised = log.counts.get("lights").copied().unwrap_or(0);
    if raised > 0 {
        let fit = if options.lights.scale() == SKATE_LIGHT_SCALE {
            "skate's fitted calibration"
        } else {
            "not skate's fitted calibration"
        };
        log.approximate(
            &format!(
                "light energy as Studio watts: 4 pi^2 x candela x light_scale {} ({fit}, fitted at 4 m assuming the \
                 engine's published punctual light model)",
                options.lights.scale()
            ),
            raised,
        );
    }
    // the world's markers (a spawn among them placed at selection by place-spawn), on their own entities
    let markers: Vec<(Entity, primase_core::Marker)> = world
        .iter::<primase_core::Marker>()
        .filter(|(e, _)| !world.has::<SkateMarker>(*e))
        .map(|(e, m)| (e, m.clone()))
        .collect();
    for (e, m) in markers {
        let kind = match m.kind.as_str() {
            "spawn" => MarkerKind::Spawn,
            "travel" => MarkerKind::Travel,
            "trainer" => MarkerKind::Trainer,
            "vfx" => MarkerKind::Vfx,
            "trigger_effect" => MarkerKind::TriggerEffect,
            other => {
                log.count(
                    &format!("markers of kind '{other}' not raised (no skate marker kind)"),
                    1,
                );
                continue;
            }
        };
        let (position, yaw, snap) = match (m.ground, world.get::<Transform>(e)) {
            // a ground marker: (east, north) is scene space (x, -z); its height is the ground's
            (Some([east, north]), _) => (
                ([east, -north], None),
                m.yaw.map(f64::from),
                Some(Snap::Mode(SnapMode::Ground)),
            ),
            (None, t) => {
                let t = t.copied().unwrap_or(Transform::IDENTITY);
                let [x, y, z] = t.cols[3];
                // it faces its local +Z: degrees about +Y, 0 facing +Z, 90 facing +X
                let f = t.cols[2];
                let yaw = f64::from(f[0]).atan2(f64::from(f[2])).to_degrees();
                (([x, z], Some(y)), Some(yaw), None)
            }
        };
        world.insert(
            e,
            SkateMarker {
                kind,
                position: Some(position),
                yaw,
                m: None,
                blueprint: None,
                snap,
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

/// `raise-skate{name=N,light_scale=standard|S[,map=NAME:surface;...][,unclassified=surface]}`:
/// [`raise_skate`] as a pass; `name` and `light_scale` are required; `map` is the caller's map of source surface names (it wins over the
/// class, [`class_to_skate`]).
#[derive(Debug)]
pub struct RaiseSkate {
    options: RaiseOptions,
}

impl RaiseSkate {
    pub fn new(options: RaiseOptions) -> RaiseSkate {
        RaiseSkate { options }
    }

    pub fn raise_options(&self) -> &RaiseOptions {
        &self.options
    }
}

/// `raise-skate` from its options (a [`primase_core::pass::PassFactory`]).
pub fn raise_skate_pass(
    o: &primase_core::pass::Options,
) -> std::result::Result<Box<dyn primase_core::pass::Pass>, String> {
    Ok(Box::new(RaiseSkate::new(RaiseOptions::from_options(o)?)))
}

impl primase_core::pass::Pass for RaiseSkate {
    fn name(&self) -> &'static str {
        "raise-skate"
    }
    fn options(&self) -> primase_core::pass::Options {
        self.options.options()
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
    /// Collision triangles on a surface with no skate surface (and their rock flags), and a collision left with none.
    fn removes(&self) -> &[&'static str] {
        &["core.collision", "skate.rock"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.surfaces", "core.collision", "skate.rock"]
    }
    /// It lowers nothing a target refuses: what it removes is what skate has no surface for.
    fn legalizes(&self) -> &[&'static str] {
        &[]
    }
    fn approximates(&self) -> &'static str {
        "light energy as Studio watts (4 pi^2 candela x the calibration); base colour alpha factors with a texture not kept; emission not represented (all counted)"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let l = raise_skate(w, &self.options).map_err(|e| e.to_string())?;
        log.absorb(l);
        Ok(())
    }
}
