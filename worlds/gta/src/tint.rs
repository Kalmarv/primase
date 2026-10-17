//! `gta-tint`: GTA's tint palettes, as the game draws them.
//!
//! A `_tnt` shader's vertex shader colours every vertex from its palette texture (`TintPaletteTex`, point sampled):
//! the column at u = vertex colour blue / 255 (the second vertex colour's for three tree effects,
//! [`crate::shader_class::TintColumn`]), the row the entity's tint (a tint past the palette's last row takes the last,
//! as the shader's clamped sampling does), and the colour multiplies the surface; across a triangle it is interpolated
//! between its vertices. The pass writes that colour into each tinted primitive's COLOR_0 (linear RGB as 16-bit
//! normalised, which glTF multiplies into the base colour; the alpha kept), per tint row: a prototype whose placements
//! use more than one row gets a copy per further row ([`World::copy_components`], its `core.lods` forms copied with
//! it), its placements moved to their row's prototype, and every tinted prototype says its row in a `gta.tint`. The
//! size this costs (prototypes and vertices copied) is counted.
//!
//! Runs after `lower-gta-pbr` and before `gta-vertex-colours`: it reads the blue channel that pass drops, and that pass
//! keeps the colour written here (the `gta.tinted` resource lists the materials whose primitives it wrote).
use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{Component, Entity, Instance, JsonError, Lods, Mesh, Semantic, Stream, Texture, World};
use primase_texcodec::{Decoder, TexDecoder};
use serde::Serialize;

use crate::components::{GtaEntity, GtaShader};
use crate::lower::{classes, texture};
use crate::shader_class::TintColumn;

/// The tint row a tinted prototype is drawn with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GtaTint {
    pub row: u32,
}

impl Component for GtaTint {
    const NAME: &'static str = "gta.tint";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// The materials whose primitives `gta-tint` wrote the palette colour into (their COLOR_0 is a tint, not GTA's
/// ambient occlusion and palette index).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct GtaTinted(pub BTreeSet<Entity>);

impl primase_core::Resource for GtaTinted {
    const NAME: &'static str = "gta.tinted";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// A decoded palette: width, height (both at least 1), RGBA8 rows top first.
struct Palette {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Palette {
    /// The palette texture's first level, decoded; an error for a palette without texels.
    fn decode(entity: Entity, t: &Texture) -> Result<Palette, String> {
        let level = t
            .levels
            .first()
            .ok_or_else(|| format!("palette {entity:?}: no levels"))?;
        if t.width == 0 || t.height == 0 {
            return Err(format!("palette {entity:?}: {} x {} texels", t.width, t.height));
        }
        let rgba = Decoder::default()
            .decode_rgba8(t.format, t.width, t.height, level)
            .map_err(|e| format!("palette: {e}"))?;
        if rgba.len() as u64 != u64::from(t.width) * u64::from(t.height) * 4 {
            return Err(format!("palette {entity:?}: decoded to {} bytes", rgba.len()));
        }
        Ok(Palette {
            width: t.width,
            height: t.height,
            rgba,
        })
    }

    fn colour(&self, row: u32, column: u32) -> [u8; 3] {
        let (row, column) = (row.min(self.height - 1) as usize, column as usize);
        let o = (row * self.width as usize + column) * 4;
        [self.rgba[o], self.rgba[o + 1], self.rgba[o + 2]]
    }

