//! `bake-forzatech-swatch-tint`: the swatch tint of `environment` materials, applied as material variants.
//!
//! The shader multiplies the albedo by `lerp(tint, 1, 1 - a (1 - c.r))`, `a` the diffuse alpha, `c` the vertex colour
//! and `tint` the `SwatchTintTexture` read at `(c.g, v)`, `v` the first of the placement's instance
//! variation. The vertex colour is the model's (the reader keeps it as `ShaderInput(0)`: it is data, not a
//! colour), so which triangles take the tint and from which palette column is the mesh's, and which palette row is the
//! placement's. This pass applies both:
//!
//! - each swatch-tinted primitive is split by its triangles' vertex colour: those with red under one half (a corner
//!   majority) are tinted, grouped by their green (the first corner's), the rest are not; a primitive of one group
//!   stays whole;
//! - each prototype's placements are given new material variants, one per distinct (model variant, palette row) among
//!   them ([`Instance::variant`] is rewritten to that dense number), whose tinted sub-primitives draw a copy of the
//!   variant's material whose base colour texture is baked with the tint: each texel's colour times
//!   `(1 - a) + a tint` (the shader's product at red 0, in linear light), alpha kept, every stored level. The tint is
//!   the palette's texel (`point` filtering at the texel the coordinates fall in, level 0, the palette's transfer).
//!   Textures are baked once per (texture, tint) and materials copied once per (material, tint), so the placements
//!   with the same tint share both. `specialize-material-variants` then makes a prototype of each distinct set of
//!   materials;
//! - the vertex colour streams are dropped from every mesh.
//!
//! Run after `select-lod` and `lower-forzatech-materials` and before `specialize-material-variants`, and after the
//! texture cap a destination asks for (`cap-textures` takes the stored levels it keeps, so the copies are made at the
//! size they are kept, not at the stored size): a prototype with levels of detail is left as it is (counted), as the
//! specializing pass refuses variants beside levels.
//!
//! The palette row of a placement is the shader's instance variation: a rule record's byte over 255. A hand-placed
//! instance's transform holds four floats where a rule record holds four bytes (both are all ones, all zeros, or a
//! varied first with the rest ones, in about the same shares; the swatch-tinted buildings' hand tints are round
//! authored fractions such as 0.514), so its first float is the row as it is (inferred); one outside 0 to 1 draws
//! untinted.
//!
//! Left out, counted: a triangle whose corners disagree on red (the majority decides) or, among the tinted,
//! on green (the first corner's); the palette's sampler (it is read at one texel, not filtered: the shader reads it through a run-time
//! sampler, `s11`, whose filter is in no file; the pass counts the texels a bilinear read would change); a material with no
//! base colour texture takes the tint as its colour factor, whole; and a texel's mip levels are tinted by their own
//! alpha, not by the alpha of the level the shader would mix.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog, PassSpec};
use primase_core::pbr::PbrMaterial;
use primase_core::{
    ColorSpace, Component, Entity, Instance, Lods, MaterialVariants, Mesh, Name, Primitive, Semantic, Stream,
    TexRole, Texture, Topology, World,
};
use primase_foundations::color::{linear_to_srgb8, srgb8_to_linear};
use primase_foundations::sampling::{Address, Levels, Transfer};
use primase_image::TextureMetadata;

use crate::components::{MeshRecords, Placement, PlacementOrigin, SwatchTint};

/// See the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BakeForzaSwatchTint;

/// `bake-forzatech-swatch-tint`'s declaration ([`primase_core::pass::PassRegistry::add`]).
pub const BAKE_FORZATECH_SWATCH_TINT: PassSpec = PassSpec {
    name: "bake-forzatech-swatch-tint",
    options: &[],
    make: |_| Ok(Box::new(BakeForzaSwatchTint)),
};

/// The vertex colour stream the reader keeps (`read/models.rs`).
const COLOUR: Semantic = Semantic::ShaderInput(0);

/// A palette row: the placement's instance variation as the `v` coordinate, an `f32`'s bits (so rows order and compare).
type Row = u32;

/// A material variant a placement draws: the model's variant and the palette row (`None`: untinted).
type Key = (u32, Option<Row>);

