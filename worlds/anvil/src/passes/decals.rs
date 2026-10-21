//! `lower-anvil-decals{rect=x0,y0,x1,y1,step=N,range=METRES,offset=METRES,sample=corner|centre,
//! family=write-gbuffer-roughness,cutoff=X,lift=UNITS}`:
//! the deferred decals ([`AnvilDeferredDecals`]) of the box family (`ddecal.hlsl` PS 381 with VS 382, material-spec
//! §23) become meshes draped on the ground `lower-anvil-terrain` draws with the same `rect`, `step`, `range`, `offset`
//! and `sample` (run before it, while the terrain is read). Each value is the engine's, read, or a stated choice:
//!
//! - **Which** (stated): the decals whose box's centre lies in `rect` (x0 <= x < x1, y0 <= y < y1), of a DDecalModel
//!   of the box family by `family`'s rule; the terrain family (PS 353: the engine writes those into the terrain's
//!   bake, not the G-buffer) and models without a Diffuse, Normal or Mask1 left out, counted. Which permutation a
//!   model draws with isn't read (the engine's selector wasn't found): `family=write-gbuffer-roughness` states the
//!   rule [`family`] (material-spec §23.8, §23.9: body byte 49 is the model's `WriteGBufferRoughness`, by the class
//!   registry; 1 on the 17 models whose names and textures are road lines, cracks, tire traces and guidance, 0 on the
//!   53 trails, borders, ground and footprints; byte 48, unnamed, equal to it on all 70).
//! - **Geometry** (the shader's box test, executed in §23.2; the surface a choice): the engine draws a decal on every
//!   surface inside its box: `|(P - c) . X| / |X|^2 <= 1`, the same along `Y`, and its depth. This pass takes the
//!   ground's surface alone: each ground triangle (the grid's own, [`GroundGrid`]) cut to the box, its pieces kept
//!   in the triangle's plane, so the decal lies on the ground mesh exactly, then lifted by `lift` along the source's
//!   up (source units). The box's depth is its stored bounds' (its minimum and maximum, which also bound x and y):
//!   for a box whose half-axes are level, its Z is vertical and the bounds' z its depth exactly; for one with a
//!   tilted half-axis (4,046 of the base forge's 1,355,338) its Z isn't stored, its bounds stand in, counted.
//! - **Texture coordinates** (the engine's, read: §23.2, §23.8): `lx = (P - c) . X / |X|^2`,
//!   `ly = (P - c) . Y / |Y|^2` at each vertex (before the lift), `U = (lo - hi lx) / 65536`, `V = 0.5 + ly / 2`,
//!   with `lo = trunc((offset + width / 2) 65535)` and `hi = trunc(width / 2 x 65535)` from the record's window.
//! - **Material** per DDecalModel (PS 381, executed): base colour its Diffuse (sRGB), metalness 0 (layout A's slot,
//!   written 0), roughness its Normal's alpha, normal map its Normal's red and green (`n = -nx X^ - ny Y^ + nz n_s`:
//!   red along `-X^`, the tangent this pass gives each vertex, green along `-Y^`, the bitangent's side its sign
//!   chooses; z rebuilt). Alpha: the engine keeps a pixel where `Mask1.B w13 scale >= ref`, `w13 = (1 - fade) m`
//!   (`m` the model's opacity). Each vertex's `COLOR_0` alpha is `(1 - fade) m / k`, the base colour's alpha
//!   `Mask1.B`, and the material's cutoff `cutoff / k`, `k` the greatest `(1 - fade) m` the model's drawn decals
//!   have: the same test exactly, `cutoff` standing for `ref / scale` (0.4 by §23.5's fit; neither constant's
//!   binding is read). A decal whose `(1 - fade) m` is under `cutoff` passes the test at no texel (Mask1.B at most 1):
//!   left out, counted; so `k` is at least `cutoff` and the material's cutoff at most 1.
//!
//! Left out, counted: the decals' angle test (its threshold `cb3[4].z` not read), the terrain-mask colour term
//! (`k = t18.z`, `cb0[49].x`, not read: taken 0), the Height maps (read only by parallax permutations, which models
//! select those not read), decals on props (the ground alone is draped).

