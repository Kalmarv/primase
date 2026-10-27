//! `lower-anvil-decals{rect=x0,y0,x1,y1,step=N,range=METRES,offset=METRES,sample=corner|centre,
//! family=write-gbuffer-roughness,cutoff=X,lift=UNITS,terrain-family=out|blend[,terrain-steps=N]}`:
//! the deferred decals ([`AnvilDeferredDecals`]) of the box family (`ddecal.hlsl` PS 381 with VS 382, material-spec
//! §23), and of the terrain family where `terrain-family=blend`, become meshes draped on the ground `lower-anvil-terrain` draws with the same `rect`, `step`, `range`, `offset`
//! and `sample` (run before it, while the terrain is read). Each value is the engine's, read, or a stated choice:
//!
//! - **Which** (stated): the decals whose box's centre lies in `rect` (x0 <= x < x1, y0 <= y < y1), of a DDecalModel
//!   of the box family by `family`'s rule (the terrain family's too with `terrain-family=blend`, else left out,
//!   counted); models without a Diffuse, Normal or Mask1 left out, counted. Which permutation a
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
//! - **The terrain family** (PS 353, 349 and 373, executed in §23.9: the engine writes them into the terrain's bake
//!   tiles, blended by `alpha = Mask1.B (1 - fade) m` where that reaches the cutoff): with `terrain-family=blend` each
//!   is draped as the box family's, its material blended (glTF's blend has no cutoff, and the factor varies per
//!   decal): one material per model and factor quantised to `1 / terrain-steps` (the fades are f32 noise around 0 on
//!   most), its alpha `Mask1.B q` where that reaches `cutoff`, else 0, baked per texel; its colour, roughness and
//!   normal map the box family's. Its displacement and Mask1's red and green (the bake's data layers) are left out.
//!   Reported as approximate: the quantised factor, the cut per texel (the engine cuts the filtered alpha), and the
//!   blend over the ground (the engine's into its bake).
//! - **Draw order**: every decal material is drawn in the decal layer (`core.draw_order`), after the alpha-tested
//!   surfaces (queue 2500, the convention): what a consumer draws over the surface beneath.
//!
//! Left out, counted: the decals' angle test (its threshold `cb3[4].z` not read), the terrain-mask colour term
//! (`k = t18.z`, `cb0[49].x`, not read: taken 0), the Height maps (read only by parallax permutations, which models
//! select those not read), decals on props (the ground alone is draped).

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef};
use primase_core::{
    ColorSpace, Component, DrawLayer, DrawOrder, Entity, Instance, Mesh, Name, NormalBasis, NormalConvention,
    NormalGreen, NormalX, NormalZ, Primitive, Semantic, Stream, TexFormat, TexRole, Texture, Topology, World,
};

use super::{
    CellPoint, GroundGrid, OFFSET, RANGE, RECT, SAMPLE, STEP, ground_options, local_origin, placed_at,
};
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
    /// What the terrain family's decals become.
    pub terrain: TerrainDecals,
}

/// What `lower-anvil-decals` does with the terrain family's decals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainDecals {
    /// Left out, counted.
    Out,
    /// Blended over the ground, their factors quantised to `1 / steps`.
    Blend { steps: u32 },
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

impl Choice for FamilyRule {
    const VALUES: &'static [(&'static str, Self)] =
        &[("write-gbuffer-roughness", FamilyRule::WriteGBufferRoughness)];
}

/// Option `terrain-family`: [`TerrainDecals`]' kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerrainFamily {
    Out,
    Blend,
}