/// A channel of the vertex colour as the byte it was stored as.
fn byte(x: f32) -> u8 {
    (x * 255.0).round().clamp(0.0, 255.0) as u8
}

/// One triangle group of a primitive: `None` is the untinted, `Some(g)` is tinted with palette column `g`.
type Group = Option<u8>;

/// A swatch primitive's triangles by group.
#[derive(Default)]
struct Split {
    groups: BTreeMap<Group, Vec<u32>>,
    mixed_red: u64,
    mixed_green: u64,
}

/// `indices`' triangles by the group their vertex colours put them in.
fn split(indices: &[u32], colours: &[[f32; 4]]) -> Result<Split, String> {
    let mut out = Split::default();
    for t in indices.as_chunks::<3>().0 {
        let mut corners = [[0.0f32; 4]; 3];
        for (c, &k) in corners.iter_mut().zip(t) {
            *c = *colours
                .get(k as usize)
                .ok_or_else(|| format!("index {k} past {} vertex colours", colours.len()))?;
        }
        let red = corners.map(|c| c[0] < 0.5);
        let on = red.iter().filter(|r| **r).count();
        if on != 0 && on != 3 {
            out.mixed_red += 1;
        }
        let group = if on * 2 > 3 {
            let g = byte(corners[0][1]);
            if corners.iter().any(|c| byte(c[1]) != g) {
                out.mixed_green += 1;
            }
            Some(g)
        } else {
            None
        };
        out.groups.entry(group).or_default().extend_from_slice(t);
    }
    Ok(out)
}

/// The palettes read so far, decoded once, with the transfer the shader's view applies.
#[derive(Default)]
struct Palettes {
    levels: BTreeMap<Entity, Option<(Levels, Transfer)>>,
    /// The (palette, column, row) read so far.
    seen: BTreeSet<(Entity, u8, Row)>,
    /// Of those, how many a bilinear read differs from the point read in a channel by more than two stored values,
    /// clamping and wrapping at the edges.
    filtered_differs: [u64; 2],
}

/// Two stored values, the margin within which a filtered read is taken to agree with the point read.
const AGREE: f64 = 2.0;

impl Palettes {
    /// The palette's linear colour at column `g`, row `row` read at the texel the coordinates fall in; `None` when it
    /// can't be read. The first read of each texel also compares it with a bilinear read.
    fn colour(&mut self, w: &World, palette: Entity, g: u8, row: Row) -> Result<Option<[f32; 3]>, String> {
        let Some((levels, transfer)) = self
            .levels
            .entry(palette)
            .or_insert_with(|| {
                let t = w.get::<Texture>(palette)?;
                let transfer = if t.color_space == ColorSpace::Srgb {
                    Transfer::SrgbColour
                } else {
                    Transfer::Stored
                };
                primase_image::levels(t).ok().map(|l| (l, transfer))
            })
            .as_ref()
        else {
            return Ok(None);
        };
        let uv = [f64::from(g) / 255.0, f64::from(f32::from_bits(row))];
        let point = levels
            .point(0, uv, [Address::Clamp; 2], *transfer)
            .map_err(|e| e.to_string())?;
        if self.seen.insert((palette, g, row)) {
            // as stored again, to compare in the values the texture holds
            let stored = |c: f64| match transfer {
                Transfer::SrgbColour => f64::from(linear_to_srgb8(c)),
                _ => (c.clamp(0.0, 1.0) * 255.0).round(),
            };
            for (k, address) in [Address::Clamp, Address::Wrap].into_iter().enumerate() {
                let filtered = levels
                    .bilinear(0, uv, [address; 2], *transfer)
                    .map_err(|e| e.to_string())?;
                if (0..3).any(|c| (stored(filtered[c]) - stored(point[c])).abs() > AGREE) {
                    self.filtered_differs[k] += 1;
                }
            }
        }
        Ok(Some([point[0] as f32, point[1] as f32, point[2] as f32]))
    }
}

