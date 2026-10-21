//! The terrain's near ground colour as a bake: what the game's terrain pixel shader draws close to the camera, by the
//! Riders Republic notes' reading of it (riders-republic docs/terrain.md §3.4-3.5; each value classed there), for the
//! nearest detail level, at every pixel of a rectangle.
//!
//! For a point (x, y) in source units, `P` its texel coordinate on the level's grid (a node's 128 texels to its side):
//!
//! 1. the splatting noise `n` (its red and green, bilinear, repeating over each node's 128 texels; as stored or
//!    decoded as sRGB, `noise`: the notes' choice raw) jitters the lookup, `q = P + (2 n - 1) / 2` texels;
//! 2. the four material texels about `q`: `floor(q)` and its neighbours towards `sign(frac(q) - 0.5)` along each axis,
//!    from the nodes' material rasters (block 2), each texel the value at its integer place;
//! 3. their detail colours blended per axis by `w = 1 / (1 + exp(k (0.5 - |f|)))`, `f = frac(q) - 0.5`, in linear
//!    light (`k` the program's choice: unmeasured; the notes find 6, 12 and 24 alike);
//! 4. a material's detail colour: its TextureSet's diffuse at uv `(x, y) g`, repeating, `g` its uv scale (the
//!    TerrainMaterial's f32 at 2, the engine's per-material record's: established) times a global factor the notes
//!    didn't find (`uv`: a stated factor, or `over-tile-size`, 1 / the f32 at 6, a lead); sampled bilinearly at the mip
//!    whose texel is nearest the bake's pixel (this pass's choice: the engine's own filtering at a distance isn't
//!    read);
//! 5. times `0.5 + b`, `b` block 8 bilinear at `P` (`occlusion`; block 8 as that texture is the notes' lead);
//! 6. encoded sRGB.
//!
//! A pixel whose material has no detail diffuse takes the far colour tile there (bilinear, decoded in the colour space
//! the pass states), counted. A tile's texel i covers [i, i + 1) of the node's texel coordinate, as the material
//! raster's integer lookup has it, for block 8 and the colour tile alike (where the engine samples those two isn't
//! read). Left out (the notes': not read, or not reproducible here): the altitude colour ramps, the
//! detail normals, heights and masks, the steep-slope normal, the per-cell uv rotation, the cell-hash term, the
//! lighting, and the distance blend to the far tile.

use std::collections::BTreeMap;
use std::rc::Rc;

use primase_core::{ColorSpace, Entity, TexFormat, TexRole, Texture, World};
use primase_image::{TextureMetadata, map_rgba8_levels, srgb_to_linear};

use crate::components::{AnvilTerrain, AnvilTextureMap, TerrainTile};
use crate::read::terrain::CORE_AT;

/// How a detail's uv scale becomes uv per source unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SplatUv {
    /// Times this factor.
    Factor(f64),
    /// Over the TerrainMaterial's f32 at 6.
    OverTileSize,
}

/// The bake's stated choices.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Splat {
    pub k: f64,
    /// Source units per pixel.
    pub pixel: f64,
    pub uv: SplatUv,
    /// Whether the noise is decoded as sRGB.
    pub noise_srgb: bool,
    /// Whether the colour is multiplied by `0.5 + block 8`.
    pub occlusion: bool,
}

/// What the bake counted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Baked {
    pub pixels: u64,
    /// Pixels a material of which has no detail diffuse (the far tile's colour there).
    pub tile_fallback: u64,
    /// Pixels outside every read node (left black).
    pub outside: u64,
}

/// An RGBA8 image's levels, row by row.
struct Levels {
    width: u32,
    height: u32,
    levels: Vec<Vec<u8>>,
}

impl Levels {
    fn of(t: &Texture) -> Result<Levels, String> {
        let meta = TextureMetadata {
            color_space: ColorSpace::Linear,
            role: TexRole::Other,
            normal: None,
        };
        let d = map_rgba8_levels(t, 0..t.levels.len(), meta, |x| Ok(x.rgba)).map_err(|e| e.to_string())?;
        Ok(Levels {
            width: d.width,
            height: d.height,
            levels: d.levels,
        })
    }

