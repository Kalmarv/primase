//! `gta-tint`: GTA's tint palettes as material variants.
//!
//! A `_tnt` shader's vertex shader colours every vertex from its palette texture (`TintPaletteTex`, point sampled):
//! the column at u = vertex colour blue / 255 (the second vertex colour's for three tree effects,
//! [`crate::shader_class::TintColumn`]), the row the entity's tint. A tinted primitive becomes one primitive per
//! palette colour it uses, each with a material per tint row its placements use: its `pbr.material` with the base
//! colour factor multiplied by the palette colour (sRGB bytes, multiplied in linear light), in `core.material_variants`
//! under the row; the primitive's own material is row 0's. Columns that give the same colour in every row share one
//! primitive. A triangle goes with its first vertex's column. Placements of a prototype with variants get their tint as
//! their variant (a tint past the palette's last row takes the last, as the shader's clamped sampling does).
//!
//! Runs after `lower-gta-pbr` (it tints that pass's materials) and before `gta-vertex-colours` (it reads the blue
//! channel that pass drops). Variant materials carry the shader's `gta.shader` and a `gta.tint`. The coarser LOD meshes
//! (`gta.drawable`, until core has alternative LOD meshes) are not tinted.
use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{
    Component, Entity, Instance, JsonError, MaterialVariants, Mesh, Primitive, Semantic, Stream, Texture,
    World,
};
use primase_texcodec::{Decoder, TexDecoder};
use serde::Serialize;

use crate::components::{GtaDrawable, GtaEntity, GtaShader};
use crate::lower::{classes, texture};
use crate::shader_class::TintColumn;

/// A variant material's tint: the palette texture, row and column, and the colour (sRGB bytes).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GtaTint {
    pub palette: Entity,
    pub row: u32,
    pub column: u32,
    pub colour: [u8; 3],
}

impl Component for GtaTint {
    const NAME: &'static str = "gta.tint";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.palette);
    }
}

