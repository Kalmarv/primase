//! Unity terrains lowered to geometry (pass `lower-unity-terrain`): each `unity.terrain` that draws its heightmap gets
//! a prototype with one primitive at the heightmap's full resolution, placed by the terrain's entity.
//!
//! - Sample (x, z) is at (x * scale.x, h / 32766 * scale.y, z * scale.z) from the terrain's position (in Unity's frame,
//!   converted); every cell (x, z)-(x + 1, z + 1) not a hole is two triangles split along that diagonal. Unity's own
//!   choice of diagonal per cell is not read (counted).
//! - Normals by central differences of the heights (one-sided at the edges), as `TerrainData.GetInterpolatedNormal`.
//! - Texture coordinates span the terrain, (0, 0) at its far corner along -z (the world's top-left origin).
//! - The material is a base colour image at the alphamap's resolution: each texel's layer weights times each layer's
//!   mean diffuse colour (linear, times its diffuse remap maximum), encoded as sRGB. The layers' tiled textures and the
//!   terrain's material template are not reproduced (counted).

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::{PbrMaterial, TextureRef, Wrap};
use primase_core::{
    ColorSpace, Entity, Instance, Mesh, Name, Primitive, Semantic, Stream, TexFormat, TexRole, Texture,
    Topology, World,
};
use primase_image::{linear_to_srgb, srgb_to_linear};
use primase_texcodec::{Decoder, TexDecoder};

use crate::components::{UnityMaterial, UnityTerrain, UnityTerrainLayer};
use crate::level::UNITY;
use crate::{Result, invalid};

fn rgba8(t: &Texture) -> Result<Vec<u8>> {
    Decoder::default()
        .decode_rgba8(t.format, t.width, t.height, &t.levels[0])
        .map_err(|e| invalid(format!("terrain texture: {e}")))
}

/// A layer's mean diffuse colour, linear, times its remap maximum; `None` without decoded pixels.
fn layer_colour(w: &World, layer: Entity) -> Result<Option<[f64; 3]>> {
    let Some(l) = w.get::<UnityTerrainLayer>(layer) else {
        return Ok(None);
    };
    let Some(t) = l.diffuse.and_then(|d| w.get::<Texture>(d)) else {
        return Ok(None);
    };
    let px = rgba8(t)?;
    let mut sum = [0.0f64; 3];
    for p in px.as_chunks::<4>().0 {
        for c in 0..3 {
            let x = f64::from(p[c]) / 255.0;
            sum[c] += if t.color_space == ColorSpace::Srgb {
                srgb_to_linear(x)
            } else {
                x
            };
        }
    }
    let n = (px.len() / 4) as f64;
    Ok(Some(
        [0, 1, 2].map(|c| sum[c] / n * f64::from(l.diffuse_remap[1][c])),
    ))
}