    fn size(&self, level: usize) -> (u32, u32) {
        let s = u32::try_from(level).unwrap_or(31);
        ((self.width >> s).max(1), (self.height >> s).max(1))
    }

    fn texel(&self, level: usize, x: i64, y: i64) -> [u8; 4] {
        let (w, h) = self.size(level);
        let (x, y) = (
            x.rem_euclid(i64::from(w)) as usize,
            y.rem_euclid(i64::from(h)) as usize,
        );
        let i = (y * w as usize + x) * 4;
        let l = &self.levels[level];
        [l[i], l[i + 1], l[i + 2], l[i + 3]]
    }

    /// Bilinear, repeating, at uv `(u, v)` (texel centres at half-integers), each channel by `decode`.
    fn sample(&self, level: usize, u: f64, v: f64, decode: impl Fn(u8) -> f64) -> [f64; 4] {
        let (w, h) = self.size(level);
        let (x, y) = (u * f64::from(w) - 0.5, v * f64::from(h) - 0.5);
        let (fx, fy) = (x.floor(), y.floor());
        let (tx, ty) = (x - fx, y - fy);
        let (ix, iy) = (fx as i64, fy as i64);
        let mut out = [0.0; 4];
        for (dx, dy, wt) in [
            (0, 0, (1.0 - tx) * (1.0 - ty)),
            (1, 0, tx * (1.0 - ty)),
            (0, 1, (1.0 - tx) * ty),
            (1, 1, tx * ty),
        ] {
            let t = self.texel(level, ix + dx, iy + dy);
            for c in 0..4 {
                out[c] += wt * decode(t[c]);
            }
        }
        out
    }
}

/// A material's detail, ready to sample.
struct Detail {
    image: Rc<Levels>,
    srgb: bool,
    /// uv per source unit.
    g: f64,
}

fn unorm(x: u8) -> f64 {
    f64::from(x) / 255.0
}

/// Material `m`'s detail into `details`, its diffuse decoded once into `images`; `None` where it has no detail
/// diffuse (or, over its tile size, none of that).
fn detail(
    w: &World,
    t: &AnvilTerrain,
    s: &Splat,
    m: u8,
    images: &mut BTreeMap<Entity, Rc<Levels>>,
    details: &mut BTreeMap<u8, Option<Detail>>,
) -> Result<(), String> {
    if details.contains_key(&m) {
        return Ok(());
    }
    let d = t.details.get(usize::from(m)).and_then(Option::as_ref);
    let made = match (d, d.and_then(|d| d.diffuse)) {
        (Some(d), Some(e)) => {
            let g = match s.uv {
                SplatUv::Factor(f) => Some(f64::from(d.uv_scale) * f),
                SplatUv::OverTileSize if d.f32_6 > 0.0 => Some(f64::from(d.uv_scale) / f64::from(d.f32_6)),
                SplatUv::OverTileSize => None,
            };
            match (g, w.get::<Texture>(e)) {
                (Some(g), Some(tex)) => {
                    let image = match images.get(&e) {
                        Some(i) => i.clone(),
                        None => {
                            let i = Rc::new(Levels::of(tex)?);
                            images.insert(e, i.clone());
                            i
                        }
                    };
                    Some(Detail {
                        image,
                        srgb: w.get::<AnvilTextureMap>(e).is_some_and(|m| m.field_25 == 1),
                        g,
                    })
                }
                _ => None,
            }
        }
        _ => None,
    };
    details.insert(m, made);
    Ok(())
}