/// A decoded palette: width, height, RGBA8 rows top first.
struct Palette {
    entity: Entity,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Palette {
    fn colour(&self, row: u32, column: u32) -> [u8; 3] {
        let o = ((row.min(self.height - 1) * self.width + column) * 4) as usize;
        [self.rgba[o], self.rgba[o + 1], self.rgba[o + 2]]
    }

    /// The column a vertex colour's blue byte selects: the shader samples at u = blue / 255.
    fn column(&self, blue: u8) -> u32 {
        (u32::from(blue) * self.width / 255).min(self.width - 1)
    }
}

/// `gta-tint` (see the module documentation).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GtaTintVariants;

impl Pass for GtaTintVariants {
    fn name(&self) -> &'static str {
        "gta-tint"
    }
    fn requires(&self) -> &[&'static str] {
        &["gta.shader", "gta.drawable", "pbr.material"]
    }
    fn provides(&self) -> &[&'static str] {
        &["core.material_variants", "gta.tint", "pbr.material", "gta.shader"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh", "gta.drawable", "core.instance"]
    }
    fn approximates(&self) -> &'static str {
        "a triangle whose vertices pick different palette columns is coloured by its first vertex's; coarser LODs not \
         tinted"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let classes = classes(w)?;
        // the tinted materials with a palette that decodes
        let mut palettes: BTreeMap<Entity, (Palette, TintColumn)> = BTreeMap::new();
        for (&m, c) in &classes {
            let Some(column) = c.tint else { continue };
            if !w.has::<PbrMaterial>(m) {
                continue;
            }
            let s = w.get::<GtaShader>(m).expect("shader");
            let Some(Some(p)) = texture(s, "TintPaletteTex") else {
                log.count("tinted materials without a palette: untinted", 1);
                continue;
            };
            let t = w.get::<Texture>(p).ok_or("palette is not a loaded texture")?;
            let rgba = Decoder::default()
                .decode_rgba8(t.format, t.width, t.height, &t.levels[0])
                .map_err(|e| format!("palette: {e}"))?;
            palettes.insert(
                m,
                (
                    Palette {
                        entity: p,
                        width: t.width,
                        height: t.height,
                        rgba,
                    },
                    column,
                ),
            );
        }
        // the tint rows each prototype is placed with
        let mut rows: BTreeMap<Entity, BTreeSet<u32>> = BTreeMap::new();
        for (e, i) in w.iter::<Instance>() {
            let tint = w.get::<GtaEntity>(e).map_or(0, |g| g.tint_value);
            rows.entry(i.prototype).or_default().insert(tint);
        }
        let mut made: BTreeMap<(Entity, u32, u32), Entity> = BTreeMap::new();
        let mut tinted_prototypes = BTreeSet::new();
        let prototypes: Vec<Entity> = w.iter::<GtaDrawable>().map(|(e, _)| e).collect();
        for e in prototypes {
            let Some(mesh) = w.get::<Mesh>(e) else { continue };
            if !mesh
                .primitives
                .iter()
                .any(|p| p.material.is_some_and(|m| palettes.contains_key(&m)))
            {
                continue;
            }
            let mut mesh = w.remove::<Mesh>(e).expect("mesh");
            let mut d = w.remove::<GtaDrawable>(e).expect("drawable");
            let mut variants = w.remove::<MaterialVariants>(e).unwrap_or_default().0;
            variants.resize(mesh.primitives.len(), BTreeMap::new());
            let used: BTreeSet<u32> = rows.get(&e).into_iter().flatten().copied().chain([0]).collect();
            let (mut prims, mut sources, mut vars) = (Vec::new(), Vec::new(), Vec::new());
            for ((p, s), v) in mesh
                .primitives
                .drain(..)
                .zip(d.sources[0].drain(..))
                .zip(variants)
            {
                let Some((palette, column)) = p.material.and_then(|m| palettes.get(&m)) else {
                    prims.push(p);
                    sources.push(s);
                    vars.push(v);
                    continue;
                };
                let base = p.material.expect("material");
                let semantic = match column {
                    TintColumn::Color0 => Semantic::Color(0),
                    TintColumn::Color1 => Semantic::Color(1),
                };
                let Some(Stream::U8x4(colours)) = p.streams.get(&semantic) else {
                    log.count(
                        "tinted primitives without their palette column's vertex colour: untinted",
                        1,
                    );
                    prims.push(p);
                    sources.push(s);
                    vars.push(v);
                    continue;
                };
                // columns with the same colour in every row are one
                let signature =
                    |c: u32| -> Vec<[u8; 3]> { (0..palette.height).map(|r| palette.colour(r, c)).collect() };
                let mut first_of: BTreeMap<Vec<[u8; 3]>, u32> = BTreeMap::new();
                let mut key = |c: u32| *first_of.entry(signature(c)).or_insert(c);
                let column_of: Vec<u32> = colours.iter().map(|c| palette.column(c[2])).collect();
                let idx: Vec<u32> = p
                    .indices
                    .clone()
                    .unwrap_or_else(|| (0..p.vertex_count() as u32).collect());
                let mut groups: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
                for t in idx.as_chunks::<3>().0 {
                    let cols = t.map(|i| column_of[i as usize]);
                    if cols[1] != cols[0] || cols[2] != cols[0] {
                        log.approximate("triangles whose vertices pick different palette columns", 1);
                    }
                    groups.entry(key(cols[0])).or_default().extend_from_slice(t);
                }
                log.count("tinted primitives", 1);
                log.count("primitives made, one per palette colour", groups.len() as u64);
                for (c, tris) in groups {
                    let mut q = subset(&p, &tris);
                    let mut map = BTreeMap::new();
                    for &row in &used {
                        let colour = palette.colour(row, c);
                        let row_key = row.min(palette.height - 1);
                        let m = *made.entry((base, row_key, c)).or_insert_with(|| {
                            let mut pbr = w.get::<PbrMaterial>(base).expect("pbr").clone();
                            for (f, b) in pbr.base_color.iter_mut().zip(colour) {
                                *f *= primase_image::srgb_to_linear(f64::from(b) / 255.0) as f32;
                            }
                            let shader = w.get::<GtaShader>(base).expect("shader").clone();
                            let m = w.spawn();
                            w.insert(m, pbr);
                            w.insert(m, shader);
                            w.insert(
                                m,
                                GtaTint {
                                    palette: palette.entity,
                                    row: row_key,
                                    column: c,
                                    colour,
                                },
                            );
                            m
                        });
                        map.insert(row, m);
                    }
                    q.material = map.get(&0).copied();
                    prims.push(q);
                    sources.push(s);
                    vars.push(map);
                }
            }
            mesh.primitives = prims;
            d.sources[0] = sources;
            w.insert(e, mesh);
            w.insert(e, d);
            w.insert(e, MaterialVariants(vars));
            tinted_prototypes.insert(e);
        }
        log.count("variant materials", made.len() as u64);
        let placements: Vec<(Entity, u32)> = w
            .iter::<Instance>()
            .filter(|(_, i)| tinted_prototypes.contains(&i.prototype))
            .map(|(e, _)| (e, w.get::<GtaEntity>(e).map_or(0, |g| g.tint_value)))
            .collect();
        for (e, tint) in placements {
            w.get_mut::<Instance>(e).expect("instance").variant = tint;
            log.count("placements given their tint as variant", 1);
        }
        Ok(())
    }
}

/// The primitive drawn by the triangles `tris` (indices into `p`'s vertices), over just the vertices they use, in
/// first-use order.
fn subset(p: &Primitive, tris: &[u32]) -> Primitive {
    let mut new_index: BTreeMap<u32, u32> = BTreeMap::new();
    let mut order = Vec::new();
    let indices = tris
        .iter()
        .map(|&i| {
            *new_index.entry(i).or_insert_with(|| {
                order.push(i as usize);
                order.len() as u32 - 1
            })
        })
        .collect();
    let streams = p
        .streams
        .iter()
        .map(|(sem, s)| {
            let s = match s {
                Stream::F32x2(v) => Stream::F32x2(order.iter().map(|&i| v[i]).collect()),
                Stream::F32x3(v) => Stream::F32x3(order.iter().map(|&i| v[i]).collect()),
                Stream::F32x4(v) => Stream::F32x4(order.iter().map(|&i| v[i]).collect()),
                Stream::U8x4(v) => Stream::U8x4(order.iter().map(|&i| v[i]).collect()),
                Stream::U16x4(v) => Stream::U16x4(order.iter().map(|&i| v[i]).collect()),
            };
            (*sem, s)
        })
        .collect();
    Primitive {
        topology: p.topology,
        streams,
        indices: Some(indices),
        material: p.material,
    }
}
