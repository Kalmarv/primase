//! `lower-skatebird-voronoi`: the materials of SkateBIRD's `Bumped Specular Voronoi` shader, baked from the shader's own
//! compiled program (`unity.programs`, read by [`crate::read::read_voronoi_programs`]) into a colour and a normal map.
//!
//! What the shader is, from its forward-base fragment program (one directional light, D3D11 ps_4_0) and its parameter
//! record: a `Lambert` surface (no specular term, whatever its name) whose `_MainTex` and `_BumpMap` are tiled
//! stochastically. Each is sampled at the 3x3 cells around floor(uv) (uv the map's own `TRANSFORM_TEX` coordinate),
//! each cell c offset by h = frac(sin(dot(c, (12.9898, 78.233))) x 43758.5453), weighted 2^(-7.2135 |c - frac(uv) +
//! h|^2) and normalised: the albedo that blend x `_Color`, the normal the same blend of the normal map, unpacked. Its
//! pattern never repeats, so no texture holds it exactly over a surface.
//!
//! The program's choices, all required options (no defaults): the variant (`keywords`, the renderer's global keywords
//! Unity draws it with, `;`-separated, and `tier`, the hardware tier; [`UnityPrograms::variant`]), the block (`cells`,
//! K >= 2 cells a side) and the resolution (`max-texels`, at most this many texels per cell; a map larger is baked at
//! it, counted). The evaluation grid, (K + 1) x texels a side, is at most 4096, an option beyond it refused.
//!
//! The bake (approximated, counted): [`iridia_exec::evaluate_periodic`] over a block of K x K cells of the map's
//! coordinate at the map's own texel density (up to `max-texels`), its first cell cross-faded so the block tiles. Each
//! texture is sampled as Unity samples it ([`primase_unity::bind::sampled_texture`]); the grid is evaluated one pixel at
//! a time, so a sample at an implicit level has no quad footprint and takes the finest level plus its bias (counted).
//! The faithful level, not yet taken, is the bake's own density: a footprint of size / texels texels, level
//! log2(size / texels), above 0 where `max-texels` caps the bake. Anisotropic filtering is sampled isotropically
//! (counted). The program's output is lit, so the surface comes out of five evaluations, each with one white directional light (x,
//! -x, y, -y, z) and nothing else ([`primase_unity::bind::directional_light`]: ambient, light probes and shadows off),
//! the tangent frame the identity and the world position zero, `_Color` white: each is max(0, n.l) x albedo, so per
//! colour channel the vector (P[x] - P[-x], P[y] - P[-y], P[z]) is the albedo times the unit normal: its length the
//! albedo, and the normal its direction in the channel of the largest albedo (flat where the albedo is black,
//! counted). When the two maps have one tiling and size one bake gives both; otherwise each is baked in its own
//! coordinate. The program's constant fields are reported as evaluated: those the material sets, those the
//! extraction sets (the light, ambient and probes, `_Color`), and any left zero, each of those an approximation.
//!
//! What is missing is never invented: a material without its programs, without the stated variant, whose program
//! samples a slot no parameter names, or with a texture property that names a texture the world holds no pixels for,
//! is left as `lower-unity-pbr` made it, an omission notice saying why. Only a property the material leaves unset
//! samples the default its shader declares (`m_PropInfo`): "white" white, "bump" the flat normal (a texel the
//! program's unpacking reads as (0, 0, 1)); another declared default is an omission too.
//!
//! The material keeps `lower-unity-pbr`'s base colour factor (`_Color`, linear) and takes the colour map (sRGB) and
//! the normal map (glTF's convention) in place of its base map, each slot's transform the map's tiling over the K
//! cells (`_ST` / K, origin moved to top-left); diffuse only (`KHR_materials_specular` factor 0), rough, not metallic.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use iridia_exec::{Input, Lane, PixelShader};
use primase_core::parallel::Threads;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{PbrMaterial, Specular, TextureRef, UvTransform};
use primase_core::{ColorSpace, Entity, Name, NormalConvention, TexFormat, TexRole, Texture, World};
use primase_image::linear_to_srgb;
use primase_unity::bind::{bind, constants, directional_light, sampled_texture};
use primase_unity::components::{UnityMaterial, UnityPrograms, UnityTexEnv, UnityTexture};

