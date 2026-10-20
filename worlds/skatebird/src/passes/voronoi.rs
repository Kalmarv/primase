//! `lower-skatebird-voronoi`: the materials of SkateBIRD's `Bumped Specular Voronoi` shader, baked from the shader's own
//! compiled program (`unity.program`, read by [`crate::read::read_voronoi_programs`]) into a colour and a normal map.
//!
//! What the shader is, from its forward-base fragment program (one directional light, D3D11 ps_4_0) and its parameter
//! record: a `Lambert` surface (no specular term, whatever its name) whose `_MainTex` and `_BumpMap` are tiled
//! stochastically. Each is sampled at the 3x3 cells around floor(uv) (uv the map's own `TRANSFORM_TEX` coordinate),
//! each cell c offset by h = frac(sin(dot(c, (12.9898, 78.233))) x 43758.5453), weighted 2^(-7.2135 |c - frac(uv) +
//! h|^2) and normalised: the albedo that blend x `_Color`, the normal the same blend of the normal map, unpacked. Its
//! pattern never repeats, so no texture holds it exactly over a surface.
//!
//! The bake (approximated, counted): [`primase_shader::evaluate_periodic`] over a block of K x K cells of the map's
//! coordinate (K = 2, the smallest whose block stays within 2048 texels a side at the map's own texel density: a map
//! larger than 1024 is baked at 1024 per cell, counted), its first cell cross-faded so the block tiles. The program's
//! output is lit, so the surface comes out of five evaluations, each with one white directional light (x, -x, y, -y,
//! z) and nothing else ([`primase_unity::bind::directional_light`]: ambient, light probes and shadows off), the
//! tangent frame the identity and `_Color` white: each is max(0, n.l) x albedo, so per colour channel the vector
//! (P[x] - P[-x], P[y] - P[-y], P[z]) is the albedo times the unit normal: its length the albedo, and the normal its
//! direction in the channel of the largest albedo (flat where the albedo is black, counted). When the two maps have
//! one tiling one bake gives both; otherwise each is baked in its own coordinate.
//!
//! The material keeps `lower-unity-pbr`'s base colour factor (`_Color`, linear) and takes the colour map (sRGB) and
//! the normal map (glTF's convention) in place of its base map, each slot's transform the map's tiling over the K
//! cells (`_ST` / K, origin moved to top-left); diffuse only (`KHR_materials_specular` factor 0), rough, not metallic.

use std::collections::BTreeMap;
use std::sync::Arc;

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::{PbrMaterial, Specular, TextureRef, UvTransform, Wrap};
use primase_core::{ColorSpace, Entity, Name, NormalConvention, TexFormat, TexRole, Texture, World};
use primase_image::linear_to_srgb;
use primase_shader::{Input, Lane, PixelShader};
use primase_unity::bind::{UnityImage, bind, constants, directional_light};
use primase_unity::components::{UnityMaterial, UnityProgram, UnityTexEnv, UnityTexture};

/// The shader this pass bakes.
pub const VORONOI: &str = "Bumped Specular Voronoi";

/// Cells per axis of the baked block, and the largest texels per cell (the block at most 2048 a side).
const CELLS: u32 = 2;
const MAX_TEXELS: u32 = 1024;

/// The five lights, towards the light: x, -x, y, -y, z.
const LIGHTS: [[f32; 3]; 5] = [
    [1.0, 0.0, 0.0],
    [-1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, -1.0, 0.0],
    [0.0, 0.0, 1.0],
];

