//! `lower-anvil-water{rect=x0,y0,x1,y1,depth=L:D,...,river-depth=D,colour-max=X,roughness=R,shore=coverage|none}`: lakes
//! ([`AnvilWater`]) become water surfaces, and the river Materials (template FlowingWater_River) water materials, as
//! the Riders Republic notes' recipe for a static export states them (docs/material-spec.md §24.6: each value
//! established by the game or a stated choice):
//!
//! - **Which**: the patches whose squares overlap `rect` (x0, y0, x1, y1, source units; a lake's 1 km water cells
//!   reach past an area's ground), a lake with none left out, counted.
//! - **Geometry** (established): each patch a flat square of side `512 >> level` metres from its position (its
//!   least x and y) at the water level; its texture coordinates its atlas rectangle, `(u0, v0)` at its least x and y,
//!   u along +x and v along +y (§24.9: the lake vertex shader, pack 2186, read; a strip test across 71 neighbouring
//!   patches agrees). One prototype a lake, a primitive per level, its vertices local to its origin and placed once
//!   there (its box's least corner rounded down to 64 m: `local_origin`, exact).
//! - **Colour** (the hue the sun-light pass's model gives, a choice): per texel of the Params atlas its water type
//!   ([`water_type`]; a texel with none, `p = 0`, the default type `(4, 0.5, 5)`, §24.3), coloured by the hue of
//!   [`water_reflectance`] at 1 m, its greatest channel `colour-max`.
//! - **Alpha** (the shader's transmittance, established; its depth a choice): `1 - max exp(-sigma d)`
//!   ([`water_opacity`], [`lake_sigma`]) at the depth `depth` states for the patch's level, times the atlas's
//!   coverage (its alpha) where `shore=coverage`. Drawn blended.
//! - **Roughness** `roughness` (a choice: the water G-buffer has no roughness slot), metalness 0, no normal map (the
//!   lake's wave textures are engine-bound, not in the forge data): reported.
//!
//! - **Rivers** (their meshes as stored; their texture coordinates are flow vectors, which a material without textures
//!   doesn't read): the Material's water type from its seven weights ([`river_type`]), coloured as a lake's, its
//!   alpha the river shader's absorption ([`river_sigma`]) at `river-depth`, roughness and metalness as a lake's;
//!   its foam and wave normal (the engine's noise and wave textures) left out, reported. A weight the Material sets
//!   none of reads 0, as every Anvil property.
//!
//! A lake without a Params atlas (one of the 36) is left out, reported; foam and turbidity (the engine's noise) are
//! left out, reported.

use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef, Wrap};
use primase_core::{
    ColorSpace, Component, Instance, Mesh, Name, Primitive, Semantic, Stream, TexFormat, TexRole, Texture,
    Topology, Transform, VertexColor, World,
};

use super::formulas::{lake_sigma, river_sigma, river_type, water_opacity, water_reflectance, water_type};
use crate::components::{AnvilMaterial, AnvilWater, MaterialBinding, MaterialValue};

/// The default water type, for a texel that stores none (`p = 0`).
const DEFAULT_TYPE: [f64; 3] = [4.0, 0.5, 5.0];

/// Where the shore fades the water's alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shore {
    /// Times the Params atlas's coverage (its alpha).
    Coverage,
    /// Not faded.
    None,
}

/// See the module documentation.
#[derive(Clone, Debug, PartialEq)]
pub struct LowerAnvilWater {
    /// The rectangle (x0, y0, x1, y1, source units) whose patches are drawn.
    pub rect: [f64; 4],
    /// The water's depth in metres for a patch of each level.
    pub depth: BTreeMap<u32, f64>,
    /// The rivers' depth in metres.
    pub river_depth: f64,
    pub colour_max: f64,
    pub roughness: f32,
    pub shore: Shore,
}