/// The shader this pass bakes.
pub const VORONOI: &str = "Bumped Specular Voronoi";

/// The largest evaluation grid a side ([`iridia_exec::evaluate`]'s bound).
const GRID: u32 = 4096;

/// The level of detail a texture sample at an implicit level takes: the bake evaluates one pixel at a time, with no
/// quad footprint, at the map's own texel density, so the finest level (plus a bias the program or texture states).
const FOOTPRINTLESS_LEVEL: f32 = 0.0;

/// The five lights, towards the light: x, -x, y, -y, z.
const LIGHTS: [[f32; 3]; 5] = [
    [1.0, 0.0, 0.0],
    [-1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, -1.0, 0.0],
    [0.0, 0.0, 1.0],
];

/// What a texture property unset on the material samples: Unity's built-in texture its shader declares as the
/// default. `None` for a name this pass does not hold a value for.
fn declared_default(name: &str) -> Option<[f32; 4]> {
    match name {
        "white" => Some([1.0; 4]),
        // the flat normal: `UnpackNormal` reads x as r x a and y as g, each 2 v - 1, so (0, 0, 1)
        "bump" => Some([0.5, 0.5, 1.0, 1.0]),
        _ => None,
    }
}

/// Per texel of a baked block: the linear albedo and the unit tangent-space normal, from the five lit evaluations
/// (module documentation). `None` for the normal where the albedo is black.
pub fn recover(probes: [[f32; 3]; 5]) -> ([f32; 3], Option<[f32; 3]>) {
    let m = |c: usize| {
        [
            probes[0][c] - probes[1][c],
            probes[2][c] - probes[3][c],
            probes[4][c],
        ]
    };
    let length = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let albedo: [f32; 3] = std::array::from_fn(|c| length(m(c)));
    let (c, a) = albedo.iter().enumerate().fold(
        (0, 0.0f32),
        |best, (c, &a)| if a > best.1 { (c, a) } else { best },
    );
    let normal = (a > 1e-6).then(|| m(c).map(|x| x / a));
    (albedo, normal)
}

/// The tiling (scale, offset) of a property, identity when it has none or a non-finite one.
fn tiling(m: &UnityMaterial, property: &str) -> ([f32; 2], [f32; 2]) {
    m.textures
        .iter()
        .find(|t| t.name == property)
        .and_then(UnityTexEnv::finite_tiling)
        .unwrap_or(([1.0; 2], [0.0; 2]))
}

/// What a texture property of a material is to the bake.
enum Source<'a> {
    /// Its texture's pixels, with its sampler settings.
    Pixels(&'a Texture, &'a UnityTexture),
    /// Unset on the material: the shader's declared default.
    Unset,
    /// Named, but the world holds no pixels for it: why.
    Missing(&'static str),
}

fn source<'a>(w: &'a World, m: &UnityMaterial, property: &str) -> Source<'a> {
    let Some(env) = m.textures.iter().find(|t| t.name == property) else {
        return Source::Unset;
    };
    if !env.referenced {
        return Source::Unset;
    }
    let Some(e) = env.texture else {
        return Source::Missing("names an object not read as a texture");
    };
    match (w.get::<Texture>(e), w.get::<UnityTexture>(e)) {
        (Some(t), Some(u)) => Source::Pixels(t, u),
        (Some(_), None) => Source::Missing("names a texture without its sampler settings (unity.texture)"),
        (None, _) => Source::Missing("names a texture whose pixels were not decoded"),
    }
}

/// One baked block: per texel (row-major, v up: row 0 at v = 0) albedo and normal.
struct Block {
    size: u32,
    albedo: Vec<[f32; 3]>,
    normal: Vec<Option<[f32; 3]>>,
}