use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef};
use primase_core::{
    ColorSpace, Component, Entity, Instance, Mesh, Name, NormalBasis, NormalConvention, NormalGreen, NormalX,
    NormalZ, Primitive, Semantic, Stream, TexFormat, TexRole, Texture, Topology, World,
};

use super::{CellPoint, GroundGrid, ground_options, local_origin, placed_at};
use crate::components::{AnvilDecalModel, AnvilDeferredDecals, AnvilTerrain, AnvilTextureMap, DeferredDecal};

/// See the module documentation.
#[derive(Clone, Debug, PartialEq)]
pub struct LowerAnvilDecals {
    pub rect: [f64; 4],
    pub step: u32,
    pub range: f64,
    pub offset: f64,
    pub sample: CellPoint,
    /// Which models are of the box family.
    pub family: FamilyRule,
    /// The alpha test's `ref / scale`.
    pub cutoff: f64,
    /// How far the decals lie above the ground, along the source's up (source units).
    pub lift: f64,
}

/// Which `ddecal.hlsl` family a DDecalModel draws with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// The box family (PS 381 and its permutations): projected onto the G-buffer.
    Box,
    /// The terrain family (PS 353 and its permutations): written into the terrain's bake.
    Terrain,
}

/// The stated rule naming a model's family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FamilyRule {
    /// [`family`]: its `WriteGBufferRoughness`.
    WriteGBufferRoughness,
}

/// A DDecalModel's family by `family=write-gbuffer-roughness`: its `WriteGBufferRoughness` (body byte 49, its flag 7)
/// 1, the box family; 0, the terrain family; `None` otherwise.
pub fn family(m: &AnvilDecalModel) -> Option<Family> {
    match m.flags[7] {
        1 => Some(Family::Box),
        0 => Some(Family::Terrain),
        _ => None,
    }
}

impl LowerAnvilDecals {
    /// From its options, all required: the ground's (`rect`, `step`, `range`, `offset`, `sample`: those of the
    /// `lower-anvil-terrain` it lies on), `family`, `cutoff` and `lift`.
    pub fn from_options(o: &Options) -> Result<LowerAnvilDecals, String> {
        let usage = "rect=x0,y0,x1,y1, step=N, range=METRES, offset=METRES, sample=corner|centre (the ground's, as \
                     lower-anvil-terrain's), family=write-gbuffer-roughness, cutoff=X (ref / scale), lift=UNITS";
        let g = ground_options(o, &["range", "family", "cutoff", "lift"], usage)?;
        let number = |k: &str| -> Result<f64, String> {
            let v: f64 = o
                .get(k)
                .ok_or_else(|| format!("{k} is required: {usage}"))?
                .parse()
                .map_err(|_| format!("{k} is a number"))?;
            if v.is_finite() && v >= 0.0 {
                Ok(v)
            } else {
                Err(format!("{k} {v}: finite and at least 0"))
            }
        };
        let family = match o.get("family").map(String::as_str) {
            Some("write-gbuffer-roughness") => FamilyRule::WriteGBufferRoughness,
            Some(v) => return Err(format!("family={v} is not write-gbuffer-roughness")),
            None => return Err(format!("family is required: {usage}")),
        };
        let range = number("range")?;
        if range <= 0.0 {
            return Err(format!("range {range}: above 0"));
        }
        Ok(LowerAnvilDecals {
            rect: g.rect,
            step: g.step,
            range,
            offset: g.offset,
            sample: g.sample,
            family,
            cutoff: number("cutoff")?,
            lift: number("lift")?,
        })
    }
}

