//! Unity terrains as stored: `TerrainData` (the heightmap, holes, splat layers and their alphamaps, tree prototypes
//! and instances) and `TerrainLayer`.
//!
//! The heightmap is `resolution` x `resolution` samples, row `z` then column `x`, each 0..=32766 for 0..1 of the
//! terrain's height (`scale.y` metres; `TerrainData.GetHeight` is `h / 32766 * size.y`); samples are `scale.x` and
//! `scale.z` metres apart from the terrain's position, so the terrain is `(resolution - 1) * scale` wide. Holes are
//! one byte per cell, 0 a hole (none stored: no holes). Alphamaps hold four layer weights per texel (RGBA), texel
//! row 0 at z = 0 as Unity stores rows.

use crate::read::player::{ObjRef, Player};
use crate::read::value::{Array, Value};
use crate::{Result, invalid};

/// A tree placed on a terrain: position as 0..1 of the terrain's size, scales, rotation about y (radians), colour,
/// and its prototype's index.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeInstance {
    pub position: [f32; 3],
    pub width_scale: f32,
    pub height_scale: f32,
    pub rotation: f32,
    pub color: u32,
    pub prototype: usize,
}

/// A `TerrainData` as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainData {
    pub name: String,
    pub resolution: u32,
    pub scale: [f32; 3],
    pub heights: Vec<i16>,
    /// One byte per cell, 0 a hole; `None` when none are stored.
    pub holes: Option<Vec<u8>>,
    pub layers: Vec<Option<ObjRef>>,
    pub alphamaps: Vec<Option<ObjRef>>,
    pub alphamap_resolution: u32,
    /// Each tree prototype's prefab.
    pub tree_prototypes: Vec<Option<ObjRef>>,
    pub trees: Vec<TreeInstance>,
}

/// A `TerrainLayer` as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainLayerData {
    pub name: String,
    pub diffuse: Option<ObjRef>,
    pub normal: Option<ObjRef>,
    pub mask: Option<ObjRef>,
    pub tile_size: [f32; 2],
    pub tile_offset: [f32; 2],
    pub specular: [f32; 4],
    pub metallic: f32,
    pub smoothness: f32,
    pub normal_scale: f32,
    /// Min and max, RGBA.
    pub diffuse_remap: [[f32; 4]; 2],
    pub mask_remap: [[f32; 4]; 2],
}

fn get<'a>(v: &'a Value, k: &str) -> Result<&'a Value> {
    v.get(k).ok_or_else(|| invalid(format!("terrain: no {k}")))
}

fn float(v: &Value, k: &str) -> Result<f32> {
    get(v, k)?
        .as_f32()
        .ok_or_else(|| invalid(format!("terrain: {k} is not a float")))
}

fn int(v: &Value, k: &str) -> Result<i64> {
    get(v, k)?
        .as_i64()
        .ok_or_else(|| invalid(format!("terrain: {k} is not an integer")))
}

fn vec<const N: usize>(v: &Value, k: &str, names: [&str; N]) -> Result<[f32; N]> {
    let x = get(v, k)?;
    let mut out = [0.0; N];
    for (o, n) in out.iter_mut().zip(names) {
        *o = float(x, n)?;
    }
    Ok(out)
}

fn list<'a>(v: &'a Value, k: &str) -> Result<&'a [Value]> {
    match get(v, k)? {
        Value::Array(Array::Values(items)) => Ok(items),
        Value::Array(a) if a.is_empty() => Ok(&[]),
        _ => Err(invalid(format!("terrain: {k} is not a list"))),
    }
}

fn pptrs(player: &Player, file: &str, items: &[Value], field: Option<&str>) -> Result<Vec<Option<ObjRef>>> {
    items
        .iter()
        .map(|p| {
            let p = match field {
                Some(f) => get(p, f)?,
                None => p,
            };
            player.resolve(file, p)
        })
        .collect()
}