/// What a prototype's baking decided, before the world changes.
struct Plan {
    /// Per primitive, its groups in draw order, each with its triangles' indices (`None`: the whole primitive).
    groups: Vec<Vec<(Group, Option<Vec<u32>>)>>,
    /// The tint a (material, palette, column, row) draws.
    requests: BTreeSet<(Entity, Entity, u8, Row)>,
}

impl Plan {
    /// The primitives of `mesh`: each swatch primitive with a colour stream split, the rest whole.
    fn of(
        mesh: &Mesh,
        swatchy: &[bool],
        keys: &BTreeMap<Key, u32>,
        resolved: impl Fn(usize, u32) -> Option<Entity>,
        swatch: &BTreeMap<Entity, Entity>,
        log: &mut PassLog,
    ) -> Result<Plan, String> {
        let mut groups = Vec::new();
        let mut requests = BTreeSet::new();
        for (i, p) in mesh.primitives().iter().enumerate() {
            let whole = vec![(None, None)];
            if !swatchy[i] {
                groups.push(whole);
                continue;
            }
            let Some(Stream::F32x4(colours)) = p.streams.get(&COLOUR) else {
                log.omit(
                    "swatch-tinted primitives with no vertex colour: untinted",
                    p.indices.as_ref().map_or(0, |x| x.len() as u64 / 3),
                );
                groups.push(whole);
                continue;
            };
            if p.topology != Topology::Triangles {
                groups.push(whole);
                continue;
            }
            let sequential: Vec<u32>;
            let indices = match &p.indices {
                Some(x) => x,
                None => {
                    sequential = (0..u32::try_from(colours.len()).map_err(|_| "vertex count")?).collect();
                    &sequential
                }
            };
            let split = split(indices, colours)?;
            log.count(
                "swatch-tinted triangles whose corners disagree on the tint mask",
                split.mixed_red,
            );
            log.count(
                "tinted triangles whose corners disagree on the palette column",
                split.mixed_green,
            );
            let single = split.groups.len() == 1;
            let list: Vec<(Group, Option<Vec<u32>>)> = split
                .groups
                .into_iter()
                .map(|(g, ix)| (g, if single { None } else { Some(ix) }))
                .collect();
            for (g, _) in &list {
                let Some(g) = *g else { continue };
                for &(v, row) in keys.keys() {
                    let Some(row) = row else { continue };
                    if let Some(m) = resolved(i, v)
                        && let Some(&palette) = swatch.get(&m)
                    {
                        requests.insert((m, palette, g, row));
                    }
                }
            }
            groups.push(list);
        }
        Ok(Plan { groups, requests })
    }
}

/// The shared state of one run.
struct Bake<'a> {
    swatch: &'a BTreeMap<Entity, Entity>,
    palettes: Palettes,
    /// The tinted material of (material, tint) so placements with one tint share it.
    materials: BTreeMap<(Entity, [u32; 3]), Entity>,
    /// The baked base colour texture of (texture, tint).
    baked: BTreeMap<(Entity, [u32; 3]), Option<Entity>>,
    /// The triangles of the tinted groups, over every prototype.
    tinted_triangles: u64,
}