/// Bake `shader` with `constants` (material set, `_Color` white) and `textures` over `cells` x `cells` at `texels`
/// each, the map coordinate (TEXCOORD0's both pairs) the grid's: the five lit evaluations, each on `threads` threads
/// (a row of the grid each at a time), then [`recover`].
fn bake(
    threads: Threads,
    shader: &(dyn PixelShader + Sync),
    constants: &iridia_exec::evaluate::Constants,
    textures: &iridia_exec::Views,
    cells: u32,
    texels: u32,
) -> std::result::Result<Block, String> {
    // TEXCOORD0: both maps' coordinates; TEXCOORD1-3: the tangent-to-world rows (identity) and the world position
    // (zero), as Unity's surface-shader vertex program writes them
    let mut inputs = BTreeMap::new();
    inputs.insert(
        ("TEXCOORD".to_owned(), 0),
        Input::Lanes([Lane::U, Lane::V, Lane::U, Lane::V]),
    );
    for (i, row) in [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]
        .into_iter()
        .enumerate()
    {
        inputs.insert(("TEXCOORD".to_owned(), i as u32 + 1), Input::Constant(row));
    }
    let lit = LIGHTS
        .iter()
        .map(|&towards| {
            let mut c = constants.clone();
            directional_light(&mut c, towards, [1.0; 3]).map_err(|e| e.to_string())?;
            let e = iridia_exec::evaluate_periodic_with(
                threads,
                shader,
                &c,
                textures,
                &inputs,
                cells,
                texels,
                &[(0, 0b0111)],
            )
            .map_err(|e| e.to_string())?;
            e.targets
                .into_iter()
                .next()
                .flatten()
                .ok_or_else(|| "the program does not write o0.rgb".to_owned())
        })
        .collect::<std::result::Result<Vec<_>, String>>()?;
    let size = cells * texels;
    let (albedo, normal) = (0..(size * size) as usize)
        .map(|p| {
            recover(std::array::from_fn(|l| {
                let v = lit[l][p];
                [v[0], v[1], v[2]]
            }))
        })
        .unzip();
    Ok(Block { size, albedo, normal })
}

/// A baked map as a texture entity, rows flipped to the world's (top first), named after `name`.
fn map(w: &mut World, size: u32, name: String, normal: bool, texel: impl Fn(usize) -> [u8; 4]) -> Entity {
    let mut px = Vec::with_capacity((size * size * 4) as usize);
    for row in (0..size).rev() {
        for x in 0..size {
            px.extend(texel((row * size + x) as usize));
        }
    }
    let e = w.spawn();
    w.insert(e, Name(name));
    w.insert(
        e,
        Texture {
            width: size,
            height: size,
            format: TexFormat::Rgba8Unorm,
            levels: vec![px],
            color_space: if normal {
                ColorSpace::Linear
            } else {
                ColorSpace::Srgb
            },
            role: if normal { TexRole::Normal } else { TexRole::Color },
            normal: normal.then_some(NormalConvention::GLTF),
        },
    );
    e
}

fn byte(x: f64) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The slot of a map baked over `cells` cells of `property`'s coordinate.
fn slot(texture: Entity, m: &UnityMaterial, property: &str, cells: u32) -> TextureRef {
    let (scale, offset) = tiling(m, property);
    let k = cells as f32;
    TextureRef {
        transform: Some(UvTransform::from_bottom_left(
            scale.map(|s| s / k),
            offset.map(|o| o / k),
        )),
        ..TextureRef::new(texture)
    }
}

/// Why a material was left as `lower-unity-pbr` made it: the omission notice.
type Omitted = String;

/// The `Bumped Specular Voronoi` materials baked (module documentation), with the program's stated variant (`keywords`,
/// `tier`), block (`cells`) and resolution (`max_texels`). Made only by [`LowerSkatebirdVoronoi::new`] or from options
/// ([`LOWER_SKATEBIRD_VORONOI`]), which check them: its fields are private, read through its accessors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LowerSkatebirdVoronoi {
    keywords: Vec<String>,
    tier: usize,
    cells: u32,
    max_texels: u32,
}

impl LowerSkatebirdVoronoi {
    /// The pass with its stated options, checked: K at least 2, texels at least 1, the grid within 4096.
    pub fn new(keywords: Vec<String>, tier: usize, cells: u32, max_texels: u32) -> Result<Self, String> {
        if cells < 2 {
            return Err(format!("cells={cells}: a block that tiles needs at least 2"));
        }
        if max_texels == 0 {
            return Err("max-texels=0: at least one texel per cell".into());
        }
        if cells
            .checked_add(1)
            .and_then(|c| c.checked_mul(max_texels))
            .is_none_or(|grid| grid > GRID)
        {
            return Err(format!(
                "cells={cells}, max-texels={max_texels}: the evaluation grid, (cells + 1) x max-texels, is at most {GRID}"
            ));
        }
        Ok(LowerSkatebirdVoronoi {
            keywords,
            tier,
            cells,
            max_texels,
        })
    }