/// The bilinear value at global texel coordinate `(gx, gy)` (texel `i` covering `[i, i + 1)`) of the texels `fetch`
/// gives, each from its own node, so a node's edge blends with its neighbour's texels; a texel `fetch` has none of
/// counts as 0.
fn across(gx: f64, gy: f64, fetch: impl Fn(i64, i64) -> Option<f64>) -> f64 {
    let (x, y) = (gx - 0.5, gy - 0.5);
    let (fx, fy) = (x.floor(), y.floor());
    let (tx, ty) = (x - fx, y - fy);
    let (ix, iy) = (fx as i64, fy as i64);
    [
        (0, 0, (1.0 - tx) * (1.0 - ty)),
        (1, 0, tx * (1.0 - ty)),
        (0, 1, (1.0 - tx) * ty),
        (1, 1, tx * ty),
    ]
    .into_iter()
    .map(|(dx, dy, w)| w * fetch(ix + dx, iy + dy).unwrap_or(0.0))
    .sum()
}

/// One node's tile as images: its far colour (BC1) and block 8 (BC4), 132 x 132 each.
struct Node<'t> {
    tile: &'t TerrainTile,
    colour: Option<Levels>,
    block_8: Option<Levels>,
}

fn tile_image(format: TexFormat, blocks: &[u8], colour: ColorSpace) -> Option<Levels> {
    (blocks.len() == 33 * 33 * 8)
        .then(|| {
            Levels::of(&Texture {
                width: 132,
                height: 132,
                format,
                levels: vec![blocks.to_vec()],
                color_space: colour,
                role: TexRole::Other,
                normal: None,
            })
            .ok()
        })
        .flatten()
}