impl Choice for TerrainFamily {
    const VALUES: &'static [(&'static str, Self)] =
        &[("out", TerrainFamily::Out), ("blend", TerrainFamily::Blend)];
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
    /// From its declared options: `terrain-steps` only with `terrain-family=blend`.
    fn from_given(g: &Given) -> Result<LowerAnvilDecals, String> {
        let ground = ground_options(g)?;
        let number = |k: &str| -> Result<f64, String> {
            let v: f64 = g.required(k).parse().map_err(|_| format!("{k} is a number"))?;
            if v.is_finite() && v >= 0.0 {
                Ok(v)
            } else {
                Err(format!("{k} {v}: finite and at least 0"))
            }
        };
        let range = number("range")?;
        if range <= 0.0 {
            return Err(format!("range {range}: above 0"));
        }
        let terrain = match g.choice::<TerrainFamily>("terrain-family") {
            TerrainFamily::Out => {
                if g.optional("terrain-steps").is_some() {
                    return Err("terrain-steps is only taken with terrain-family=blend".into());
                }
                TerrainDecals::Out
            }
            TerrainFamily::Blend => {
                let steps: u32 = g
                    .optional("terrain-steps")
                    .ok_or("terrain-steps is required with terrain-family=blend")?
                    .parse()
                    .map_err(|_| "terrain-steps is a whole number".to_owned())?;
                if !(1..=65535).contains(&steps) {
                    return Err(format!("terrain-steps {steps}: 1 to 65535"));
                }
                TerrainDecals::Blend { steps }
            }
        };
        Ok(LowerAnvilDecals {
            rect: ground.rect,
            step: ground.step,
            range,
            offset: ground.offset,
            sample: ground.sample,
            family: g.choice("family"),
            cutoff: number("cutoff")?,
            lift: number("lift")?,
            terrain,
        })
    }
}

/// `lower-anvil-decals`' declaration ([`primase_core::pass::PassRegistry::add`]): the ground's options are those of
/// the `lower-anvil-terrain` it lies on; every choice is the program's.
pub const LOWER_ANVIL_DECALS: PassSpec = PassSpec {
    name: "lower-anvil-decals",
    options: &[
        RECT,
        STEP,
        RANGE,
        OFFSET,
        SAMPLE,
        OptionSpec::choice::<FamilyRule>("family", "which models are of the box family"),
        OptionSpec::required("cutoff", "0.4", "the alpha test's ref / scale, at least 0"),
        OptionSpec::required(
            "lift",
            "0.01",
            "source units, at least 0: how far the decals lie above the ground, along the source's up",
        ),
        OptionSpec::choice::<TerrainFamily>(
            "terrain-family",
            "the terrain family's decals left out (counted), or blended over the ground",
        ),
        OptionSpec::optional(
            "terrain-steps",
            "255",
            "1 to 65535: the blended factors quantised to 1 / steps (terrain-family=blend)",
            "required with terrain-family=blend, not taken with out",
        ),
    ],
    make: |g| Ok(Box::new(LowerAnvilDecals::from_given(g)?)),
};

/// A point in the source frame, f64.
type P = [f64; 3];

use primase_core::vector::{cross, dot, sub, unit};

/// `polygon` cut to where `f` is at least 0 (`f` linear), in order ([`primase_meshops::clip_half_plane`]).
fn clip(polygon: &[P], f: impl Fn(P) -> f64) -> Vec<P> {
    primase_meshops::clip_half_plane(
        polygon,
        |&p| f(p),
        |a, b, s| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * s),
    )
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
                // the grid's squares have extent, so its triangles have area
                let normal = unit(cross(sub(tri[1], tri[0]), sub(tri[2], tri[0])))
                    .ok_or("a ground triangle with no area")?;
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

/// How a decal material draws its alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Draw {
    /// The box family's: masked at `cutoff / k`, each decal's factor over `k` in its vertices' `COLOR_0` alpha.
    Mask { k: f64 },
    /// The terrain family's: blended, its alpha `Mask1.B q` where that reaches `cutoff`, else 0, baked into the texture.
    Blend { q: f64 },
}

/// What a draped decal draws with: the box family's model material, or the terrain family's at a quantised factor
/// (its index of `terrain-steps`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum DrawKey {
    Box,
    Terrain(u32),
}

/// The queue a decal material is given: after the alpha-tested surfaces, before the blended ones (the convention other
/// readers' decals take; the engine's pass sequence draws its deferred decals after the opaque surfaces they cover).
pub(super) const DECAL_QUEUE: i32 = 2500;