/// The terrain's base colour image: alphamap weights times layer colours, sRGB, rows top first as the alphamaps are.
fn splat(w: &World, t: &UnityTerrain, log: &mut PassLog) -> Result<Option<Texture>> {
    let mut colours = Vec::with_capacity(t.layers.len());
    for l in &t.layers {
        match l.map(|l| layer_colour(w, l)).transpose()?.flatten() {
            Some(c) => colours.push(c),
            None => {
                log.count("terrain layers without a decoded diffuse texture (black)", 1);
                colours.push([0.0; 3]);
            }
        }
    }
    let n = t.alphamap_resolution;
    let mut maps = Vec::new();
    for a in &t.alphamaps {
        let Some(tex) = a.and_then(|a| w.get::<Texture>(a)) else {
            log.count("terrains without decoded alphamaps (no base colour image)", 1);
            return Ok(None);
        };
        if (tex.width, tex.height) != (n, n) {
            return Err(invalid(format!(
                "alphamap {}x{} for resolution {n}",
                tex.width, tex.height
            )));
        }
        maps.push(rgba8(tex)?);
    }
    let mut out = vec![0u8; (n * n * 4) as usize];
    for (i, p) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let mut lin = [0.0f64; 3];
        for (l, c) in colours.iter().enumerate() {
            let Some(m) = maps.get(l / 4) else { break };
            let weight = f64::from(m[i * 4 + l % 4]) / 255.0;
            for k in 0..3 {
                lin[k] += weight * c[k];
            }
        }
        for k in 0..3 {
            p[k] = (linear_to_srgb(lin[k]).clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        p[3] = 255;
    }
    Ok(Some(Texture {
        width: n,
        height: n,
        format: TexFormat::Rgba8Unorm,
        levels: vec![out],
        color_space: ColorSpace::Srgb,
        role: TexRole::Color,
        normal: None,
    }))
}

/// The terrain's primitive in its own space (world axes).
pub fn terrain_primitive(t: &UnityTerrain, material: Option<Entity>) -> Primitive {
    let n = t.resolution as usize;
    let [sx, sy, sz] = t.scale.map(f64::from);
    let h = |x: usize, z: usize| f64::from(t.heights[z * n + x]) / 32766.0 * sy;
    let mut positions = Vec::with_capacity(n * n);
    let mut normals = Vec::with_capacity(n * n);
    let mut uv = Vec::with_capacity(n * n);
    let last = (n - 1) as f64;
    for z in 0..n {
        for x in 0..n {
            positions.push(UNITY.point([(x as f64 * sx) as f32, h(x, z) as f32, (z as f64 * sz) as f32]));
            let (x0, x1) = (x.saturating_sub(1), (x + 1).min(n - 1));
            let (z0, z1) = (z.saturating_sub(1), (z + 1).min(n - 1));
            let gx = (h(x1, z) - h(x0, z)) / ((x1 - x0) as f64 * sx);
            let gz = (h(x, z1) - h(x, z0)) / ((z1 - z0) as f64 * sz);
            let l = (gx * gx + 1.0 + gz * gz).sqrt();
            normals.push(UNITY.direction([(-gx / l) as f32, (1.0 / l) as f32, (-gz / l) as f32]));
            uv.push([(x as f64 / last) as f32, (1.0 - z as f64 / last) as f32]);
        }
    }
    let mut indices = Vec::with_capacity((n - 1) * (n - 1) * 6);
    for z in 0..n - 1 {
        for x in 0..n - 1 {
            if t.holes.as_ref().is_some_and(|h| h[z * (n - 1) + x] == 0) {
                continue;
            }
            let a = (z * n + x) as u32;
            let (b, c) = (a + 1, a + n as u32);
            let d = c + 1;
            // Unity's (a, c, d) and (a, d, b), reversed for the world's mirrored frame
            indices.extend_from_slice(&[a, d, c, a, b, d]);
        }
    }
    Primitive {
        topology: Topology::Triangles,
        streams: [
            (Semantic::Position, Stream::F32x3(positions)),
            (Semantic::Normal, Stream::F32x3(normals)),
            (Semantic::TexCoord(0), Stream::F32x2(uv)),
        ]
        .into_iter()
        .collect(),
        indices: Some(indices),
        material,
    }
}

/// Lower every drawn `unity.terrain` not yet placed (see the module documentation).
pub fn lower_unity_terrain(w: &mut World) -> Result<PassLog> {
    let mut log = PassLog::default();
    let terrains: Vec<(Entity, UnityTerrain)> = w
        .iter::<UnityTerrain>()
        .filter(|(e, t)| t.draw_heightmap && !w.has::<Instance>(*e))
        .map(|(e, t)| (e, t.clone()))
        .collect();
    for (e, t) in terrains {
        let name = w.get::<Name>(e).map(|n| n.0.clone()).unwrap_or_default();
        let material = match splat(w, &t, &mut log)? {
            Some(tex) => {
                let te = w.spawn();
                w.insert(te, Name(format!("{name} (base colour)")));
                w.insert(te, tex);
                let me = w.spawn();
                w.insert(me, Name(format!("{name} (terrain)")));
                w.insert(
                    me,
                    PbrMaterial {
                        base_color_texture: Some(TextureRef {
                            wrap: [Wrap::Clamp; 2],
                            ..TextureRef::new(te)
                        }),
                        metallic: 0.0,
                        roughness: 1.0,
                        ..PbrMaterial::default()
                    },
                );
                log.approximate("terrain layers by their mean colour (tiling detail lost)", 1);
                Some(me)
            }
            None => None,
        };
        if let Some(shader) = t
            .material
            .and_then(|m| w.get::<UnityMaterial>(m))
            .and_then(|m| m.shader.clone())
        {
            log.approximate(&format!("terrain material template not lowered: {shader}"), 1);
        }
        let prim = terrain_primitive(&t, material);
        log.count(
            "terrain triangles",
            prim.indices.as_ref().map_or(0, |i| i.len() as u64 / 3),
        );
        log.approximate("terrain cells split along one diagonal", 1);
        let p = w.spawn();
        w.insert(p, Name(format!("{name} (terrain)")));
        w.insert(
            p,
            Mesh {
                primitives: vec![prim],
            },
        );
        w.insert(
            e,
            Instance {
                prototype: p,
                variant: 0,
            },
        );
        log.count("terrains lowered", 1);
    }
    Ok(log)
}

/// [`lower_unity_terrain`] as a pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct LowerUnityTerrain;

impl Pass for LowerUnityTerrain {
    fn name(&self) -> &'static str {
        "lower-unity-terrain"
    }
    fn requires(&self) -> &[&'static str] {
        &["unity.terrain"]
    }
    fn provides(&self) -> &[&'static str] {
        &["core.mesh", "core.instance", "core.texture", "pbr.material"]
    }
    fn approximates(&self) -> &'static str {
        "one diagonal per terrain cell; terrain layers by their mean colour; the terrain material not lowered"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let r = lower_unity_terrain(w).map_err(|e| e.to_string())?;
        for (k, n) in r.counts {
            log.count(&k, n);
        }
        for (k, n) in r.approximations {
            log.approximate(&k, n);
        }
        Ok(())
    }
}