impl Bake<'_> {
    /// `source` with `tint` applied to each texel, as the shader does where the vertex red is 0: the colour (decoded
    /// as the shader's view does) times `(1 - a) + a tint`, alpha kept. `None` when `source` has no pixels.
    fn baked_texture(
        &mut self,
        w: &mut World,
        source: Entity,
        tint: [f32; 3],
        log: &mut PassLog,
    ) -> Result<Option<Entity>, String> {
        let key = (source, tint.map(f32::to_bits));
        if let Some(&done) = self.baked.get(&key) {
            return Ok(done);
        }
        let Some(t) = w.get::<Texture>(source) else {
            log.omit("swatch-tinted base colour textures with no pixels: untinted", 1);
            self.baked.insert(key, None);
            return Ok(None);
        };
        let space = t.color_space;
        let decode: [f64; 256] = std::array::from_fn(|x| {
            if space == ColorSpace::Srgb {
                srgb8_to_linear(x as u8)
            } else {
                x as f64 / 255.0
            }
        });
        let encode = |x: f64| {
            if space == ColorSpace::Srgb {
                linear_to_srgb8(x)
            } else {
                (x.clamp(0.0, 1.0) * 255.0).round() as u8
            }
        };
        let texture = primase_image::map_rgba8_levels(
            t,
            0..t.levels.len(),
            TextureMetadata {
                color_space: space,
                role: TexRole::Color,
                normal: None,
            },
            |texel| {
                let [r, g, b, a] = texel.rgba;
                let a1 = f64::from(a) / 255.0;
                let factor = |c: usize| (1.0 - a1) + a1 * f64::from(tint[c]);
                Ok([
                    encode(decode[usize::from(r)] * factor(0)),
                    encode(decode[usize::from(g)] * factor(1)),
                    encode(decode[usize::from(b)] * factor(2)),
                    a,
                ])
            },
        )
        .map_err(|e| format!("baking a swatch tint into a base colour texture: {e}"))?;
        let name = w.get::<Name>(source).map(|n| n.0.clone()).unwrap_or_default();
        let e = w.spawn();
        w.insert(e, Name(format!("{name} (swatch tint)")));
        w.insert(e, texture);
        log.count(
            "swatch-tinted base colour textures baked, one per texture and tint",
            1,
        );
        self.baked.insert(key, Some(e));
        Ok(Some(e))
    }

    /// A copy of swatch material `m` drawing the tint: its base colour texture baked with it, else (no texture, or
    /// none with pixels) its colour factor multiplied.
    fn tinted_material(
        &mut self,
        w: &mut World,
        m: Entity,
        tint: [f32; 3],
        log: &mut PassLog,
    ) -> Result<Entity, String> {
        let texture = w
            .get::<PbrMaterial>(m)
            .ok_or("a swatch-tinted material that is not lowered")?
            .base_color_texture
            .map(|slot| slot.texture);
        let baked = match texture {
            Some(t) => self.baked_texture(w, t, tint, log)?,
            None => None,
        };
        let copy = w.duplicate(m).ok_or("a dead swatch material")?;
        w.remove::<SwatchTint>(copy);
        let pbr = w
            .get_mut::<PbrMaterial>(copy)
            .ok_or("a swatch-tinted material that is not lowered")?;
        match (baked, pbr.base_color_texture.as_mut()) {
            (Some(b), Some(slot)) => slot.texture = b,
            _ => {
                for (c, t) in pbr.base_color.iter_mut().zip(tint) {
                    *c *= t;
                }
                log.approximate(
                    "swatch tint of a material with no base colour texture taken as its colour factor: materials",
                    1,
                );
            }
        }
        Ok(copy)
    }

    /// The key of each placement of a prototype, with the dense variant number it takes.
    fn keys(w: &World, placements: &[Entity], log: &mut PassLog) -> (BTreeMap<Key, u32>, Vec<(Entity, Key)>) {
        let mut by_placement = Vec::new();
        for &p in placements {
            let variant = w.get::<Instance>(p).map_or(0, |i| i.variant);
            let tint = match w.get::<Placement>(p).map(|x| &x.origin) {
                // the shader's instance variation is the byte over 255
                Some(PlacementOrigin::Rule { variation, .. }) => {
                    Some((f32::from(variation[0]) / 255.0).to_bits())
                }
                // a hand-placed instance's transform holds the four floats a rule record holds as bytes (both hold
                // all ones, all zeros, or a varied first with the rest ones, in about the same shares): the first is
                // the row as it is
                Some(PlacementOrigin::Hand { tint, .. }) => {
                    if (0.0..=1.0).contains(&tint[0]) {
                        log.approximate(
                            "hand-placed swatch-tinted instances take the first of their transform's tint as the \
                             instance variation (inferred): instances",
                            1,
                        );
                        Some(tint[0].to_bits())
                    } else {
                        log.omit(
                            "hand-placed instances of swatch-tinted prototypes whose tint is outside 0 to 1: untinted",
                            1,
                        );
                        None
                    }
                }
                None => {
                    log.omit(
                        "instances of swatch-tinted prototypes with no placement record: untinted",
                        1,
                    );
                    None
                }
            };
            by_placement.push((p, (variant, tint)));
        }
        let distinct: BTreeSet<Key> = by_placement.iter().map(|(_, k)| *k).collect();
        let ids = distinct.into_iter().zip(0u32..).collect::<BTreeMap<Key, u32>>();
        (ids, by_placement)
    }

    fn prototype(
        &mut self,
        w: &mut World,
        proto: Entity,
        placements: &[Entity],
        log: &mut PassLog,
    ) -> Result<(), String> {
        let Some(mesh) = w.get::<Mesh>(proto) else {
            return Ok(());
        };
        let variants: Vec<BTreeMap<u32, Entity>> = w
            .get::<MaterialVariants>(proto)
            .map(|v| v.0.clone())
            .unwrap_or_default();
        let own: Vec<Option<Entity>> = mesh.primitives().iter().map(|p| p.material).collect();
        let swatchy: Vec<bool> = own
            .iter()
            .enumerate()
            .map(|(i, m)| {
                m.iter()
                    .chain(variants.get(i).into_iter().flat_map(BTreeMap::values))
                    .any(|m| self.swatch.contains_key(m))
            })
            .collect();
        if !swatchy.iter().any(|b| *b) {
            return Ok(());
        }
        let primitive_triangles: Vec<u64> = mesh
            .primitives()
            .iter()
            .map(|p| p.indices.as_ref().map_or(0, |i| i.len() as u64 / 3))
            .collect();
        let triangles: u64 = primitive_triangles.iter().sum();
        if w.has::<Lods>(proto) {
            log.omit(
                "swatch-tinted prototypes with levels of detail: untinted (select-lod first)",
                1,
            );
            return Ok(());
        }
        let resolved = |i: usize, v: u32| variants.get(i).and_then(|m| m.get(&v)).copied().or(own[i]);
        let (ids, by_placement) = Self::keys(w, placements, log);
        let plan = Plan::of(mesh, &swatchy, &ids, resolved, self.swatch, log)?;

        // the tinted materials the plan asks for
        let mut made: BTreeMap<(Entity, u8, Row), Entity> = BTreeMap::new();
        for &(m, palette, g, row) in &plan.requests {
            let Some(rgb) = self.palettes.colour(w, palette, g, row)? else {
                log.omit("swatch tint palettes that can't be read: untinted", 1);
                continue;
            };
            let key = (m, rgb.map(f32::to_bits));
            let tinted = match self.materials.get(&key) {
                Some(&t) => t,
                None => {
                    let t = self.tinted_material(w, m, rgb, log)?;
                    log.count("swatch-tinted material copies made, one per material and tint", 1);
                    self.materials.insert(key, t);
                    t
                }
            };
            made.insert((m, g, row), tinted);
        }

        // the forms specialize will make: the materials each key draws, before and after
        let before: BTreeSet<Vec<Option<Entity>>> = ids
            .keys()
            .map(|&(v, _)| (0..own.len()).map(|i| resolved(i, v)).collect())
            .collect();

        let mut from = Vec::new();
        let mut maps: Vec<BTreeMap<u32, Entity>> = Vec::new();
        let mut cut: Vec<Option<Vec<u32>>> = Vec::new();
        for (i, list) in plan.groups.iter().enumerate() {
            for (g, indices) in list {
                if g.is_some() {
                    self.tinted_triangles += indices
                        .as_ref()
                        .map_or(primitive_triangles[i], |x| x.len() as u64 / 3);
                }
                from.push(i);
                cut.push(indices.clone());
                let mut map = BTreeMap::new();
                for (&(v, row), &id) in &ids {
                    let base = resolved(i, v);
                    let drawn = match (g, row, base) {
                        (Some(g), Some(row), Some(m)) => made.get(&(m, *g, row)).copied().or(Some(m)),
                        _ => base,
                    };
                    if let Some(d) = drawn
                        && Some(d) != own[i]
                    {
                        map.insert(id, d);
                    }
                }
                maps.push(map);
            }
        }
        let after: BTreeSet<Vec<Option<Entity>>> = ids
            .values()
            .map(|id| {
                maps.iter()
                    .zip(&from)
                    .map(|(m, &i)| m.get(id).copied().or(own[i]))
                    .collect()
            })
            .collect();

        let (before_primitives, after_primitives) = (own.len() as u64, from.len() as u64);
        w.rebuild_primitives(proto, |mesh| {
            let mut primitives: Vec<Primitive> = Vec::with_capacity(from.len());
            for (&i, indices) in from.iter().zip(&cut) {
                let source = mesh.primitives().get(i).ok_or("listed")?;
                primitives.push(match indices {
                    None => source.clone(),
                    Some(ix) => primase_meshops::subset_primitive(source, ix).map_err(|e| e.to_string())?,
                });
            }
            Ok((primitives, from.clone()))
        })?;
        if !variants.is_empty() || maps.iter().any(|m| !m.is_empty()) {
            w.insert(proto, MaterialVariants(maps));
        }
        for (p, key) in by_placement {
            if let (Some(&id), Some(i)) = (ids.get(&key), w.get_mut::<Instance>(p)) {
                i.variant = id;
            }
        }

        log.count("swatch-tinted prototypes", 1);
        log.count(
            "primitives of swatch-tinted prototypes before the split",
            before_primitives,
        );
        log.count(
            "primitives of swatch-tinted prototypes after the split",
            after_primitives,
        );
        log.count("swatch-tint material variants of prototypes", ids.len() as u64);
        let added = after.len().saturating_sub(before.len()) as u64;
        log.count(
            "prototype copies specialize-material-variants adds for swatch tint",
            added,
        );
        log.count("triangles those prototype copies add", added * triangles);
        log.count("triangles of swatch-tinted prototypes", triangles);
        Ok(())
    }
}