impl LowerAnvilDecals {
    /// A decal material on `target`, of the model `m`'s textures composed: base colour the Diffuse with its alpha
    /// (`Mask1`'s blue, as `draw` takes it), roughness the Normal's alpha in green; its Normal marked as the decal's normal
    /// map; drawn in the decal layer. `Err` names why a model can't be drawn.
    fn material(
        &self,
        w: &mut World,
        target: Entity,
        m: &AnvilDecalModel,
        draw: Draw,
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
        // the alpha the shader reads: Mask1's blue, made linear where it's viewed sRGB
        let blue = |b: u8| {
            if mask_srgb {
                primase_foundations::color::srgb8_to_linear(b)
            } else {
                f64::from(b) / 255.0
            }
        };
        if mask_srgb {
            log.approximate(
                "decal models whose Mask1 is viewed sRGB: its blue made linear, to the nearest 8-bit value",
                1,
            );
        }
        let alpha = |b: u8| -> u8 {
            let a = match draw {
                Draw::Mask { .. } => blue(b),
                // the cut baked in: what passes keeps its blend weight, the rest is clear
                Draw::Blend { q } => {
                    let a = blue(b) * q;
                    if a >= self.cutoff { a } else { 0.0 }
                }
            };
            (a.clamp(0.0, 1.0) * 255.0).round() as u8
        };
        let base: Vec<u8> = colour.levels[0]
            .as_chunks::<4>()
            .0
            .iter()
            .zip(shape.levels[0].as_chunks::<4>().0)
            .flat_map(|(c, s)| [c[0], c[1], c[2], alpha(s[2])])
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
            target,
            PbrMaterial {
                base_color_texture: Some(TextureRef::new(base_texture)),
                metallic: 0.0,
                roughness: 1.0,
                metallic_roughness_texture: Some(TextureRef::new(rough_texture)),
                normal_texture: Some(TextureRef::new(normal)),
                alpha_mode: match draw {
                    Draw::Mask { k } => AlphaMode::Mask {
                        cutoff: (self.cutoff / k) as f32,
                    },
                    Draw::Blend { .. } => AlphaMode::Blend,
                },
                ..PbrMaterial::default()
            },
        );
        w.insert(target, DrawOrder::conventional(DrawLayer::Decal, DECAL_QUEUE));
        Ok(target)
    }
}