    /// The column a vertex colour's blue byte selects: the shader samples at u = blue / 255.
    fn column(&self, blue: u8) -> u32 {
        (u64::from(blue) * u64::from(self.width) / 255).min(u64::from(self.width - 1)) as u32
    }
}

/// `gta-tint` (see the module documentation).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GtaTintVariants;

/// sRGB byte to linear, as 16-bit normalised.
fn linear16(c: u8) -> u16 {
    (primase_image::srgb_to_linear(f64::from(c) / 255.0) * 65535.0).round() as u16
}

/// `mesh`'s tinted primitives coloured for `row`.
fn tint_mesh(
    mesh: &mut Mesh,
    row: u32,
    palettes: &BTreeMap<Entity, (Palette, TintColumn)>,
    log: &mut PassLog,
) {
    for p in mesh.primitives_mut() {
        let Some((palette, column)) = p.material.and_then(|m| palettes.get(&m)) else {
            continue;
        };
        let semantic = match column {
            TintColumn::Color0 => Semantic::Color(0),
            TintColumn::Color1 => Semantic::Color(1),
        };
        let Some(Stream::U8x4(index)) = p.streams.get(&semantic) else {
            log.count(
                "tinted primitives without their palette column's vertex colour: untinted",
                1,
            );
            continue;
        };
        let alpha: Vec<u16> = match p.streams.get(&Semantic::Color(0)) {
            Some(Stream::U8x4(c)) if c.len() == index.len() => {
                // COLOR_0's red and green are GTA's per-vertex ambient occlusion: replaced by the tint here
                if c.iter().any(|c| c[0] != 255 || c[1] != 255) {
                    log.approximate("primitives with per-vertex ambient occlusion not carried", 1);
                }
                c.iter().map(|c| u16::from(c[3]) * 257).collect()
            }
            _ => vec![65535; index.len()],
        };
        let colours: Vec<[u16; 4]> = index
            .iter()
            .zip(&alpha)
            .map(|(c, &a)| {
                let [r, g, b] = palette.colour(row, palette.column(c[2]));
                [linear16(r), linear16(g), linear16(b), a]
            })
            .collect();
        p.streams.insert(Semantic::Color(0), Stream::U16x4(colours));
        log.count("tinted primitives", 1);
    }
}

fn vertices(m: Option<&Mesh>) -> u64 {
    m.map_or(0, |m| {
        m.primitives().iter().map(|p| p.vertex_count() as u64).sum()
    })
}

impl Pass for GtaTintVariants {
    fn name(&self) -> &'static str {
        "gta-tint"
    }
    // no `requires`: it runs on every loaded part and leaves one without its input (a part holding only a water
    // surface, a cell whose shaders all draw nothing) as it is
    fn provides(&self) -> &[&'static str] {
        &["gta.tint", "gta.tinted"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh", "core.instance"]
    }
    fn approximates(&self) -> &'static str {
        "per-vertex ambient occlusion (vertex colour 0 red and green) replaced by the tint"
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
            palettes.insert(m, (Palette::decode(p, t)?, column));
        }
        if palettes.is_empty() {
            return Ok(());
        }
        // per prototype, its placements by tint row
        let mut rows: BTreeMap<Entity, BTreeMap<u32, Vec<Entity>>> = BTreeMap::new();
        for (e, i) in w.iter::<Instance>() {
            let tint = w.get::<GtaEntity>(e).map_or(0, |g| g.tint_value);
            rows.entry(i.prototype)
                .or_default()
                .entry(tint)
                .or_default()
                .push(e);
        }
        let tinted_mesh = |w: &World, e: Entity| {
            w.get::<Mesh>(e).is_some_and(|m| {
                m.primitives()
                    .iter()
                    .any(|p| p.material.is_some_and(|m| palettes.contains_key(&m)))
            })
        };
        let (mut copies, mut copied_vertices) = (0u64, 0u64);
        for (proto, by_row) in rows {
            let forms = |w: &World, e: Entity| -> Vec<Entity> {
                w.get::<Lods>(e)
                    .map_or(Vec::new(), |l| l.forms.iter().map(|f| f.form).collect())
            };
            if !tinted_mesh(w, proto) && !forms(w, proto).iter().any(|&f| tinted_mesh(w, f)) {
                continue;
            }
            // the first row keeps the prototype; every further row gets a copy, its forms copied with it, all made
            // before any is tinted (tinting replaces the palette column the copies would need)
            let mut targets = Vec::with_capacity(by_row.len());
            for (k, (&row, placements)) in by_row.iter().enumerate() {
                let target = if k == 0 {
                    proto
                } else {
                    let copy = crate::components::copy_prototype(w, proto);
                    copies += 1;
                    for h in std::iter::once(copy).chain(forms(w, copy)) {
                        copied_vertices += vertices(w.get::<Mesh>(h));
                    }
                    for &e in placements {
                        w.get_mut::<Instance>(e).expect("instance").prototype = copy;
                    }
                    copy
                };
                targets.push((row, target));
            }
            for (row, target) in targets {
                for h in std::iter::once(target).chain(forms(w, target)) {
                    if let Some(mut mesh) = w.remove::<Mesh>(h) {
                        tint_mesh(&mut mesh, row, &palettes, log);
                        w.insert(h, mesh);
                    }
                }
                w.insert(target, GtaTint { row });
                log.count("tinted prototypes (one per tint row)", 1);
            }
        }
        log.count("prototypes copied for a further tint row", copies);
        log.count("vertices copied for a further tint row", copied_vertices);
        let mut tinted = w.remove_resource::<GtaTinted>().unwrap_or_default();
        tinted.0.extend(palettes.keys().copied());
        w.insert_resource(tinted);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::texformat::TexFormat;
    use primase_core::{ColorSpace, TexRole};