/// Unity's default texture of a property without one: "bump" (flat, whatever its packing) for the normal map, else
/// white.
fn default_texture(property: &str) -> [f32; 4] {
    if property == "_BumpMap" {
        [0.5, 0.5, 1.0, 1.0]
    } else {
        [1.0; 4]
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

/// A material's texture property: its env and its decoded texture entity, when it has one.
fn decoded<'a>(w: &World, m: &'a UnityMaterial, property: &str) -> Option<(&'a UnityTexEnv, Entity)> {
    let env = m.texture(property)?;
    let e = env.texture?;
    w.has::<Texture>(e).then_some((env, e))
}

/// The tiling (scale, offset) of a property, identity when it has none or a non-finite one.
fn tiling(m: &UnityMaterial, property: &str) -> ([f32; 2], [f32; 2]) {
    m.textures
        .iter()
        .find(|t| t.name == property)
        .and_then(UnityTexEnv::finite_tiling)
        .unwrap_or(([1.0; 2], [0.0; 2]))
}

/// One baked block: per texel (row-major, v up: row 0 at v = 0) albedo and normal.
struct Block {
    size: u32,
    albedo: Vec<[f32; 3]>,
    normal: Vec<Option<[f32; 3]>>,
}

/// Bake `shader` with `constants` (material set, `_Color` white) and `textures` over K x K cells at `texels` each,
/// the map coordinate (TEXCOORD0's both pairs) the grid's: the five lit evaluations in parallel, then [`recover`].
fn bake(
    shader: &(dyn PixelShader + Sync),
    constants: &primase_shader::Constants,
    textures: &BTreeMap<u32, primase_shader::Texture>,
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
    let lit: Vec<std::result::Result<Vec<[f32; 4]>, String>> = std::thread::scope(|s| {
        let handles: Vec<_> = LIGHTS
            .iter()
            .map(|&towards| {
                let inputs = &inputs;
                s.spawn(move || {
                    let mut c = constants.clone();
                    directional_light(&mut c, towards, [1.0; 3]).map_err(|e| e.to_string())?;
                    let e = primase_shader::evaluate_periodic(
                        shader,
                        &c,
                        textures,
                        inputs,
                        CELLS,
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
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err("a bake thread panicked".to_owned()))
            })
            .collect()
    });
    let lit = lit.into_iter().collect::<std::result::Result<Vec<_>, _>>()?;
    let size = CELLS * texels;
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

/// The slot of a map baked over K cells of `property`'s coordinate.
fn slot(texture: Entity, m: &UnityMaterial, property: &str) -> TextureRef {
    let (scale, offset) = tiling(m, property);
    let k = CELLS as f32;
    TextureRef {
        texture,
        texcoord: 0,
        wrap: [Wrap::Repeat; 2],
        transform: Some(UvTransform::from_bottom_left(
            scale.map(|s| s / k),
            offset.map(|o| o / k),
        )),
    }
}

/// Bake one material (module documentation).
fn lower(
    w: &mut World,
    e: Entity,
    m: &UnityMaterial,
    p: &UnityProgram,
    log: &mut PassLog,
) -> Result<(), String> {
    let name = w.get::<Name>(e).map_or_else(String::new, |n| n.0.clone());
    let bound = bind(p).map_err(|err| err.to_string())?;
    let mut mc = constants(&bound, m).map_err(|err| err.to_string())?;
    for unset in &mc.unset {
        log.count(
            &format!("{VORONOI}: program fields no property sets, left zero: {unset}"),
            1,
        );
    }
    // the albedo baked without `_Color`: the material's base colour factor keeps it
    mc.constants
        .set("_Color", &[1.0; 4])
        .map_err(|err| err.to_string())?;
    let mut textures = BTreeMap::new();
    for &slot in bound.shader.textures() {
        let property = bound
            .textures
            .iter()
            .find(|(_, s)| **s == slot)
            .map_or("", |(n, _)| n.as_str());
        let texture = match decoded(w, m, property) {
            Some((_, t)) => {
                let (wrap, filter) = w
                    .get::<UnityTexture>(t)
                    .map_or(([0, 0], 1), |u| (u.wrap, u.filter));
                let texture = w.get::<Texture>(t).ok_or("a decoded texture without pixels")?;
                primase_shader::Texture::Image(Arc::new(
                    UnityImage::new(texture, wrap, filter).map_err(|err| err.to_string())?,
                ))
            }
            None => {
                log.count(
                    &format!("{VORONOI}: texture properties at Unity's default ({property})"),
                    1,
                );
                primase_shader::Texture::Constant(default_texture(property))
            }
        };
        textures.insert(slot, texture);
    }
    let size = |property: &str| {
        decoded(w, m, property)
            .and_then(|(_, t)| w.get::<Texture>(t))
            .map(|t| t.width.max(t.height))
    };
    let (colour, bump) = (size("_MainTex"), size("_BumpMap"));
    let mut texels = |n: u32| {
        if n > MAX_TEXELS {
            log.approximate(
                &format!("{VORONOI}: maps larger than {MAX_TEXELS} baked at {MAX_TEXELS} texels per cell"),
                1,
            );
        }
        n.clamp(1, MAX_TEXELS)
    };
    let shared = colour.is_some() && colour == bump && tiling(m, "_MainTex") == tiling(m, "_BumpMap");
    let mut baked = BTreeMap::new();
    if let Some(n) = colour {
        baked.insert(
            "_MainTex",
            bake(&bound.shader, &mc.constants, &textures, texels(n))?,
        );
    }
    if let Some(n) = bump
        && !shared
    {
        baked.insert(
            "_BumpMap",
            bake(&bound.shader, &mc.constants, &textures, texels(n))?,
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
        pbr.base_color_texture = Some(slot(t, m, "_MainTex"));
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
        pbr.normal_texture = Some(slot(t, m, "_BumpMap"));
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
            "{VORONOI} baked: {name}, K {CELLS}, {} texels per cell",
            baked.values().next().map_or(0, |b| b.size / CELLS)
        ),
        1,
    );
    log.approximate(
        &format!(
            "{VORONOI}: stochastic tiling baked as a {CELLS} x {CELLS}-cell block that tiles, its first cell \
             cross-faded; Lambert, diffuse only"
        ),
        1,
    );
    Ok(())
}

/// The `Bumped Specular Voronoi` materials baked (module documentation); a material of the shader without its program
/// (`unity.program`) keeps `lower-unity-pbr`'s base colour, counted.
#[derive(Debug, Clone, Copy)]
pub struct LowerSkatebirdVoronoi;

impl Pass for LowerSkatebirdVoronoi {
    fn name(&self) -> &'static str {
        "lower-skatebird-voronoi"
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
        "Bumped Specular Voronoi's stochastic tiling baked as a 2 x 2-cell block that tiles, its first cell \
         cross-faded; its surface recovered from five lit evaluations"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let materials: Vec<(Entity, UnityMaterial)> = w
            .iter::<UnityMaterial>()
            .filter(|(_, m)| m.shader.as_deref() == Some(VORONOI))
            .map(|(e, m)| (e, m.clone()))
            .collect();
        for (e, m) in materials {
            let Some(p) = w.get::<UnityProgram>(e).cloned() else {
                log.count(
                    &format!("{VORONOI}: materials without their program, base colour only"),
                    1,
                );
                continue;
            };
            lower(w, e, &m, &p, log)?;
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

    /// A map baked over two cells of a material tiled 10 x 3 takes a fifth and a half of that, from the bottom-left.
    #[test]
    fn a_baked_slot_tiles_over_its_cells() {
        let mut m = UnityMaterial {
            material: primase_unity::components::UnityObject {
                file: "sharedassets1.assets".into(),
                path_id: 1,
            },
            shader: Some(VORONOI.into()),
            shader_state: Vec::new(),
            textures: Vec::new(),
            floats: Vec::new(),
            ints: Vec::new(),
            colors: Vec::new(),
            keywords: Vec::new(),
            tags: Vec::new(),
            render_queue: -1,
        };
        m.textures.push(UnityTexEnv {
            name: "_MainTex".into(),
            texture: None,
            scale: [10.0, 3.0],
            offset: [0.0, 0.5],
        });
        let mut w = World::new();
        let t = w.spawn();
        let r = slot(t, &m, "_MainTex");
        assert_eq!(
            r.transform,
            Some(UvTransform::from_bottom_left([5.0, 1.5], [0.0, 0.25]))
        );
        assert_eq!(default_texture("_BumpMap"), [0.5, 0.5, 1.0, 1.0]);
    }
}
