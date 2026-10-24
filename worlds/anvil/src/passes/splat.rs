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
//! the pass states), counted. A node whose colour tile or block 8 is needed and isn't decoded (not 33 x 33 blocks: the
//! reader's are, `tnl_code::tile` decompresses exactly) is an error naming it, never read as 0; a node outside the read
//! ones counts as 0 where the bilinear blend reaches it. A tile's texel i covers [i, i + 1) of the node's texel coordinate, as the material
//! raster's integer lookup has it, for block 8 and the colour tile alike (where the engine samples those two isn't
//! read). Left out (the notes': not read, or not reproducible here): the altitude colour ramps, the
//! detail normals, heights and masks, the steep-slope normal, the per-cell uv rotation, the cell-hash term, the
//! lighting, and the distance blend to the far tile.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use primase_core::parallel::{self, Threads};

use primase_core::{ColorSpace, Entity, TexFormat, TexRole, Texture, World};
use primase_image::sampling::{Address, Levels, Transfer};
use primase_image::srgb8_to_linear;

use crate::components::{AnvilTerrain, AnvilTextureMap, TerrainTile};

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

/// A material's detail, ready to sample.
struct Detail {
    image: Arc<Levels>,
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
    uv: SplatUv,
    m: u8,
    images: &mut BTreeMap<Entity, Arc<Levels>>,
    details: &mut BTreeMap<u8, Option<Detail>>,
) -> Result<(), String> {
    if details.contains_key(&m) {
        return Ok(());
    }
    let d = t.details.get(usize::from(m)).and_then(Option::as_ref);
    let made = match (d, d.and_then(|d| d.diffuse)) {
        (Some(d), Some(e)) => {
            let g = match uv {
                SplatUv::Factor(f) => Some(f64::from(d.uv_scale) * f),
                SplatUv::OverTileSize if d.f32_6 > 0.0 => Some(f64::from(d.uv_scale) / f64::from(d.f32_6)),
                SplatUv::OverTileSize => None,
            };
            match (g, w.get::<Texture>(e)) {
                (Some(g), Some(tex)) => {
                    let image = match images.get(&e) {
                        Some(i) => i.clone(),
                        None => {
                            let i = Arc::new(Levels::of(tex).map_err(|e| e.to_string())?);
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
/// gives, each from its own node, so a node's edge blends with its neighbour's texels; a texel of a node that isn't
/// read (`fetch` gives none) counts as 0, and a texel `fetch` can't give (its node's tile not decoded) is an error.
fn across(gx: f64, gy: f64, fetch: impl Fn(i64, i64) -> Result<Option<f64>, String>) -> Result<f64, String> {
    let (x, y) = (gx - 0.5, gy - 0.5);
    let (fx, fy) = (x.floor(), y.floor());
    let (tx, ty) = (x - fx, y - fy);
    let (ix, iy) = (fx as i64, fy as i64);
    let mut terms = [0.0; 4];
    for (term, (dx, dy, w)) in terms.iter_mut().zip([
        (0, 0, (1.0 - tx) * (1.0 - ty)),
        (1, 0, tx * (1.0 - ty)),
        (0, 1, (1.0 - tx) * ty),
        (1, 1, tx * ty),
    ]) {
        *term = w * fetch(ix + dx, iy + dy)?.unwrap_or(0.0);
    }
    // summed as before (`Iterator::sum`, in order)
    Ok(terms.into_iter().sum())
}

/// One node's tile as images: its far colour (BC1) and block 8 (BC4), 132 x 132 each, or why a tile isn't one (read
/// only where a bake needs it: [`Node::tile`]).
struct Node<'t> {
    tile: &'t TerrainTile,
    colour: Result<Levels, String>,
    block_8: Result<Levels, String>,
}

impl Node<'_> {
    /// Its `which` tile (`colour` or `block_8`), or an error naming the node and why the tile isn't decoded.
    fn tile<'a>(&self, which: &'a Result<Levels, String>, name: &str) -> Result<&'a Levels, String> {
        which.as_ref().map_err(|why| {
            format!(
                "terrain node (row {}, column {}): its {name} tile isn't decoded: {why}",
                self.tile.row, self.tile.column
            )
        })
    }
}

/// A node's stored tile as an image: 33 x 33 blocks of 8 bytes, 132 x 132 texels; anything else (a node read without
/// the tile, a tile of another size, blocks that don't decode) is the reason it isn't one.
fn tile_image(format: TexFormat, blocks: &[u8], colour: ColorSpace) -> Result<Levels, String> {
    if blocks.len() != 33 * 33 * 8 {
        return Err(format!("{} bytes, not 33 x 33 blocks of 8", blocks.len()));
    }
    Levels::of(&Texture {
        width: 132,
        height: 132,
        format,
        levels: vec![blocks.to_vec()],
        color_space: colour,
        role: TexRole::Other,
        normal: None,
    })
    .map_err(|e| e.to_string())
}

/// The near ground's material lookup (steps 1 to 3 of the module documentation): the nodes' rasters, the noise and
/// `k`, for the bake and for the weights at points alike.
pub struct Lookup<'t> {
    nodes: BTreeMap<(i64, i64), Node<'t>>,
    noise: Levels,
    noise_transfer: Transfer,
    /// Source units per texel, and the quadtree's corner.
    texel: f64,
    origin: [f64; 2],
    k: f64,
}

/// The four material texels about a point and their blend: `materials` this texel, its x neighbour, its y
/// neighbour and the diagonal (`None` outside every read node's raster), blended per axis by `wx`, `wy`; the
/// point's global texel coordinate `(gx, gy)`.
pub struct Corners {
    pub materials: [Option<u8>; 4],
    pub wx: f64,
    pub wy: f64,
    pub gx: f64,
    pub gy: f64,
}

impl Corners {
    /// Each corner's weight: `(1 - wx)(1 - wy)`, `wx (1 - wy)`, `(1 - wx) wy`, `wx wy` (the bilinear blend's).
    pub fn weights(&self) -> [f64; 4] {
        let (wx, wy) = (self.wx, self.wy);
        [(1.0 - wx) * (1.0 - wy), wx * (1.0 - wy), (1.0 - wx) * wy, wx * wy]
    }
}

impl<'t> Lookup<'t> {
    /// The lookup of `t`'s ground at blend sharpness `k`, its noise as stored or sRGB-decoded; `tile_colour` the far
    /// tiles' colour space.
    pub fn new(
        w: &World,
        t: &'t AnvilTerrain,
        k: f64,
        noise_srgb: bool,
        tile_colour: ColorSpace,
    ) -> Result<Self, String> {
        let level = t.level.min(30);
        let node_side = t.side / f64::from(1u32 << level);
        let noise = match t.noise.and_then(|e| w.get::<Texture>(e)) {
            Some(n) => Levels::of(n).map_err(|e| e.to_string())?,
            None => {
                return Err(
                    "the ground's material lookup needs the splatting noise (TerrainRead's noise)".into(),
                );
            }
        };
        let nodes = t
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
        Ok(Lookup {
            nodes,
            noise,
            noise_transfer: if noise_srgb {
                Transfer::SrgbColour
            } else {
                Transfer::Stored
            },
            texel: node_side / 128.0,
            origin: t.origin,
            k,
        })
    }

    /// The node and its local texel of a global texel.
    fn locate(&self, gx: i64, gy: i64) -> Option<(&Node<'t>, usize, usize)> {
        let n = self.nodes.get(&(gy.div_euclid(128), gx.div_euclid(128)))?;
        Some((
            n,
            gy.rem_euclid(128) as usize + TerrainTile::CORE_AT,
            gx.rem_euclid(128) as usize + TerrainTile::CORE_AT,
        ))
    }

    fn material(&self, gx: i64, gy: i64) -> Option<u8> {
        let (n, i, j) = self.locate(gx, gy)?;
        n.tile.material.get(i * 132 + j).copied()
    }

    /// The occlusion factor at global texel coordinate `(gx, gy)`: `0.5 + b`, `b` block 8 bilinear there (step 5); a
    /// node whose block 8 isn't decoded is an error.
    pub fn occlusion(&self, gx: f64, gy: f64) -> Result<f64, String> {
        let b = across(gx, gy, |tx, ty| {
            let Some((n, i, j)) = self.locate(tx, ty) else {
                return Ok(None);
            };
            let image = n.tile(&n.block_8, "block 8")?;
            Ok(Some(unorm(
                image.texel(0, j as i64, i as i64, [Address::Wrap; 2])[0],
            )))
        })?;
        Ok(0.5 + b)
    }

    /// The four material texels about `(x, y)` (source units) and their blend; `None` where the point's own node
    /// isn't read.
    pub fn corners(&self, x: f64, y: f64) -> Result<Option<Corners>, String> {
        let (gx, gy) = (
            (x - self.origin[0]) / self.texel,
            (y - self.origin[1]) / self.texel,
        );
        let (nx, ny) = (gx.div_euclid(128.0), gy.div_euclid(128.0));
        if !self.nodes.contains_key(&(ny as i64, nx as i64)) {
            return Ok(None);
        }
        let (lx, ly) = (gx - nx * 128.0, gy - ny * 128.0);
        let n = self
            .noise
            .bilinear(
                0,
                [lx / 128.0, ly / 128.0],
                [Address::Wrap; 2],
                self.noise_transfer,
            )
            .map_err(|e| e.to_string())?;
        let (qx, qy) = (gx + (2.0 * n[0] - 1.0) * 0.5, gy + (2.0 * n[1] - 1.0) * 0.5);
        let (cx, cy) = (qx.floor() as i64, qy.floor() as i64);
        let (fx, fy) = (qx - qx.floor() - 0.5, qy - qy.floor() - 0.5);
        let (sx, sy) = (if fx >= 0.0 { 1 } else { -1 }, if fy >= 0.0 { 1 } else { -1 });
        let weight = |f: f64| 1.0 / (1.0 + (self.k * (0.5 - f.abs())).exp());
        let corners = [(cx, cy), (cx + sx, cy), (cx, cy + sy), (cx + sx, cy + sy)];
        Ok(Some(Corners {
            materials: corners.map(|(mx, my)| self.material(mx, my)),
            wx: weight(fx),
            wy: weight(fy),
            gx,
            gy,
        }))
    }
}

/// A terrain material's detail as a layer: its TextureSet's diffuse, its detail normal if it has one, and its uv per
/// source unit (step 4); `None` where it has no detail diffuse (or, over its tile size, none of that).
pub fn layer_detail(t: &AnvilTerrain, uv: SplatUv, m: u8) -> Option<(Entity, Option<Entity>, f64)> {
    let d = t.details.get(usize::from(m))?.as_ref()?;
    let g = match uv {
        SplatUv::Factor(f) => f64::from(d.uv_scale) * f,
        SplatUv::OverTileSize if d.f32_6 > 0.0 => f64::from(d.uv_scale) / f64::from(d.f32_6),
        SplatUv::OverTileSize => return None,
    };
    Some((d.diffuse?, d.normal, g))
}

/// Which coordinates of a point address the material details.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DetailPlane {
    /// The flat and mixed programs' (ps 1908): the detail at the point's (x, y).
    Xy,
    /// The vertical program's (ps 1924, `TerrainTesselVerticalOnly`, docs/terrain.md 3.10 (3)): the whole near colour
    /// at the point's (x, z) and again at its (y, z), mixed by `w` (0: all (x, z); 1: all (y, z)), `w` the program's
    /// `saturate(5 (n.x^2 / (n.x^2 + n.y^2) - 0.4))` of the pixel's normal.
    Vertical { w: f64 },
}

impl DetailPlane {
    /// The vertical program's mix of the two planes at the pixel normal `n` (source frame); 0 where `n` has no
    /// horizontal part.
    pub fn vertical(n: [f64; 3]) -> DetailPlane {
        let h = n[0] * n[0] + n[1] * n[1];
        let w = if h > 0.0 {
            (5.0 * (n[0] * n[0] / h - 0.4)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        DetailPlane::Vertical { w }
    }
}

/// The near ground colour at points of the terrain: the lookup, the details decoded as pixels first need them, the
/// bake's choices. [`bake`] runs it over rectangles; `lower-anvil-terrain`'s charts run it at the points of a steep face.
pub struct Baker<'a> {
    w: &'a World,
    t: &'a AnvilTerrain,
    s: &'a Splat,
    tile_colour: ColorSpace,
    lookup: Lookup<'a>,
    /// Each material's detail, decoded when a pixel first needs it (a bank holds a hundred 2048-texel maps).
    images: BTreeMap<Entity, Arc<Levels>>,
    details: BTreeMap<u8, Option<Detail>>,
}

/// A baked point: its sRGB8 colour, and whether a material of it had no detail diffuse (its far colour tile there).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Texel {
    pub rgb: [u8; 3],
    pub tile_fallback: bool,
}

impl<'a> Baker<'a> {
    pub fn new(
        w: &'a World,
        t: &'a AnvilTerrain,
        s: &'a Splat,
        tile_colour: ColorSpace,
    ) -> Result<Self, String> {
        Ok(Baker {
            w,
            t,
            s,
            tile_colour,
            lookup: Lookup::new(w, t, s.k, s.noise_srgb, tile_colour)?,
            images: BTreeMap::new(),
            details: BTreeMap::new(),
        })
    }

    /// The colour at the point `(x, y, z)` (source units) as `plane` addresses the details, for a pixel `pixel` units
    /// across (it picks the detail's mip); `None` where the point's node isn't read. The details it needs are decoded
    /// first.
    pub fn texel(
        &mut self,
        x: f64,
        y: f64,
        z: f64,
        plane: DetailPlane,
        pixel: f64,
    ) -> Result<Option<Texel>, String> {
        let Some(corners) = self.lookup.corners(x, y)? else {
            return Ok(None);
        };
        self.prepare(corners.materials.iter().flatten().copied())?;
        Ok(Some(self.shade(corners, x, y, z, plane, pixel)?))
    }

    /// The materials `ms`' details decoded (each once over the baker's life).
    fn prepare(&mut self, ms: impl IntoIterator<Item = u8>) -> Result<(), String> {
        for m in ms {
            detail(self.w, self.t, self.s.uv, m, &mut self.images, &mut self.details)?;
        }
        Ok(())
    }

    /// [`Baker::texel`] at `corners`, the point's, whose materials' details are prepared.
    fn shade(
        &self,
        corners: Corners,
        x: f64,
        y: f64,
        z: f64,
        plane: DetailPlane,
        pixel: f64,
    ) -> Result<Texel, String> {
        let Corners {
            materials,
            wx,
            wy,
            gx,
            gy,
        } = corners;
        let mut fell_back = false;
        let mut at = |baker: &Self, coord: [f64; 2]| -> Result<[[f64; 3]; 4], String> {
            let mut out = [[0.0; 3]; 4];
            for (o, m) in out.iter_mut().zip(materials) {
                *o = baker.colour_of(m, coord, (gx, gy), pixel, &mut fell_back)?;
            }
            Ok(out)
        };
        let colours = match plane {
            DetailPlane::Xy => at(self, [x, y])?,
            DetailPlane::Vertical { w } => {
                let xz = (w < 1.0).then(|| at(self, [x, z])).transpose()?;
                let yz = (w > 0.0).then(|| at(self, [y, z])).transpose()?;
                match (xz, yz) {
                    (Some(a), Some(b)) => {
                        std::array::from_fn(|c| std::array::from_fn(|k| a[c][k] + (b[c][k] - a[c][k]) * w))
                    }
                    (Some(a), None) => a,
                    (None, Some(b)) => b,
                    (None, None) => unreachable!("w is below 1 or above 0"),
                }
            }
        };
        let [c00, c10, c01, c11] = colours;
        let lerp = |a: [f64; 3], b: [f64; 3], t: f64| [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * t);
        let mut c = lerp(lerp(c00, c10, wx), lerp(c01, c11, wx), wy);
        if self.s.occlusion {
            let o = self.lookup.occlusion(gx, gy)?;
            c = c.map(|v| v * o);
        }
        Ok(Texel {
            rgb: c.map(primase_image::linear_to_srgb8),
            tile_fallback: fell_back,
        })
    }

    /// Material `m`'s colour (linear) with its detail sampled at `coord` (source units), or its far colour tile at the
    /// global texel `(gx, gy)` where it has no detail diffuse (setting `fell_back`).
    fn colour_of(
        &self,
        m: Option<u8>,
        coord: [f64; 2],
        (gx, gy): (f64, f64),
        pixel: f64,
        fell_back: &mut bool,
    ) -> Result<[f64; 3], String> {
        let detail = m.and_then(|m| self.details.get(&m)).and_then(Option::as_ref);
        match detail {
            Some(d) => {
                let level0 = f64::from(d.image.width) * d.g * pixel;
                let top = d.image.levels.len().saturating_sub(1);
                let mip = (level0.max(1.0).log2().round() as usize).min(top);
                let transfer = if d.srgb {
                    Transfer::SrgbColour
                } else {
                    Transfer::Stored
                };
                let c = d
                    .image
                    .bilinear(
                        mip,
                        [coord[0] * d.g, coord[1] * d.g],
                        [Address::Wrap; 2],
                        transfer,
                    )
                    .map_err(|e| e.to_string())?;
                Ok([c[0], c[1], c[2]])
            }
            None => {
                *fell_back = true;
                let decode = |c: u8| match self.tile_colour {
                    ColorSpace::Srgb => srgb8_to_linear(c),
                    _ => unorm(c),
                };
                let mut c = [0.0; 3];
                for (k, c) in c.iter_mut().enumerate() {
                    *c = across(gx, gy, |tx, ty| {
                        let Some((n, i, j)) = self.lookup.locate(tx, ty) else {
                            return Ok(None);
                        };
                        let image = n.tile(&n.colour, "colour")?;
                        Ok(Some(decode(
                            image.texel(0, j as i64, i as i64, [Address::Wrap; 2])[k],
                        )))
                    })?;
                }
                Ok(c)
            }
        }
    }
}

/// Each rectangle of `rects` (x0, y0, x1, y1, source units) of the terrain `t`'s ground baked as `s` states, an sRGB
/// RGBA8 image a pixel of `s.pixel` units to its side from (x0, y0) along +x and +y (row 0 at y0), and what it counted;
/// the noise and the materials' details decoded once for them all. `tile_colour` is the far tiles' colour space.
pub fn bake(
    w: &World,
    t: &AnvilTerrain,
    rects: &[[f64; 4]],
    s: &Splat,
    tile_colour: ColorSpace,
) -> Result<Vec<(Texture, Baked)>, String> {
    let threads = Threads::of(w);
    let mut baker = Baker::new(w, t, s, tile_colour)?;
    let mut results = Vec::with_capacity(rects.len());
    for &[x0, y0, x1, y1] in rects {
        let width = ((x1 - x0) / s.pixel).ceil().max(1.0) as u32;
        let height = ((y1 - y0) / s.pixel).ceil().max(1.0) as u32;
        let at = |px: u32, py: u32| {
            (
                x0 + (f64::from(px) + 0.5) * s.pixel,
                y0 + (f64::from(py) + 0.5) * s.pixel,
            )
        };
        // the materials the rectangle's pixels meet, row by row, their details decoded before any pixel is shaded
        let rows: Vec<u32> = (0..height).collect();
        let met = parallel::map(threads, &rows, |_, &py| {
            let mut met = BTreeSet::new();
            for px in 0..width {
                let (x, y) = at(px, py);
                if let Some(c) = baker.lookup.corners(x, y)? {
                    met.extend(c.materials.iter().flatten().copied());
                }
            }
            Ok::<_, String>(met)
        })
        .map_err(|e| e.to_string())?
        .into_iter()
        .collect::<Result<Vec<_>, String>>()?;
        baker.prepare(met.into_iter().flatten().collect::<BTreeSet<u8>>())?;
        // each row shaded on its own (at most the stated threads rows at once), its counts added in row order
        let mut out = vec![0u8; width as usize * height as usize * 4];
        // each row's counts, and the error that stopped it (the first failing row's is the bake's)
        let mut jobs: Vec<(Baked, Option<String>, &mut [u8])> = out
            .chunks_mut(width as usize * 4)
            .map(|row| (Baked::default(), None, row))
            .collect();
        let baker = &baker;
        parallel::rows_mut(threads, &mut jobs, 1, |py, job| {
            let (baked, failed, row) = &mut job[0];
            let shaded = |x: f64, y: f64| -> Result<Option<Texel>, String> {
                match baker.lookup.corners(x, y)? {
                    None => Ok(None),
                    Some(corners) => baker
                        .shade(corners, x, y, 0.0, DetailPlane::Xy, s.pixel)
                        .map(Some),
                }
            };
            for px in 0..width {
                baked.pixels += 1;
                let (x, y) = at(px, py as u32);
                let texel = match shaded(x, y) {
                    Ok(Some(t)) => t,
                    Ok(None) => {
                        baked.outside += 1;
                        continue;
                    }
                    Err(e) => {
                        *failed = Some(e);
                        return;
                    }
                };
                if texel.tile_fallback {
                    baked.tile_fallback += 1;
                }
                let i = px as usize * 4;
                row[i..i + 3].copy_from_slice(&texel.rgb);
                row[i + 3] = 255;
            }
        })
        .map_err(|e| e.to_string())?;
        if let Some(e) = jobs.iter().find_map(|(_, failed, _)| failed.clone()) {
            return Err(e);
        }
        let mut baked = Baked::default();
        for (b, _, _) in &jobs {
            baked.pixels += b.pixels;
            baked.outside += b.outside;
            baked.tile_fallback += b.tile_fallback;
        }
        drop(jobs);
        results.push((
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
        ));
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w_of(n: [f64; 3]) -> f64 {
        match DetailPlane::vertical(n) {
            DetailPlane::Vertical { w } => w,
            DetailPlane::Xy => panic!("vertical"),
        }
    }

    /// The vertical program's `saturate(5 (n.x^2 / (n.x^2 + n.y^2) - 0.4))`, worked by hand.
    #[test]
    fn the_planes_mix_by_the_normals_azimuth() {
        assert_eq!(w_of([1.0, 0.0, 0.5]), 1.0);
        assert_eq!(w_of([0.0, 1.0, 0.5]), 0.0);
        // 45 degrees: x^2 / (x^2 + y^2) = 1/2, so 5 (0.5 - 0.4) = 0.5
        assert!((w_of([1.0, 1.0, 0.3]) - 0.5).abs() < 1e-12);
        // 0.4 and below take (x, z) only, 0.6 and above (y, z) only: x^2 / y^2 = 2/3 gives 0.4, 3/2 gives 0.6
        assert!(w_of([(2.0f64 / 3.0).sqrt(), 1.0, 0.0]) < 1e-12);
        assert!(w_of([1.5f64.sqrt(), 1.0, 0.0]) > 1.0 - 1e-12);
        // the sign of a component doesn't matter, and a flat normal has no horizontal part: (x, z)
        assert_eq!(w_of([-1.0, 0.0, 0.5]), 1.0);
        assert_eq!(w_of([0.0, 0.0, 1.0]), 0.0);
    }
}