impl Pass for LowerAnvilDecals {
    fn name(&self) -> &'static str {
        "lower-anvil-decals"
    }
    fn options(&self) -> Options {
        let [x0, y0, x1, y1] = self.rect;
        let mut o = Options::from([
            ("rect".to_owned(), format!("{x0},{y0},{x1},{y1}")),
            ("step".to_owned(), self.step.to_string()),
            ("range".to_owned(), self.range.to_string()),
            ("offset".to_owned(), self.offset.to_string()),
            ("sample".to_owned(), self.sample.text().to_owned()),
            ("family".to_owned(), self.family.text().to_owned()),
            ("cutoff".to_owned(), self.cutoff.to_string()),
            ("lift".to_owned(), self.lift.to_string()),
        ]);
        match self.terrain {
            TerrainDecals::Out => {
                o.insert("terrain-family".to_owned(), TerrainFamily::Out.text().to_owned());
            }
            TerrainDecals::Blend { steps } => {
                o.insert(
                    "terrain-family".to_owned(),
                    TerrainFamily::Blend.text().to_owned(),
                );
                o.insert("terrain-steps".to_owned(), steps.to_string());
            }
        }
        o
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilDeferredDecals::NAME, AnvilTerrain::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            Mesh::NAME,
            PbrMaterial::NAME,
            DrawOrder::NAME,
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
         colour term left out; the terrain family blended over the ground at quantised factors"
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

/// A set's drawn decals: its entity, model, frame and each decal draped with what it draws with.
struct DrapedSet {
    set: Entity,
    model: Entity,
    frame: primase_core::frame::SourceFrame,
    decals: Vec<(DeferredDecal, Draped, DrawKey)>,
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
            let terrain_family = match (family(&m), self.terrain) {
                (Some(Family::Box), _) => false,
                (Some(Family::Terrain), TerrainDecals::Blend { .. }) => true,
                (Some(Family::Terrain), TerrainDecals::Out) => {
                    log.omit(
                        "deferred decals of the terrain family (PS 353: the terrain's bake)",
                        inside.len() as u64,
                    );
                    continue;
                }
                (None, _) => {
                    log.omit(
                        "deferred decals of a model of neither family",
                        inside.len() as u64,
                    );
                    continue;
                }
            };
            let mut decals = Vec::new();
            for d in inside {
                let factor = (1.0 - f64::from(d.fade)) * f64::from(m.opacity);
                // what it draws with; even a texel of Mask1.B 1 fails the test where the factor is under the cutoff:
                // the engine draws none of it
                let key = match self.terrain {
                    TerrainDecals::Blend { steps } if terrain_family => {
                        let index = (factor * f64::from(steps)).round().max(0.0);
                        // in range of u32: at most steps times the factor's bound
                        let q = index / f64::from(steps);
                        if q < self.cutoff || q <= 0.0 {
                            log.omit(
                                "terrain-family decals whose quantised factor leaves no texel visible",
                                1,
                            );
                            continue;
                        }
                        DrawKey::Terrain(index as u32)
                    }
                    _ => {
                        if factor < self.cutoff || factor <= 0.0 {
                            log.omit(
                                "deferred decals whose fade leaves no texel visible ((1 - fade) m under the cutoff)",
                                1,
                            );
                            continue;
                        }
                        DrawKey::Box
                    }
                };
                let [cx, cy] = [f64::from(d.centre[0]), f64::from(d.centre[1])];
                let Some(g) = grids.iter().find(|g| {
                    let (gx0, gx1) = (g.x_of(g.columns[0]), g.x_of(g.columns[g.columns.len() - 1]));
                    let (gy0, gy1) = (g.y_of(g.rows[0]), g.y_of(g.rows[g.rows.len() - 1]));
                    gx0 <= cx && cx <= gx1 && gy0 <= cy && cy <= gy1
                }) else {
                    log.omit("deferred decals off the ground grid", 1);
                    continue;
                };
                if d.axis_x == [0.0; 3] || d.axis_y == [0.0; 3] {
                    log.omit("deferred decals with a zero half-axis", 1);
                    continue;
                }
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
                decals.push((
                    *d,
                    Draped {
                        triangles,
                        alpha: factor,
                    },
                    key,
                ));
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
        // each material: the box family's per model, its k the greatest factor of its drawn decals; the terrain
        // family's per model and quantised factor
        let mut ks: BTreeMap<Entity, f64> = BTreeMap::new();
        let mut terrain_keys: BTreeSet<(Entity, u32)> = BTreeSet::new();
        for s in &draped {
            for (_, d, key) in &s.decals {
                match key {
                    DrawKey::Box => {
                        let k = ks.entry(s.model).or_insert(0.0);
                        *k = k.max(d.alpha);
                    }
                    DrawKey::Terrain(index) => {
                        terrain_keys.insert((s.model, *index));
                    }
                }
            }
        }
        // per (model, key): its material entity and the box family's k
        let mut materials: BTreeMap<(Entity, DrawKey), Option<(Entity, f64)>> = BTreeMap::new();
        for (&model, &k) in &ks {
            let Some(m) = w.get::<AnvilDecalModel>(model).cloned() else {
                continue;
            };
            let made = match self.material(w, model, &m, Draw::Mask { k }, log) {
                Ok(material) => Some((material, k)),
                Err(why) => {
                    log.omit(why, 1);
                    None
                }
            };
            materials.insert((model, DrawKey::Box), made);
        }
        if let TerrainDecals::Blend { steps } = self.terrain {
            for &(model, index) in &terrain_keys {
                let Some(m) = w.get::<AnvilDecalModel>(model).cloned() else {
                    continue;
                };
                let q = f64::from(index) / f64::from(steps);
                let target = w.spawn();
                if let Some(Name(name)) = w.get::<Name>(model).cloned() {
                    w.insert(target, Name(format!("{name} at {q}")));
                }
                let made = match self.material(w, target, &m, Draw::Blend { q }, log) {
                    Ok(material) => Some((material, q)),
                    Err(why) => {
                        log.omit(why, 1);
                        None
                    }
                };
                materials.insert((model, DrawKey::Terrain(index)), made);
            }
        }
        // every model read is consumed: drawn as its materials, or left out
        let models: Vec<Entity> = w.iter::<AnvilDecalModel>().map(|(e, _)| e).collect();
        for e in models {
            w.remove::<AnvilDecalModel>(e);
        }
        let (mut drawn, mut terrain_drawn, mut triangles_drawn, mut sets_drawn) = (0u64, 0u64, 0u64, 0u64);
        for DrapedSet {
            set: e,
            model,
            frame,
            decals,
        } in draped
        {
            let flip = frame.flips_winding();
            // a primitive per material the set's decals draw with
            let mut groups: BTreeMap<DrawKey, Vec<(DeferredDecal, Draped)>> = BTreeMap::new();
            for (d, piece, key) in decals {
                groups.entry(key).or_default().push((d, piece));
            }
            let mut primitives = Vec::new();
            let mut all_positions: Vec<[f32; 3]> = Vec::new();
            for (key, group) in groups {
                let Some(Some((material, k))) = materials.get(&(model, key)) else {
                    log.omit("deferred decals whose model can't be drawn", group.len() as u64);
                    continue;
                };
                let (mut positions, mut normals, mut tangents, mut uvs, mut colours, mut indices) = (
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                );
                for (d, piece) in &group {
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
                    // decals with a zero half-axis were left out above
                    let tangent = unit(x).ok_or("a deferred decal with no x half-axis")?.map(|v| -v);
                    let alpha = match key {
                        DrawKey::Box => (piece.alpha / k) as f32,
                        DrawKey::Terrain(_) => 1.0,
                    };
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
                            positions.push(frame.point([
                                p[0] as f32,
                                p[1] as f32,
                                (p[2] + self.lift) as f32,
                            ]));
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
                match key {
                    DrawKey::Box => drawn += group.len() as u64,
                    DrawKey::Terrain(_) => terrain_drawn += group.len() as u64,
                }
                all_positions.extend(&positions);
                let mut streams = BTreeMap::new();
                streams.insert(Semantic::Position, Stream::F32x3(positions));
                streams.insert(Semantic::Normal, Stream::F32x3(normals));
                streams.insert(Semantic::Tangent(0), Stream::F32x4(tangents));
                streams.insert(Semantic::TexCoord(0), Stream::F32x2(uvs));
                if colours.iter().any(|c| c[3] != 1.0) {
                    streams.insert(Semantic::Color(0), Stream::F32x4(colours));
                }
                primitives.push(Primitive {
                    topology: Topology::Triangles,
                    streams,
                    indices: Some(indices),
                    material: Some(*material),
                });
            }
            if primitives.is_empty() {
                continue;
            }
            // the set's vertices local to its origin
            let origin = local_origin(&all_positions);
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
            w.insert(placed, placed_at(origin));
            if let Some(name) = w.get::<Name>(e).cloned() {
                w.insert(placed, name);
            }
            log.place(placed);
            sets_drawn += 1;
        }
        if drawn > 0 {
            log.count("deferred decals drawn", drawn);
        }
        if terrain_drawn > 0 {
            log.count("terrain-family decals drawn", terrain_drawn);
            log.approximate(
                "terrain-family decals blended over the ground, their factor to 1/terrain-steps and their cut baked per \
                 texel (the engine blends them into the terrain's bake and cuts the filtered alpha)",
                terrain_drawn,
            );
        }
        if drawn + terrain_drawn > 0 {
            log.count("deferred decal sets drawn", sets_drawn);
            log.count("deferred decal triangles", triangles_drawn);
            log.count(
                "deferred decal materials",
                materials.values().filter(|m| m.is_some()).count() as u64,
            );
            log.approximate(
                "deferred decals draped on the ground alone, lifted",
                drawn + terrain_drawn,
            );
            log.omit(
                "deferred decals' angle test (its threshold not read) and terrain-mask colour term (not read: 0)",
                drawn + terrain_drawn,
            );
        }
        Ok(())
    }
}