    /// The renderer's keywords the variant is stated with.
    pub fn keywords(&self) -> &[String] {
        &self.keywords
    }

    /// The hardware tier the variant is stated in.
    pub fn tier(&self) -> usize {
        self.tier
    }

    /// K: cells a side of the baked block.
    pub fn cells(&self) -> u32 {
        self.cells
    }

    /// The most texels per cell.
    pub fn max_texels(&self) -> u32 {
        self.max_texels
    }

    /// From its declared options, through [`LowerSkatebirdVoronoi::new`].
    fn from_given(g: &Given) -> Result<Self, String> {
        let number = |k: &str| -> Result<u32, String> {
            let v = g.required(k);
            v.parse().map_err(|_| format!("{k}={v}: a whole number expected"))
        };
        let keywords = g
            .required("keywords")
            .split(';')
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(str::to_owned)
            .collect();
        Self::new(
            keywords,
            number("tier")? as usize,
            number("cells")?,
            number("max-texels")?,
        )
    }

    /// Bake one material (module documentation): `Ok(Err(why))` when it is left out.
    fn lower(
        &self,
        w: &mut World,
        e: Entity,
        m: &UnityMaterial,
        programs: &UnityPrograms,
        log: &mut PassLog,
    ) -> Result<Result<(), Omitted>, String> {
        let name = w.get::<Name>(e).map_or_else(String::new, |n| n.0.clone());
        let Some(variant) = programs.variant(&m.keywords, &self.keywords, self.tier) else {
            return Ok(Err(format!(
                "no stored variant for keywords {:?} in tier {}",
                self.keywords, self.tier
            )));
        };
        let bound = bind(VORONOI, variant).map_err(|err| err.to_string())?;
        let mut mc = constants(&bound, m).map_err(|err| err.to_string())?;
        // the albedo baked without `_Color`: the material's base colour factor keeps it
        mc.constants
            .set("_Color", &[1.0; 4])
            .map_err(|err| err.to_string())?;
        let mut textures = BTreeMap::new();
        for &slot in bound.shader.textures() {
            let Some((property, _)) = bound.textures.iter().find(|(_, s)| **s == slot) else {
                return Ok(Err(format!(
                    "its program samples t{slot}, which no parameter names"
                )));
            };
            let texture = match source(w, m, property) {
                Source::Pixels(t, u) => {
                    if u.anisotropic() {
                        log.approximate(
                            &format!(
                                "{VORONOI}: textures Unity filters anisotropically, sampled isotropically"
                            ),
                            1,
                        );
                    }
                    iridia_exec::View::Image(Arc::new(
                        sampled_texture(t, u)
                            .map_err(|err| format!("{property}: {err}"))?
                            .with_footprintless_level(FOOTPRINTLESS_LEVEL)?,
                    ))
                }
                Source::Missing(why) => return Ok(Err(format!("{property} {why}"))),
                Source::Unset => {
                    let declared = programs
                        .texture_defaults
                        .iter()
                        .find(|(p, _)| p == property)
                        .map(|(_, d)| d.as_str());
                    match declared.and_then(declared_default) {
                        Some(texel) => {
                            log.count(
                                &format!(
                                    "{VORONOI}: {property} unset, its declared default \"{}\" sampled",
                                    declared.unwrap_or_default()
                                ),
                                1,
                            );
                            iridia_exec::View::Constant(texel)
                        }
                        None => {
                            return Ok(Err(format!(
                                "{property} is unset and its declared default ({}) has no value here",
                                declared.unwrap_or("none declared")
                            )));
                        }
                    }
                }
            };
            textures.insert(slot, texture);
        }
        let textures = iridia_exec::Views::refusing(textures);
        // the bindings the evaluation runs with: the material's, the extraction's, and any left zero
        let lights = {
            let mut probe = mc.constants.clone();
            directional_light(&mut probe, LIGHTS[0], [1.0; 3]).map_err(|err| err.to_string())?
        };
        let extraction: BTreeSet<&str> = lights.iter().map(String::as_str).chain(["_Color"]).collect();
        let material: Vec<&String> = bound
            .layout
            .fields
            .keys()
            .filter(|k| !mc.unset.contains(k) && !extraction.contains(k.as_str()))
            .collect();
        log.count(
            &format!(
                "{VORONOI}: fields the material sets: {}",
                material.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            ),
            1,
        );
        log.count(
            &format!(
                "{VORONOI}: fields the extraction sets (five white directional lights, no ambient, probes or \
                 shadows; _Color white; inputs: tangent frame identity, position zero): {}",
                extraction.iter().copied().collect::<Vec<_>>().join(", ")
            ),
            1,
        );
        for unresolved in mc.unset.iter().filter(|k| !extraction.contains(k.as_str())) {
            log.approximate(
                &format!("{VORONOI}: program fields neither the material nor the extraction sets, zero: {unresolved}"),
                1,
            );
        }
        let size = |property: &str| match source(w, m, property) {
            Source::Pixels(t, _) => Some(t.width.max(t.height)),
            _ => None,
        };
        let (colour, bump) = (size("_MainTex"), size("_BumpMap"));
        let mut texels = |n: u32| {
            if n > self.max_texels {
                log.approximate(
                    &format!(
                        "{VORONOI}: maps larger than {} baked at {} texels per cell",
                        self.max_texels, self.max_texels
                    ),
                    1,
                );
            }
            n.clamp(1, self.max_texels)
        };
        let shared = colour.is_some() && colour == bump && tiling(m, "_MainTex") == tiling(m, "_BumpMap");
        let threads = Threads::of(w);
        let mut baked = BTreeMap::new();
        if let Some(n) = colour {
            let t = texels(n);
            baked.insert(
                "_MainTex",
                bake(threads, &bound.shader, &mc.constants, &textures, self.cells, t)?,
            );
        }
        if let Some(n) = bump
            && !shared
        {
            let t = texels(n);
            baked.insert(
                "_BumpMap",
                bake(threads, &bound.shader, &mc.constants, &textures, self.cells, t)?,
            );
        }
        let mut pbr = w
            .get::<PbrMaterial>(e)
            .cloned()
            .ok_or("no pbr.material to replace")?;
        if let Some(block) = baked.get("_MainTex") {
            let t = map(w, block.size, format!("{name} (Voronoi colour)"), false, |i| {
                let a = block.albedo[i];
                [
                    byte(linear_to_srgb(f64::from(a[0]))),
                    byte(linear_to_srgb(f64::from(a[1]))),
                    byte(linear_to_srgb(f64::from(a[2]))),
                    255,
                ]
            });
            pbr.base_color_texture = Some(slot(t, m, "_MainTex", self.cells));
        }
        let normals = if shared { "_MainTex" } else { "_BumpMap" };
        if bump.is_some()
            && let Some(block) = baked.get(normals)
        {
            let flat = block.normal.iter().filter(|n| n.is_none()).count() as u64;
            if flat > 0 {
                log.count(&format!("{VORONOI}: texels of black albedo, normal flat"), flat);
            }
            let t = map(w, block.size, format!("{name} (Voronoi normal)"), true, |i| {
                let n = block.normal[i].unwrap_or([0.0, 0.0, 1.0]);
                [
                    byte(f64::from(n[0]) * 0.5 + 0.5),
                    byte(f64::from(n[1]) * 0.5 + 0.5),
                    byte(f64::from(n[2]) * 0.5 + 0.5),
                    255,
                ]
            });
            pbr.normal_texture = Some(slot(t, m, "_BumpMap", self.cells));
            pbr.normal_scale = 1.0;
        }
        pbr.metallic = 0.0;
        pbr.roughness = 1.0;
        pbr.specular = Some(Specular {
            factor: 0.0,
            ..Specular::default()
        });
        w.insert(e, pbr);
        log.count(
            &format!(
                "{VORONOI} baked: {name}, K {}, {} texels per cell",
                self.cells,
                baked.values().next().map_or(0, |b| b.size / self.cells)
            ),
            1,
        );
        log.approximate(
            &format!(
                "{VORONOI}: stochastic tiling baked as a {0} x {0}-cell block that tiles, its first cell \
                 cross-faded; Lambert, diffuse only",
                self.cells
            ),
            1,
        );
        log.approximate(
            &format!(
                "{VORONOI}: samples at an implicit level evaluated without a quad's footprint, at level \
                 {FOOTPRINTLESS_LEVEL} plus their bias"
            ),
            1,
        );
        Ok(Ok(()))
    }
}