/// A point in the source frame, f64.
type P = [f64; 3];

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: P, b: P) -> P {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn unit(a: P) -> P {
    let l = dot(a, a).sqrt();
    a.map(|x| x / l)
}

/// `polygon` cut to where `f` is at least 0 (`f` linear), in order.
fn clip(polygon: &[P], f: impl Fn(P) -> f64) -> Vec<P> {
    let mut out = Vec::with_capacity(polygon.len() + 2);
    for (k, &a) in polygon.iter().enumerate() {
        let b = polygon[(k + 1) % polygon.len()];
        let (fa, fb) = (f(a), f(b));
        if fa >= 0.0 {
            out.push(a);
        }
        if (fa >= 0.0) != (fb >= 0.0) {
            let s = fa / (fa - fb);
            out.push(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * s));
        }
    }
    out
}

/// One decal's draped pieces: its triangles' vertices (source frame, before the lift), each triangle's ground normal
/// (source frame, unit, up), and its alpha factor `(1 - fade) m`.
struct Draped {
    triangles: Vec<([P; 3], P)>,
    alpha: f64,
}

/// The engine's box test as planes on `d`'s box: its half-axes' slabs, then its bounds.
fn box_planes(d: &DeferredDecal) -> Vec<Box<dyn Fn(P) -> f64>> {
    let (c, x, y) = (
        d.centre.map(f64::from),
        d.axis_x.map(f64::from),
        d.axis_y.map(f64::from),
    );
    let (xx, yy) = (dot(x, x), dot(y, y));
    let [lo, hi] = d.bounds.map(|b| b.map(f64::from));
    let mut planes: Vec<Box<dyn Fn(P) -> f64>> = vec![
        Box::new(move |p| 1.0 - dot(sub(p, c), x) / xx),
        Box::new(move |p| 1.0 + dot(sub(p, c), x) / xx),
        Box::new(move |p| 1.0 - dot(sub(p, c), y) / yy),
        Box::new(move |p| 1.0 + dot(sub(p, c), y) / yy),
    ];
    for a in 0..3 {
        planes.push(Box::new(move |p| p[a] - lo[a]));
        planes.push(Box::new(move |p| hi[a] - p[a]));
    }
    planes
}

/// `d` cut to the ground triangles of `g` under its bounds.
fn drape(d: &DeferredDecal, g: &GroundGrid<'_>) -> Result<Vec<([P; 3], P)>, String> {
    let [lo, hi] = d.bounds.map(|b| b.map(f64::from));
    // the grid's squares whose extent meets the bounds' x and y
    let pitch = g.spacing * f64::from(g.columns.get(1).copied().unwrap_or(0) - g.columns[0]);
    let span = |first: f64, n: usize, from: f64, to: f64| -> Option<(usize, usize)> {
        let a = ((from - first) / pitch).floor().max(0.0);
        let b = ((to - first) / pitch).ceil().min(n as f64 - 1.0);
        // in range: clamped to 0..n - 1 above
        (a < b).then_some((a as usize, b as usize))
    };
    let (Some((c0, c1)), Some((r0, r1))) = (
        span(g.x_of(g.columns[0]), g.columns.len(), lo[0], hi[0]),
        span(g.y_of(g.rows[0]), g.rows.len(), lo[1], hi[1]),
    ) else {
        return Ok(Vec::new());
    };
    let planes = box_planes(d);
    let mut out = Vec::new();
    for r in r0..r1 {
        for c in c0..c1 {
            let corner = |rr: usize, cc: usize| -> Result<P, String> {
                Ok(g.point(g.columns[cc], g.rows[rr])?.map(f64::from))
            };
            let [a, b, dd, f] = [
                corner(r, c)?,
                corner(r, c + 1)?,
                corner(r + 1, c)?,
                corner(r + 1, c + 1)?,
            ];
            // the ground's two triangles of the square, counter-clockwise seen from up
            for tri in [[a, b, f], [a, f, dd]] {
                let normal = unit(cross(sub(tri[1], tri[0]), sub(tri[2], tri[0])));
                let mut polygon = tri.to_vec();
                for plane in &planes {
                    polygon = clip(&polygon, plane);
                    if polygon.len() < 3 {
                        break;
                    }
                }
                polygon.dedup();
                if polygon.len() > 1 && polygon[0] == polygon[polygon.len() - 1] {
                    polygon.pop();
                }
                for k in 1..polygon.len().saturating_sub(1) {
                    out.push(([polygon[0], polygon[k], polygon[k + 1]], normal));
                }
            }
        }
    }
    Ok(out)
}