    fn palette(width: u32, height: u32, levels: Vec<Vec<u8>>) -> Texture {
        Texture {
            width,
            height,
            format: TexFormat::Rgba8Unorm,
            levels,
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        }
    }

    #[test]
    fn palettes_without_texels_are_errors() {
        let e = Entity::from_index(1);
        assert!(Palette::decode(e, &palette(2, 0, vec![Vec::new()])).is_err());
        assert!(Palette::decode(e, &palette(0, 2, vec![Vec::new()])).is_err());
        assert!(Palette::decode(e, &palette(2, 2, Vec::new())).is_err());
        let p = Palette::decode(e, &palette(2, 1, vec![vec![1, 2, 3, 4, 5, 6, 7, 8]])).expect("palette");
        // a row past the last takes the last; blue 255 is the last column
        assert_eq!(p.colour(9, p.column(255)), [5, 6, 7]);
        assert_eq!(p.colour(0, p.column(0)), [1, 2, 3]);
    }

    fn placement(w: &mut World, prototype: Entity, tint: u32) {
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype,
                variant: 0,
            },
        );
        w.insert(
            e,
            GtaEntity {
                ymap: "m".into(),
                index: 0,
                interior: None,
                lod_parent: None,
                archetype: 0,
                flags: 0,
                guid: 0,
                parent_index: -1,
                lod_dist: 100.0,
                child_lod_dist: 0.0,
                lod_level: 0,
                num_children: 0,
                priority_level: 0,
                ambient_occlusion_multiplier: 255,
                artificial_ambient_occlusion: 255,
                tint_value: tint,
            },
        );
    }

    /// Every tint row's prototype gets its own row's colour: the copies for further rows are made from the untinted
    /// prototype (tinting replaces the palette column they need).
    #[test]
    fn every_tint_row_gets_its_own_colour() {
        let mut w = World::new();
        // a 1 x 3 palette (one column, rows 0-2): red, green, blue
        let pal = w.spawn();
        w.insert(
            pal,
            palette(1, 3, vec![vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255]]),
        );
        let m = w.spawn();
        w.insert(m, PbrMaterial::default());
        w.insert(
            m,
            GtaShader {
                drawable: "d".into(),
                index: 0,
                name: primase_hash::joaat(b"default_tnt"),
                preset: 0,
                render_bucket: 0,
                bucket_mask: 0,
                params: vec![(
                    primase_hash::joaat(b"TintPaletteTex"),
                    crate::components::GtaParam::Texture {
                        name: None,
                        texture: Some(pal),
                    },
                )],
            },
        );
        let proto = w.spawn();
        let mut streams = BTreeMap::new();
        streams.insert(
            Semantic::Position,
            Stream::F32x3(vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
        );
        streams.insert(Semantic::Color(0), Stream::U8x4(vec![[255, 255, 0, 255]; 3]));
        w.insert(
            proto,
            Mesh::new(vec![primase_core::Primitive {
                topology: primase_core::Topology::Triangles,
                streams,
                indices: Some(vec![0, 1, 2]),
                material: Some(m),
            }]),
        );
        w.insert(
            proto,
            crate::components::GtaSources {
                lod: 0,
                sources: vec![[0, 0]],
            },
        );
        for row in [0, 1, 2] {
            placement(&mut w, proto, row);
        }
        GtaTintVariants
            .run(&mut w, &mut PassLog::default())
            .expect("gta-tint");
        let colour = |w: &World, row: u32| {
            let p = w
                .iter::<GtaTint>()
                .find(|(_, t)| t.row == row)
                .map(|(e, _)| e)
                .expect("a prototype per row");
            match &w.get::<Mesh>(p).expect("mesh").primitives()[0].streams[&Semantic::Color(0)] {
                Stream::U16x4(c) => c[0],
                s => panic!("not the tint: {s:?}"),
            }
        };
        assert_eq!(colour(&w, 0), [65535, 0, 0, 65535]);
        assert_eq!(colour(&w, 1), [0, 65535, 0, 65535]);
        assert_eq!(colour(&w, 2), [0, 0, 65535, 65535]);
    }
}