/// `lower-skatebird-voronoi`'s declaration ([`primase_core::pass::PassRegistry::add`]): the variant, block and
/// resolution are the program's choices, every option required.
pub const LOWER_SKATEBIRD_VORONOI: PassSpec = PassSpec {
    name: "lower-skatebird-voronoi",
    options: &[
        OptionSpec::required(
            "keywords",
            "DIRECTIONAL",
            "the renderer's keywords the variant is stated with, joined by ; (possibly none)",
        ),
        OptionSpec::required("tier", "3", "the hardware tier the variant is stated in"),
        OptionSpec::required("cells", "2", "K: cells a side of the baked block, at least 2"),
        OptionSpec::required(
            "max-texels",
            "1024",
            "the most texels per cell, at least 1; (cells + 1) x max-texels is at most 4096",
        ),
    ],
    make: |g| Ok(Box::new(LowerSkatebirdVoronoi::from_given(g)?)),
};

impl Pass for LowerSkatebirdVoronoi {
    fn name(&self) -> &'static str {
        "lower-skatebird-voronoi"
    }
    fn options(&self) -> Options {
        Options::from([
            ("keywords".to_owned(), self.keywords.join(";")),
            ("tier".to_owned(), self.tier.to_string()),
            ("cells".to_owned(), self.cells.to_string()),
            ("max-texels".to_owned(), self.max_texels.to_string()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &["unity.material", "pbr.material"]
    }
    fn provides(&self) -> &[&'static str] {
        // the baked maps are new texture entities, named after their material
        &["core.texture", "core.name"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["pbr.material"]
    }
    fn approximates(&self) -> &'static str {
        "Bumped Specular Voronoi's stochastic tiling baked as a K x K-cell block that tiles, its first cell \
         cross-faded; its surface recovered from five lit evaluations"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let materials: Vec<(Entity, UnityMaterial)> = w
            .iter::<UnityMaterial>()
            .filter(|(_, m)| m.shader.as_deref() == Some(VORONOI))
            .map(|(e, m)| (e, m.clone()))
            .collect();
        for (e, m) in materials {
            let outcome = match w.get::<UnityPrograms>(e).cloned() {
                Some(p) => self.lower(w, e, &m, &p, log)?,
                None => Err("its programs were not read (unity.programs)".to_owned()),
            };
            if let Err(why) = outcome {
                log.omit(
                    &format!("{VORONOI} materials not baked, base colour only: {why}"),
                    1,
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// Five lit values of albedo a and unit normal n recover both: x and y from the differences of opposite lights,
    /// z from the light along it; a black texel has no normal.
    #[test]
    fn five_lit_values_recover_albedo_and_normal() {
        let a = [0.5f32, 0.25, 0.0];
        let n = [0.6f32, -0.48, 0.64];
        let lit = |l: [f32; 3]| {
            let d = (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]).max(0.0);
            a.map(|x| x * d)
        };
        let (albedo, normal) = recover(LIGHTS.map(lit));
        for c in 0..3 {
            assert!((albedo[c] - a[c]).abs() < 1e-6, "{albedo:?}");
        }
        let normal = normal.unwrap();
        for c in 0..3 {
            assert!((normal[c] - n[c]).abs() < 1e-6, "{normal:?}");
        }
        assert_eq!(recover([[0.0; 3]; 5]), ([0.0; 3], None));
    }

    fn material(textures: Vec<UnityTexEnv>) -> UnityMaterial {
        UnityMaterial {
            material: primase_unity::components::UnityObject {
                file: "sharedassets1.assets".into(),
                path_id: 1,
            },
            shader: Some(VORONOI.into()),
            shader_state: Vec::new(),
            textures,
            floats: Vec::new(),
            ints: Vec::new(),
            colors: Vec::new(),
            keywords: Vec::new(),
            tags: Vec::new(),
            render_queue: -1,
        }
    }

    fn env(name: &str, referenced: bool, texture: Option<Entity>) -> UnityTexEnv {
        UnityTexEnv {
            name: name.into(),
            referenced,
            texture,
            scale: [10.0, 3.0],
            offset: [0.0, 0.5],
        }
    }

    /// A map baked over K cells of a material tiled 10 x 3 takes 1 / K of that, from the bottom-left.
    #[test]
    fn a_baked_slot_tiles_over_its_cells() {
        let m = material(vec![env("_MainTex", false, None)]);
        let mut w = World::new();
        let t = w.spawn();
        assert_eq!(
            slot(t, &m, "_MainTex", 2).transform,
            Some(UvTransform::from_bottom_left([5.0, 1.5], [0.0, 0.25]))
        );
        assert_eq!(
            slot(t, &m, "_MainTex", 4).transform,
            Some(UvTransform::from_bottom_left([2.5, 0.75], [0.0, 0.125]))
        );
    }

    /// An unset property samples its shader's declared default; one naming an object not read as a texture, or a
    /// texture without pixels, is missing, not a default; only "white" and "bump" have values.
    #[test]
    fn unset_properties_take_declared_defaults_and_missing_sources_are_not_defaults() {
        let mut w = World::new();
        let undecoded = w.spawn();
        let m = material(vec![
            env("_Unset", false, None),
            env("_Other", true, None),
            env("_Undecoded", true, Some(undecoded)),
        ]);
        assert!(matches!(source(&w, &m, "_Unset"), Source::Unset));
        assert!(matches!(source(&w, &m, "_Absent"), Source::Unset));
        assert!(matches!(source(&w, &m, "_Other"), Source::Missing(_)));
        assert!(matches!(source(&w, &m, "_Undecoded"), Source::Missing(_)));
        assert_eq!(declared_default("white"), Some([1.0; 4]));
        assert_eq!(declared_default("bump"), Some([0.5, 0.5, 1.0, 1.0]));
        assert_eq!(declared_default("gray"), None);
    }

    /// Every option is required and checked; the grid bound refuses a block too large to evaluate.
    #[test]
    fn options_are_the_programs_choices() {
        let o = |kv: &[(&str, &str)]| -> Options {
            kv.iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        let all = [
            ("keywords", "DIRECTIONAL"),
            ("tier", "3"),
            ("cells", "2"),
            ("max-texels", "1024"),
        ];
        let make = |o: &Options| LOWER_SKATEBIRD_VORONOI.instance(o);
        let pass = LowerSkatebirdVoronoi::new(vec!["DIRECTIONAL".into()], 3, 2, 1024).unwrap();
        assert_eq!(make(&o(&all)).unwrap().options(), pass.options());
        assert_eq!(
            (pass.keywords(), pass.tier(), pass.cells(), pass.max_texels()),
            (&["DIRECTIONAL".to_owned()][..], 3, 2, 1024)
        );
        assert_eq!(pass.options(), o(&all));
        for missing in 0..all.len() {
            let mut fewer = all.to_vec();
            fewer.remove(missing);
            assert!(make(&o(&fewer)).is_err());
        }
        let with = |k: &str, v: &str| {
            let mut changed = all.to_vec();
            changed.retain(|(key, _)| *key != k);
            changed.push((k, v));
            make(&o(&changed))
        };
        assert!(with("cells", "1").is_err());
        assert!(with("max-texels", "0").is_err());
        assert!(with("max-texels", "2048").is_err());
        // no overflow on the grid check: refused, not wrapped or panicking
        assert!(LowerSkatebirdVoronoi::new(Vec::new(), 0, u32::MAX, 1).is_err());
        assert!(LowerSkatebirdVoronoi::new(Vec::new(), 0, 2, u32::MAX).is_err());
        assert!(with("tier", "x").is_err());
        assert!(with("extra", "1").is_err());
        assert_eq!(
            with("keywords", "").unwrap().options(),
            LowerSkatebirdVoronoi::new(Vec::new(), 3, 2, 1024)
                .unwrap()
                .options()
        );
    }
}