/// A model's material and its alpha divisor `k`.
struct ModelMaterial {
    material: Entity,
    k: f64,
}

impl LowerAnvilDecals {
    /// The model's material (on its own entity), its textures composed: base colour the Diffuse with the Mask1's blue
    /// as alpha, roughness the Normal's alpha in green; its Normal marked as the decal's normal map. `Err` names why a
    /// model can't be drawn.
    fn material(
        &self,
        w: &mut World,
        e: Entity,
        m: &AnvilDecalModel,
        k: f64,
        log: &mut PassLog,
    ) -> Result<Entity, &'static str> {
        let [Some(diffuse), Some(normal), _, Some(mask)] = m.textures else {
            return Err("decal models without a Diffuse, Normal or Mask1 map read");
        };
        let decode = |w: &World, t: Entity, space: ColorSpace| -> Result<Texture, &'static str> {
            let tex = w
                .get::<Texture>(t)
                .ok_or("decal models whose maps hold no texture")?;
            let meta = primase_image::TextureMetadata {
                color_space: space,
                role: TexRole::Other,
                normal: None,
            };
            primase_image::map_rgba8_levels(tex, 0..1, meta, |x| Ok(x.rgba))
                .map_err(|_| "decal models whose maps don't decode")
        };
        // a map's view: sRGB where its TextureMap says (field 25 = 1), the sampler then reading it linear
        let srgb = |w: &World, t: Entity| w.get::<AnvilTextureMap>(t).is_some_and(|d| d.field_25 == 1);
        if !srgb(w, diffuse) || srgb(w, normal) {
            return Err("decal models whose Diffuse isn't sRGB-viewed or Normal is");
        }
        let mask_srgb = srgb(w, mask);
        let colour = decode(w, diffuse, ColorSpace::Srgb)?;
        let shape = decode(w, mask, ColorSpace::Linear)?;
        let rough = decode(w, normal, ColorSpace::Linear)?;
        if (colour.width, colour.height) != (shape.width, shape.height) {
            return Err("decal models whose Diffuse and Mask1 differ in size");
        }
        // the alpha the shader reads: Mask1's blue, made linear where it's viewed sRGB (to the nearest 8-bit value)
        let blue = |b: u8| {
            if mask_srgb {
                (primase_image::srgb_to_linear(f64::from(b) / 255.0) * 255.0).round() as u8
            } else {
                b
            }
        };
        if mask_srgb {
            log.approximate(
                "decal models whose Mask1 is viewed sRGB: its blue made linear, to the nearest 8-bit value",
                1,
            );
        }
        let base: Vec<u8> = colour.levels[0]
            .as_chunks::<4>()
            .0
            .iter()
            .zip(shape.levels[0].as_chunks::<4>().0)
            .flat_map(|(c, s)| [c[0], c[1], c[2], blue(s[2])])
            .collect();
        let roughness: Vec<u8> = rough.levels[0]
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|n| [0, n[3], 0, 255])
            .collect();
        let base_texture = w.spawn();
        w.insert(
            base_texture,
            Texture {
                width: colour.width,
                height: colour.height,
                format: TexFormat::Rgba8Unorm,
                levels: vec![base],
                color_space: ColorSpace::Srgb,
                role: TexRole::Color,
                normal: None,
            },
        );
        let rough_texture = w.spawn();
        w.insert(
            rough_texture,
            Texture {
                width: rough.width,
                height: rough.height,
                format: TexFormat::Rgba8Unorm,
                levels: vec![roughness],
                color_space: ColorSpace::Linear,
                role: TexRole::Mask,
                normal: None,
            },
        );
        if let Some(tex) = w.get_mut::<Texture>(normal) {
            tex.role = TexRole::Normal;
            tex.color_space = ColorSpace::Linear;
            tex.normal = Some(NormalConvention {
                green: NormalGreen::Up,
                z: NormalZ::Rebuild,
                x: NormalX::Red,
                basis: NormalBasis::TangentBitangent,
            });
        }
        w.insert(
            e,
            PbrMaterial {
                base_color_texture: Some(TextureRef::new(base_texture)),
                metallic: 0.0,
                roughness: 1.0,
                metallic_roughness_texture: Some(TextureRef::new(rough_texture)),
                normal_texture: Some(TextureRef::new(normal)),
                alpha_mode: AlphaMode::Mask {
                    cutoff: (self.cutoff / k) as f32,
                },
                ..PbrMaterial::default()
            },
        );
        Ok(e)
    }
}