impl LowerAnvilWater {
    /// From its options: `depth=L:D,...` (every level a lake's patches have), `colour-max`, `roughness`, `shore`, all
    /// required.
    pub fn from_options(o: &Options) -> Result<LowerAnvilWater, String> {
        let known = ["rect", "depth", "river-depth", "colour-max", "roughness", "shore"];
        if let Some(k) = o.keys().find(|k| !known.contains(&k.as_str())) {
            return Err(format!("unknown option {k}"));
        }
        let usage = "rect=x0,y0,x1,y1, depth=LEVEL:METRES,..., river-depth=METRES, colour-max=X, roughness=R, shore=coverage|none";
        let get = |k: &str| {
            o.get(k)
                .map(String::as_str)
                .ok_or_else(|| format!("{k} is required: {usage}"))
        };
        let rect: Vec<f64> = get("rect")?
            .split(',')
            .map(|v| v.trim().parse::<f64>())
            .collect::<Result<_, _>>()
            .map_err(|_| "rect is four numbers x0,y0,x1,y1".to_owned())?;
        let rect: [f64; 4] = rect
            .try_into()
            .map_err(|_| "rect is four numbers x0,y0,x1,y1".to_owned())?;
        if !(rect.iter().all(|v| v.is_finite()) && rect[0] < rect[2] && rect[1] < rect[3]) {
            return Err(format!("rect {rect:?}: finite, x0 < x1 and y0 < y1"));
        }
        let mut depth = BTreeMap::new();
        for item in get("depth")?.split(',') {
            let (level, metres) = item
                .split_once(':')
                .ok_or_else(|| format!("depth item {item:?} is not LEVEL:METRES"))?;
            let level: u32 = level
                .trim()
                .parse()
                .map_err(|_| format!("depth level {level:?}"))?;
            let metres: f64 = metres.trim().parse().map_err(|_| format!("depth {metres:?}"))?;
            if !(metres.is_finite() && metres > 0.0) {
                return Err(format!("depth {metres} at level {level}: finite and above 0"));
            }
            depth.insert(level, metres);
        }
        let number = |k: &str| -> Result<f64, String> {
            let v: f64 = get(k)?.parse().map_err(|_| format!("{k} is a number"))?;
            if v.is_finite() && v >= 0.0 {
                Ok(v)
            } else {
                Err(format!("{k} {v}: finite and at least 0"))
            }
        };
        let river_depth = number("river-depth")?;
        if river_depth <= 0.0 {
            return Err(format!("river-depth {river_depth}: above 0"));
        }
        let colour_max = number("colour-max")?;
        if colour_max > 1.0 {
            return Err(format!("colour-max {colour_max}: at most 1"));
        }
        let roughness = number("roughness")?;
        if roughness > 1.0 {
            return Err(format!("roughness {roughness}: at most 1"));
        }
        let shore = match get("shore")? {
            "coverage" => Shore::Coverage,
            "none" => Shore::None,
            v => return Err(format!("shore={v} is not coverage or none")),
        };
        Ok(LowerAnvilWater {
            rect,
            depth,
            river_depth,
            colour_max,
            roughness: roughness as f32,
            shore,
        })
    }
}

/// The water texture of `params` (its RGBA8 texels, `width` wide) at depth `d`: colour the type's hue, alpha its
/// opacity (times the coverage where `shore` says).
fn water_texture(texels: &[u8], width: u32, height: u32, d: f64, s: &LowerAnvilWater) -> Texture {
    let level = texels
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, a]| {
            let unorm = |x: u8| f64::from(x) / 255.0;
            let p = water_type([unorm(r), unorm(g), unorm(b)]);
            let p = if p == [0.0; 3] { DEFAULT_TYPE } else { p };
            let hue = water_reflectance(p, 1.0);
            let top = hue.iter().copied().fold(0.0, f64::max);
            let colour = hue.map(|c| if top > 0.0 { c / top * s.colour_max } else { 0.0 });
            let mut alpha = water_opacity(lake_sigma(p), d);
            if s.shore == Shore::Coverage {
                alpha *= unorm(a);
            }
            let srgb = |x: f64| primase_image::linear_to_srgb8(x);
            [
                srgb(colour[0]),
                srgb(colour[1]),
                srgb(colour[2]),
                (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
            ]
        })
        .collect();
    Texture {
        width,
        height,
        format: TexFormat::Rgba8Unorm,
        levels: vec![level],
        color_space: ColorSpace::Srgb,
        role: TexRole::Color,
        normal: None,
    }
}