impl TerrainData {
    /// Read the TerrainData `r` of `player`.
    pub fn read(player: &Player, r: &ObjRef) -> Result<Self> {
        let t = player.decode(r)?;
        let hm = get(&t, "m_Heightmap")?;
        let resolution = int(hm, "m_Resolution")? as u32;
        let heights = match get(hm, "m_Heights")? {
            Value::Array(Array::I16(h)) => h.clone(),
            _ => return Err(invalid("terrain: m_Heights is not 16-bit")),
        };
        let n = resolution as usize;
        if heights.len() != n * n {
            return Err(invalid(format!(
                "terrain: {} heights for resolution {resolution}",
                heights.len()
            )));
        }
        let holes = match hm.get("m_Holes") {
            Some(Value::Array(Array::U8(h))) if !h.is_empty() => {
                if h.len() != (n - 1) * (n - 1) {
                    return Err(invalid(format!(
                        "terrain: {} holes for {} cells",
                        h.len(),
                        (n - 1) * (n - 1)
                    )));
                }
                Some(h.clone())
            }
            Some(Value::Array(Array::Bool(h))) if !h.is_empty() => {
                if h.len() != (n - 1) * (n - 1) {
                    return Err(invalid(format!(
                        "terrain: {} holes for {} cells",
                        h.len(),
                        (n - 1) * (n - 1)
                    )));
                }
                Some(h.iter().map(|&b| u8::from(b)).collect())
            }
            _ => None,
        };
        let splat = get(&t, "m_SplatDatabase")?;
        let detail = get(&t, "m_DetailDatabase")?;
        let trees = list(detail, "m_TreeInstances")?
            .iter()
            .map(|i| {
                Ok(TreeInstance {
                    position: vec(i, "position", ["x", "y", "z"])?,
                    width_scale: float(i, "widthScale")?,
                    height_scale: float(i, "heightScale")?,
                    rotation: float(i, "rotation")?,
                    color: int(get(i, "color")?, "rgba")? as u32,
                    prototype: int(i, "index")? as usize,
                })
            })
            .collect::<Result<_>>()?;
        Ok(TerrainData {
            name: t.get("m_Name").and_then(Value::as_str).unwrap_or("").to_owned(),
            resolution,
            scale: vec(hm, "m_Scale", ["x", "y", "z"])?,
            heights,
            holes,
            layers: pptrs(player, &r.file, list(splat, "m_TerrainLayers")?, None)?,
            alphamaps: pptrs(player, &r.file, list(splat, "m_AlphaTextures")?, None)?,
            alphamap_resolution: int(splat, "m_AlphamapResolution")? as u32,
            tree_prototypes: pptrs(player, &r.file, list(detail, "m_TreePrototypes")?, Some("prefab"))?,
            trees,
        })
    }

    /// The height of sample (x, z) in metres above the terrain's position.
    pub fn height(&self, x: usize, z: usize) -> f64 {
        f64::from(self.heights[z * self.resolution as usize + x]) / 32766.0 * f64::from(self.scale[1])
    }

    /// The terrain's size in metres: (resolution - 1) * scale along x and z, scale.y high.
    pub fn size(&self) -> [f64; 3] {
        let cells = f64::from(self.resolution - 1);
        [
            cells * f64::from(self.scale[0]),
            f64::from(self.scale[1]),
            cells * f64::from(self.scale[2]),
        ]
    }
}

impl TerrainLayerData {
    /// Read the TerrainLayer `r` of `player`.
    pub fn read(player: &Player, r: &ObjRef) -> Result<Self> {
        let l = player.decode(r)?;
        let tex = |k: &str| -> Result<Option<ObjRef>> {
            match l.get(k) {
                Some(p) => player.resolve(&r.file, p),
                None => Ok(None),
            }
        };
        let rgba = ["x", "y", "z", "w"];
        Ok(TerrainLayerData {
            name: l.get("m_Name").and_then(Value::as_str).unwrap_or("").to_owned(),
            diffuse: tex("m_DiffuseTexture")?,
            normal: tex("m_NormalMapTexture")?,
            mask: tex("m_MaskMapTexture")?,
            tile_size: vec(&l, "m_TileSize", ["x", "y"])?,
            tile_offset: vec(&l, "m_TileOffset", ["x", "y"])?,
            specular: vec(&l, "m_Specular", ["r", "g", "b", "a"])?,
            metallic: float(&l, "m_Metallic")?,
            smoothness: float(&l, "m_Smoothness")?,
            normal_scale: float(&l, "m_NormalScale")?,
            diffuse_remap: [
                vec(&l, "m_DiffuseRemapMin", rgba)?,
                vec(&l, "m_DiffuseRemapMax", rgba)?,
            ],
            mask_remap: [
                vec(&l, "m_MaskMapRemapMin", rgba)?,
                vec(&l, "m_MaskMapRemapMax", rgba)?,
            ],
        })
    }
}