impl Pass for LowerAnvilDecals {
    fn name(&self) -> &'static str {
        "lower-anvil-decals"
    }
    fn options(&self) -> Options {
        let [x0, y0, x1, y1] = self.rect;
        Options::from([
            ("rect".to_owned(), format!("{x0},{y0},{x1},{y1}")),
            ("step".to_owned(), self.step.to_string()),
            ("range".to_owned(), self.range.to_string()),
            ("offset".to_owned(), self.offset.to_string()),
            (
                "sample".to_owned(),
                match self.sample {
                    CellPoint::Corner => "corner",
                    CellPoint::Centre => "centre",
                }
                .to_owned(),
            ),
            (
                "family".to_owned(),
                match self.family {
                    FamilyRule::WriteGBufferRoughness => "write-gbuffer-roughness",
                }
                .to_owned(),
            ),
            ("cutoff".to_owned(), self.cutoff.to_string()),
            ("lift".to_owned(), self.lift.to_string()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilDeferredDecals::NAME, AnvilTerrain::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            Mesh::NAME,
            PbrMaterial::NAME,
            Texture::NAME,
            Instance::NAME,
            primase_core::Transform::NAME,
            Name::NAME,
        ]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Texture::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilDeferredDecals::NAME, AnvilDecalModel::NAME]
    }
    fn approximates(&self) -> &'static str {
        "deferred decals draped on the ground alone, lifted by a stated height; their angle test and terrain-mask \
         colour term left out"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // the terrains, set aside while the world changes (lower-anvil-terrain lowers them after)
        let ids: Vec<Entity> = w.iter::<AnvilTerrain>().map(|(e, _)| e).collect();
        let terrains: Vec<(Entity, AnvilTerrain)> = ids
            .into_iter()
            .filter_map(|e| w.remove::<AnvilTerrain>(e).map(|t| (e, t)))
            .collect();
        let result = self.lower(w, &terrains, log);
        for (e, t) in terrains {
            w.insert(e, t);
        }
        result
    }
}

/// A set's drawn decals: its entity, model, frame and each decal draped.
struct DrapedSet {
    set: Entity,
    model: Entity,
    frame: primase_core::frame::SourceFrame,
    decals: Vec<(DeferredDecal, Draped)>,
}