/// The rectangle `rect` (x0, y0, x1, y1, source units) of the terrain `t`'s ground baked as `s` states, an sRGB RGBA8
/// image a pixel of `s.pixel` units to its side from (x0, y0) along +x and +y (row 0 at y0), and what it counted.
/// `tile_colour` is the far tiles' colour space.
pub fn bake(
    w: &World,
    t: &AnvilTerrain,
    rect: [f64; 4],
    s: &Splat,
    tile_colour: ColorSpace,
) -> Result<(Texture, Baked), String> {
    let level = t.level.min(30);
    let node_side = t.side / f64::from(1u32 << level);
    let texel = node_side / 128.0;
    let [x0, y0, x1, y1] = rect;
    let width = ((x1 - x0) / s.pixel).ceil().max(1.0) as u32;
    let height = ((y1 - y0) / s.pixel).ceil().max(1.0) as u32;
    let noise = match t.noise.and_then(|e| w.get::<Texture>(e)) {
        Some(n) => Levels::of(n)?,
        None => return Err("the splat bake needs the splatting noise (TerrainRead's noise)".into()),
    };
    // each material's detail, decoded when a pixel first needs it (a bank holds a hundred 2048-texel maps)
    let mut images: BTreeMap<Entity, Rc<Levels>> = BTreeMap::new();
    let mut details: BTreeMap<u8, Option<Detail>> = BTreeMap::new();
    let nodes: BTreeMap<(i64, i64), Node> = t
        .tiles
        .iter()
        .map(|tile| {
            (
                (i64::from(tile.row), i64::from(tile.column)),
                Node {
                    tile,
                    colour: tile_image(TexFormat::Bc1, &tile.colour, tile_colour),
                    block_8: tile_image(TexFormat::Bc4Unorm, &tile.block_8, ColorSpace::Linear),
                },
            )
        })
        .collect();
    // the node and its local texel of a global texel
    let locate = |gx: i64, gy: i64| -> Option<(&Node, usize, usize)> {
        let n = nodes.get(&(gy.div_euclid(128), gx.div_euclid(128)))?;
        Some((
            n,
            gy.rem_euclid(128) as usize + CORE_AT,
            gx.rem_euclid(128) as usize + CORE_AT,
        ))
    };
    let material = |gx: i64, gy: i64| -> Option<u8> {
        let (n, i, j) = locate(gx, gy)?;
        n.tile.material.get(i * 132 + j).copied()
    };
    let decode_noise = |x: u8| {
        if s.noise_srgb {
            srgb_to_linear(unorm(x))
        } else {
            unorm(x)
        }
    };
    let mut out = vec![0u8; width as usize * height as usize * 4];
    let mut baked = Baked::default();
    for py in 0..height {
        for px in 0..width {
            baked.pixels += 1;
            let x = x0 + (f64::from(px) + 0.5) * s.pixel;
            let y = y0 + (f64::from(py) + 0.5) * s.pixel;
            let (gx, gy) = ((x - t.origin[0]) / texel, (y - t.origin[1]) / texel);
            let (nx, ny) = (gx.div_euclid(128.0), gy.div_euclid(128.0));
            if !nodes.contains_key(&(ny as i64, nx as i64)) {
                baked.outside += 1;
                continue;
            }
            let (lx, ly) = (gx - nx * 128.0, gy - ny * 128.0);
            let n = noise.sample(0, lx / 128.0, ly / 128.0, decode_noise);
            let (qx, qy) = (gx + (2.0 * n[0] - 1.0) * 0.5, gy + (2.0 * n[1] - 1.0) * 0.5);
            let (cx, cy) = (qx.floor() as i64, qy.floor() as i64);
            let (fx, fy) = (qx - qx.floor() - 0.5, qy - qy.floor() - 0.5);
            let (sx, sy) = (if fx >= 0.0 { 1 } else { -1 }, if fy >= 0.0 { 1 } else { -1 });
            let weight = |f: f64| 1.0 / (1.0 + (s.k * (0.5 - f.abs())).exp());
            let (wx, wy) = (weight(fx), weight(fy));
            let corners = [(cx, cy), (cx + sx, cy), (cx, cy + sy), (cx + sx, cy + sy)];
            let materials = corners.map(|(mx, my)| material(mx, my));
            for m in materials.iter().flatten() {
                detail(w, t, s, *m, &mut images, &mut details)?;
            }
            let mut fell_back = false;
            let mut colour_of = |m: Option<u8>| -> [f64; 3] {
                let detail = m.and_then(|m| details.get(&m)).and_then(Option::as_ref);
                match detail {
                    Some(d) => {
                        let level0 = f64::from(d.image.width) * d.g * s.pixel;
                        let top = d.image.levels.len().saturating_sub(1);
                        let mip = (level0.max(1.0).log2().round() as usize).min(top);
                        let decode = |c: u8| {
                            if d.srgb {
                                srgb_to_linear(unorm(c))
                            } else {
                                unorm(c)
                            }
                        };
                        let c = d.image.sample(mip, x * d.g, y * d.g, decode);
                        [c[0], c[1], c[2]]
                    }
                    None => {
                        fell_back = true;
                        let decode = |c: u8| match tile_colour {
                            ColorSpace::Srgb => srgb_to_linear(unorm(c)),
                            _ => unorm(c),
                        };
                        [0, 1, 2].map(|k| {
                            across(gx, gy, |tx, ty| {
                                let (n, i, j) = locate(tx, ty)?;
                                Some(decode(n.colour.as_ref()?.texel(0, j as i64, i as i64)[k]))
                            })
                        })
                    }
                }
            };
            let [c00, c10, c01, c11] = materials.map(&mut colour_of);
            if fell_back {
                baked.tile_fallback += 1;
            }
            let lerp = |a: [f64; 3], b: [f64; 3], t: f64| [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * t);
            let mut c = lerp(lerp(c00, c10, wx), lerp(c01, c11, wx), wy);
            if s.occlusion {
                let b = across(gx, gy, |tx, ty| {
                    let (n, i, j) = locate(tx, ty)?;
                    Some(unorm(n.block_8.as_ref()?.texel(0, j as i64, i as i64)[0]))
                });
                c = c.map(|v| v * (0.5 + b));
            }
            let i = (py as usize * width as usize + px as usize) * 4;
            for k in 0..3 {
                out[i + k] = primase_image::linear_to_srgb8(c[k]);
            }
            out[i + 3] = 255;
        }
    }
    Ok((
        Texture {
            width,
            height,
            format: TexFormat::Rgba8Unorm,
            levels: vec![out],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        },
        baked,
    ))
}