impl Pass for LowerAnvilWater {
    fn name(&self) -> &'static str {
        "lower-anvil-water"
    }
    fn options(&self) -> Options {
        let [x0, y0, x1, y1] = self.rect;
        Options::from([
            ("rect".to_owned(), format!("{x0},{y0},{x1},{y1}")),
            (
                "depth".to_owned(),
                self.depth
                    .iter()
                    .map(|(l, d)| format!("{l}:{d}"))
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            ("river-depth".to_owned(), self.river_depth.to_string()),
            ("colour-max".to_owned(), self.colour_max.to_string()),
            ("roughness".to_owned(), self.roughness.to_string()),
            (
                "shore".to_owned(),
                match self.shore {
                    Shore::Coverage => "coverage",
                    Shore::None => "none",
                }
                .to_owned(),
            ),
        ])
    }
    // no `requires`: a world may hold lakes, river Materials, both or neither
    fn provides(&self) -> &[&'static str] {
        &[
            Mesh::NAME,
            PbrMaterial::NAME,
            Texture::NAME,
            Instance::NAME,
            Transform::NAME,
            Name::NAME,
            VertexColor::NAME,
        ]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilWater::NAME, AnvilMaterial::NAME]
    }
    fn approximates(&self) -> &'static str {
        "lakes as flat blended water at stated depths, coloured by the water type's hue; no waves, foam or turbidity"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let lakes: Vec<_> = w.iter::<AnvilWater>().map(|(e, _)| e).collect();
        let mut lowered = 0u64;
        for e in lakes {
            let Some(lake) = w.remove::<AnvilWater>(e) else {
                continue;
            };
            let Some(params) = lake.atlases[1] else {
                log.omit("lakes without a Params atlas: left out", 1);
                continue;
            };
            let atlas = w
                .get::<Texture>(params)
                .ok_or("a lake's Params atlas holds no texture")?;
            let meta = primase_image::TextureMetadata {
                color_space: ColorSpace::Linear,
                role: TexRole::Other,
                normal: None,
            };
            let decoded = primase_image::map_rgba8_levels(atlas, 0..1, meta, |x| Ok(x.rgba))
                .map_err(|e| format!("Params atlas: {e}"))?;
            let texels = decoded.levels[0].clone();
            let mut by_level: BTreeMap<u32, Vec<&crate::components::WaterPatch>> = BTreeMap::new();
            let [x0, y0, x1, y1] = self.rect;
            for p in &lake.patches {
                let side = p.side().map_or(0.0, f64::from);
                let [x, y] = [f64::from(p.position[0]), f64::from(p.position[1])];
                if x < x1 && x + side > x0 && y < y1 && y + side > y0 {
                    by_level.entry(p.level).or_default().push(p);
                } else {
                    log.omit("lake patches outside the rectangle", 1);
                }
            }
            if by_level.is_empty() {
                log.omit("lakes with no patch in the rectangle", 1);
                continue;
            }
            let flip = lake.frame.flips_winding();
            let mut primitives = Vec::new();
            for (level, patches) in by_level {
                let d = *self
                    .depth
                    .get(&level)
                    .ok_or_else(|| format!("a lake's patches of level {level}: no depth stated for it"))?;
                let side = 512u32
                    .checked_shr(level)
                    .ok_or_else(|| format!("patch level {level}: past 512's bits"))?
                    as f32;
                let texture = w.spawn();
                w.insert(
                    texture,
                    water_texture(&texels, decoded.width, decoded.height, d, self),
                );
                let material = w.spawn();
                w.insert(
                    material,
                    PbrMaterial {
                        base_color_texture: Some(TextureRef {
                            wrap: [Wrap::Clamp; 2],
                            ..TextureRef::new(texture)
                        }),
                        metallic: 0.0,
                        roughness: self.roughness,
                        alpha_mode: AlphaMode::Blend,
                        ..PbrMaterial::default()
                    },
                );
                let (mut positions, mut uvs, mut indices) = (Vec::new(), Vec::new(), Vec::new());
                for p in &patches {
                    let [x, y, z] = p.position;
                    let [u0, v0, u1, v1] = p.rect;
                    let first = u32::try_from(positions.len()).map_err(|_| "too many patches")?;
                    for ([dx, dy], uv) in [
                        ([0.0, 0.0], [u0, v0]),
                        ([side, 0.0], [u1, v0]),
                        ([side, side], [u1, v1]),
                        ([0.0, side], [u0, v1]),
                    ] {
                        positions.push(lake.frame.point([x + dx, y + dy, z]));
                        uvs.push(uv);
                    }
                    // counter-clockwise seen from +z (the water's up) in the source frame
                    for tri in [[0, 1, 2], [0, 2, 3]] {
                        let tri = tri.map(|k| first + k);
                        indices.extend(if flip { [tri[0], tri[2], tri[1]] } else { tri });
                    }
                }
                log.count(&format!("lake patches of level {level}"), patches.len() as u64);
                let mut streams = BTreeMap::new();
                streams.insert(Semantic::Position, Stream::F32x3(positions));
                streams.insert(Semantic::TexCoord(0), Stream::F32x2(uvs));
                primitives.push(Primitive {
                    topology: Topology::Triangles,
                    streams,
                    indices: Some(indices),
                    material: Some(material),
                });
            }
            // the vertices local to the lake's origin
            let all: Vec<[f32; 3]> = primitives
                .iter()
                .filter_map(|p| p.positions())
                .flat_map(|p| p.iter().copied())
                .collect();
            let origin = super::local_origin(&all);
            for p in &mut primitives {
                if let Some(Stream::F32x3(v)) = p.streams.get_mut(&Semantic::Position) {
                    for q in v.iter_mut() {
                        *q = std::array::from_fn(|k| q[k] - origin[k]);
                    }
                }
            }
            w.insert(e, Mesh::new(primitives));
            let placed = w.spawn();
            w.insert(
                placed,
                Instance {
                    prototype: e,
                    variant: 0,
                },
            );
            w.insert(placed, super::placed_at(origin));
            if let Some(name) = w.get::<Name>(e).cloned() {
                w.insert(placed, name);
            }
            log.place(placed);
            lowered += 1;
        }
        // the river Materials
        let rivers: Vec<_> = w
            .iter::<AnvilMaterial>()
            .filter(|(_, m)| m.template_name.as_deref() == Some("FlowingWater_River"))
            .map(|(e, _)| e)
            .collect();
        for e in &rivers {
            let Some(m) = w.remove::<AnvilMaterial>(*e) else {
                continue;
            };
            // the weights at the river shader's cb3[6..12]
            let weight = |vec4: u32| -> f64 {
                let Some(&(key, _)) = m
                    .bindings
                    .iter()
                    .find(|(_, b)| *b == MaterialBinding::Scalar { vec4 })
                else {
                    return 0.0;
                };
                m.values
                    .iter()
                    .find_map(|(k, v)| match v {
                        MaterialValue::Float(x) if *k == key => Some(f64::from(*x)),
                        _ => None,
                    })
                    .unwrap_or(0.0)
            };
            let p = river_type(std::array::from_fn(|k| weight(6 + k as u32)));
            let hue = water_reflectance(p, 1.0);
            let top = hue.iter().copied().fold(0.0, f64::max);
            let colour = hue.map(|c| {
                if top > 0.0 {
                    (c / top * self.colour_max) as f32
                } else {
                    0.0
                }
            });
            let alpha = water_opacity(river_sigma(p), self.river_depth) as f32;
            w.insert(
                *e,
                PbrMaterial {
                    base_color: [colour[0], colour[1], colour[2], alpha],
                    metallic: 0.0,
                    roughness: self.roughness,
                    alpha_mode: AlphaMode::Blend,
                    ..PbrMaterial::default()
                },
            );
            w.insert(*e, VertexColor::Ignored);
        }
        if !rivers.is_empty() {
            log.count("river Materials lowered", rivers.len() as u64);
            log.omit(
                "rivers' foam and wave normal (the engine's noise and wave textures; their flow-vector UVs unread)",
                rivers.len() as u64,
            );
        }
        if lowered > 0 {
            log.count("lakes lowered", lowered);
            log.omit(
                "lakes' waves, foam and turbidity (the engine's noise and wave textures, not in the forge data)",
                lowered,
            );
        }
        Ok(())
    }
}