impl Pass for BakeForzaSwatchTint {
    fn name(&self) -> &'static str {
        "bake-forzatech-swatch-tint"
    }
    fn options(&self) -> Options {
        Options::new()
    }
    fn provides(&self) -> &[&'static str] {
        &[MaterialVariants::NAME, PbrMaterial::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[
            Mesh::NAME,
            Instance::NAME,
            MaterialVariants::NAME,
            MeshRecords::NAME,
        ]
    }
    fn removes(&self) -> &[&'static str] {
        &[SwatchTint::NAME]
    }
    fn approximates(&self) -> &'static str {
        "the swatch palette is read at one texel, not filtered; a hand-placed instance's tint byte is inferred from its transform's tint"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let swatch: BTreeMap<Entity, Entity> = w.iter::<SwatchTint>().map(|(e, s)| (e, s.palette)).collect();
        if !swatch.is_empty() {
            let mut placed: BTreeMap<Entity, Vec<Entity>> = BTreeMap::new();
            for (p, i) in w.iter::<Instance>() {
                placed.entry(i.prototype).or_default().push(p);
            }
            let mut bake = Bake {
                swatch: &swatch,
                palettes: Palettes::default(),
                materials: BTreeMap::new(),
                baked: BTreeMap::new(),
                tinted_triangles: 0,
            };
            for (proto, placements) in placed {
                bake.prototype(w, proto, &placements, log)?;
            }
            log.approximate(
                "swatch palette read at one texel, not filtered: triangles",
                bake.tinted_triangles,
            );
            log.count("palette texels read", bake.palettes.seen.len() as u64);
            log.count(
                "palette texels a clamped bilinear read would change by more than two stored values",
                bake.palettes.filtered_differs[0],
            );
            log.count(
                "palette texels a wrapped bilinear read would change by more than two stored values",
                bake.palettes.filtered_differs[1],
            );
        }
        for e in swatch.keys() {
            w.remove::<SwatchTint>(*e);
        }
        let meshes: Vec<Entity> = w.iter::<Mesh>().map(|(e, _)| e).collect();
        let mut dropped = 0u64;
        for e in meshes {
            let Some(mesh) = w.get_mut::<Mesh>(e) else {
                continue;
            };
            for i in 0..mesh.primitives().len() {
                if let Some(mut p) = mesh.primitive_mut(i)
                    && p.streams_mut().remove(&COLOUR).is_some()
                {
                    dropped += 1;
                }
            }
        }
        log.count("vertex colour streams dropped", dropped);
        Ok(())
    }
}