impl LowerAnvilDecals {
    fn lower(
        &self,
        w: &mut World,
        terrains: &[(Entity, AnvilTerrain)],
        log: &mut PassLog,
    ) -> Result<(), String> {
        let grids: Vec<GroundGrid<'_>> = terrains
            .iter()
            .filter_map(|(_, t)| {
                GroundGrid::new(t, self.rect, self.step, self.range, self.offset, self.sample)
            })
            .collect();
        let sets: Vec<Entity> = w.iter::<AnvilDeferredDecals>().map(|(e, _)| e).collect();
        let [x0, y0, x1, y1] = self.rect;
        let mut draped: Vec<DrapedSet> = Vec::new();
        for e in sets {
            let Some(set) = w.remove::<AnvilDeferredDecals>(e) else {
                continue;
            };
            let n = set.decals.len() as u64;
            let inside: Vec<&DeferredDecal> = set
                .decals
                .iter()
                .filter(|d| {
                    let [x, y] = [f64::from(d.centre[0]), f64::from(d.centre[1])];
                    x0 <= x && x < x1 && y0 <= y && y < y1
                })
                .collect();
            if n > inside.len() as u64 {
                log.omit("deferred decals outside the rectangle", n - inside.len() as u64);
            }
            if inside.is_empty() {
                continue;
            }
            let Some((model, m)) = set
                .model
                .and_then(|m| w.get::<AnvilDecalModel>(m).map(|a| (m, a.clone())))
            else {
                log.omit(
                    "deferred decals whose DDecalModel isn't read",
                    inside.len() as u64,
                );
                continue;
            };
            let FamilyRule::WriteGBufferRoughness = self.family;
            match family(&m) {
                Some(Family::Box) => {}
                Some(Family::Terrain) => {
                    log.omit(
                        "deferred decals of the terrain family (PS 353: the terrain's bake)",
                        inside.len() as u64,
                    );
                    continue;
                }
                None => {
                    log.omit(
                        "deferred decals of a model of neither family",
                        inside.len() as u64,
                    );
                    continue;
                }
            }
            let mut decals = Vec::new();
            for d in inside {
                let alpha = (1.0 - f64::from(d.fade)) * f64::from(m.opacity);
                // even a texel of Mask1.B 1 fails the test (B (1 - fade) m >= cutoff): the engine draws none of it
                if alpha < self.cutoff || alpha <= 0.0 {
                    log.omit(
                        "deferred decals whose fade leaves no texel visible ((1 - fade) m under the cutoff)",
                        1,
                    );
                    continue;
                }
                let [cx, cy] = [f64::from(d.centre[0]), f64::from(d.centre[1])];
                let Some(g) = grids.iter().find(|g| {
                    let (gx0, gx1) = (g.x_of(g.columns[0]), g.x_of(g.columns[g.columns.len() - 1]));
                    let (gy0, gy1) = (g.y_of(g.rows[0]), g.y_of(g.rows[g.rows.len() - 1]));
                    gx0 <= cx && cx <= gx1 && gy0 <= cy && cy <= gy1
                }) else {
                    log.omit("deferred decals off the ground grid", 1);
                    continue;
                };
                let triangles = drape(d, g)?;
                if triangles.is_empty() {
                    log.omit("deferred decals with no ground in their box", 1);
                    continue;
                }
                if d.axis_x[2] != 0.0 || d.axis_y[2] != 0.0 {
                    log.approximate(
                        "deferred decals with a tilted half-axis: their depth their stored bounds'",
                        1,
                    );
                }
                decals.push((*d, Draped { triangles, alpha }));
            }
            if !decals.is_empty() {
                draped.push(DrapedSet {
                    set: e,
                    model,
                    frame: set.frame,
                    decals,
                });
            }
        }
        // each model's material, its k the greatest alpha factor of its drawn decals
        let mut ks: BTreeMap<Entity, f64> = BTreeMap::new();
        for s in &draped {
            for (_, d) in &s.decals {
                let k = ks.entry(s.model).or_insert(0.0);
                *k = k.max(d.alpha);
            }
        }
        let mut materials: BTreeMap<Entity, Option<ModelMaterial>> = BTreeMap::new();
        for (&model, &k) in &ks {
            let Some(m) = w.get::<AnvilDecalModel>(model).cloned() else {
                continue;
            };
            let made = match self.material(w, model, &m, k, log) {
                Ok(material) => Some(ModelMaterial { material, k }),
                Err(why) => {
                    log.omit(why, 1);
                    None
                }
            };
            materials.insert(model, made);
        }
        // every model read is consumed: drawn as its material, or left out
        let models: Vec<Entity> = w.iter::<AnvilDecalModel>().map(|(e, _)| e).collect();
        for e in models {
            w.remove::<AnvilDecalModel>(e);
        }
        let (mut drawn, mut triangles_drawn, mut sets_drawn) = (0u64, 0u64, 0u64);
        for DrapedSet {
            set: e,
            model,
            frame,
            decals,
        } in draped
        {
            let Some(Some(mm)) = materials.get(&model) else {
                log.omit("deferred decals whose model can't be drawn", decals.len() as u64);
                continue;
            };
            let flip = frame.flips_winding();
            let (mut positions, mut normals, mut tangents, mut uvs, mut colours, mut indices) = (
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            );
            for (d, piece) in &decals {
                let (c, x, y) = (
                    d.centre.map(f64::from),
                    d.axis_x.map(f64::from),
                    d.axis_y.map(f64::from),
                );
                let (xx, yy) = (dot(x, x), dot(y, y));
                // the window packed as the engine packs it (f32 arithmetic, truncated)
                let [offset, width] = d.window;
                let lo = f64::from(((offset + width / 2.0) * 65535.0).trunc());
                let hi = f64::from((width / 2.0 * 65535.0).trunc());
                let tangent = unit(x).map(|v| -v);
                let alpha = (piece.alpha / mm.k) as f32;
                for (tri, n) in &piece.triangles {
                    // the bitangent's sign: cross(N, T) w along -Y^
                    let mut sign = if dot(cross(*n, tangent), y) < 0.0 {
                        1.0
                    } else {
                        -1.0
                    };
                    if flip {
                        sign = -sign;
                    }
                    let first = u32::try_from(positions.len()).map_err(|_| "too many decal vertices")?;
                    for p in tri {
                        let (lx, ly) = (dot(sub(*p, c), x) / xx, dot(sub(*p, c), y) / yy);
                        uvs.push([((lo - hi * lx) / 65536.0) as f32, (0.5 + ly / 2.0) as f32]);
                        positions.push(frame.point([p[0] as f32, p[1] as f32, (p[2] + self.lift) as f32]));
                        normals.push(frame.direction(n.map(|v| v as f32)));
                        let t = frame.direction(tangent.map(|v| v as f32));
                        tangents.push([t[0], t[1], t[2], sign]);
                        colours.push([1.0, 1.0, 1.0, alpha]);
                    }
                    indices.extend(if flip {
                        [first, first + 2, first + 1]
                    } else {
                        [first, first + 1, first + 2]
                    });
                }
                triangles_drawn += piece.triangles.len() as u64;
            }
            drawn += decals.len() as u64;
            let origin = local_origin(&positions);
            for p in &mut positions {
                *p = std::array::from_fn(|k| p[k] - origin[k]);
            }
            let mut streams = BTreeMap::new();
            streams.insert(Semantic::Position, Stream::F32x3(positions));
            streams.insert(Semantic::Normal, Stream::F32x3(normals));
            streams.insert(Semantic::Tangent, Stream::F32x4(tangents));
            streams.insert(Semantic::TexCoord(0), Stream::F32x2(uvs));
            if colours.iter().any(|c| c[3] != 1.0) {
                streams.insert(Semantic::Color(0), Stream::F32x4(colours));
            }
            w.insert(
                e,
                Mesh::new(vec![Primitive {
                    topology: Topology::Triangles,
                    streams,
                    indices: Some(indices),
                    material: Some(mm.material),
                }]),
            );
            let placed = w.spawn();
            w.insert(
                placed,
                Instance {
                    prototype: e,
                    variant: 0,
                },
            );
            w.insert(placed, placed_at(origin));
            if let Some(name) = w.get::<Name>(e).cloned() {
                w.insert(placed, name);
            }
            log.place(placed);
            sets_drawn += 1;
        }
        if drawn > 0 {
            log.count("deferred decals drawn", drawn);
            log.count("deferred decal sets drawn", sets_drawn);
            log.count("deferred decal triangles", triangles_drawn);
            log.count(
                "deferred decal models drawn",
                materials.values().filter(|m| m.is_some()).count() as u64,
            );
            log.approximate("deferred decals draped on the ground alone, lifted", drawn);
            log.omit(
                "deferred decals' angle test (its threshold not read) and terrain-mask colour term (not read: 0)",
                drawn,
            );
        }
        Ok(())
    }
}
