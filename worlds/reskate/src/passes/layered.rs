//! A `core.layered` blend drawn as skate. can draw it: a base surface plus decal overlays whose per-vertex opacity
//! reproduces the blend.
//!
//! The core blend is a weighted sum, `sum(w_i * L_i)`, not normalised (`rgb_product_4`'s weights sum to `1 - r`, so
//! red darkens toward black). skate.'s only per-vertex blend is a decal's opacity (stock decal row, `Color0.r`), and
//! alpha-blended decals drawn in order compute a lerp chain. So:
//! - the base is slot 0, drawn opaque on every triangle;
//! - slot `i >= 1` is an overlay with opacity `a_i = w_i / (w_0 + ... + w_i)` (0/0 = 0), in slot order: the chain
//!   then gives `sum(w_i * L_i) / s` with `s = sum(w)`;
//! - where `s < 1` a last overlay, black, with opacity `1 - s` scales that by `s`, giving the core blend exactly;
//! - where `s > 1` (possible with `identity`) the weights are scaled to sum to 1 (counted).
//!
//! A layer's UV set and transform (core samples every texture of the layer at the transform of its selected set) go
//! into the coordinates of the parts that draw it: the base (slot 0) and each of its overlays (their own copies of
//! the primitive) get `TexCoord(0)` = the transform applied to the selected set, per vertex in f32, exact up to that
//! rounding because the transform is affine and so commutes with interpolation. The layer materials' own texture
//! transforms are raise-skate's (baked before it); a skate material samples `TexCoord(0)` untransformed.
//!
//! A null slot is a layer of black (core: it contributes black without redistributing its weight). An overlay keeps
//! only triangles with some corner whose opacity, as written ([`crate::decal::decal_opacity`]),
//! is above zero. The game's decal fades through a mask-shaped dissolve rather than linear alpha, and weights are
//! evaluated per vertex then interpolated (the core blend interpolates channels, then evaluates): both are named
//! approximations of the lowering pass, not of this arithmetic.
//!
//! The chain needs the overlays drawn in slot order, and the game does not draw a surface's overlapping decal
//! sections in their section order (measured: three coplanar overlays in all six section orders gave four
//! different results, matching neither section nor material-key order, the same from every view). Where two or
//! more overlays overlap, the mix can therefore differ from the blend; it stays smooth. The default,
//! [`OverlayLimit::All`], draws every layer. [`OverlayLimit::One`] gives a triangle at most one overlay instead: the
//! strongest other layer (by the summed corner weights of slots `>= 1`, and of the black remainder where the
//! weights sum below one), drawn over the base with opacity `w_k / (w_0 + w_k)` (0/0 = 0), the two-layer blend
//! renormalised, and counts the triangles that lose a layer. Its overlays never overlap, but neighbouring triangles
//! that keep different layers meet in hard edges (measured in game), so it is not the default. The writer draws the
//! default's overlays in chain order by giving each rank an object of its own ([`crate::blend_order`]).
//!
//! Runtime: the in-game results and gates of this lowering are on ReSkate v2.1.0-pre (the latest release, the
//! official runtime for it) with maps compiled by ReSkate Studio 20261012.

use primase_core::{Entity, Mesh, World};

pub use crate::components::layered::{BlendOpacity, VertexBlend};

use crate::decal::decal_opacity;

/// What an overlay draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OverlayLayer {
    /// The layer of this slot (`>= 1`; slot 0 is the base).
    Slot(usize),
    /// Black, for weight that sums below one.
    Darkening,
}

/// One overlay: its triangles (indices into the primitive's vertices) and an opacity per vertex.
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayPart {
    pub layer: OverlayLayer,
    pub indices: Vec<u32>,
    pub opacity: Vec<f32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OverlayCounts {
    /// Vertices whose weights summed above one and were scaled down.
    pub clamped_vertices: usize,
    /// Vertices whose weights summed below one (the darkening overlay draws there).
    pub darkening_vertices: usize,
    /// Triangles where more than one overlay draws ([`OverlayLimit::All`]: where their draw order matters) or would
    /// draw ([`OverlayLimit::One`]: only the strongest is kept).
    pub overlapping_triangles: usize,
    /// Their area, in the primitive's units squared.
    pub overlapping_area: f64,
}

/// How many overlays a triangle may get.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayLimit {
    /// Every layer, in slot order: the exact lerp chain where the game draws the overlays in that order (it does not
    /// always: see the module doc).
    All,
    /// At most one: the strongest other layer over the base, renormalised; order-free, but neighbouring triangles
    /// that keep different layers meet in hard edges.
    One,
}

/// Weight sums within this of one count as one (f32 rounding of the evaluated mapping, a few ulp).
const SUM_TOLERANCE: f64 = 1e-6;

/// Per-vertex opacities of overlays `1..slots` and the darkening overlay, from per-vertex slot weights (the
/// mapping already evaluated; only the first `slots` entries are used).
pub fn opacities(
    weights: &[[f32; 4]],
    slots: usize,
    counts: &mut OverlayCounts,
) -> (Vec<Vec<f32>>, Vec<f32>) {
    let n = weights.len();
    let mut over = vec![vec![0f32; n]; slots.saturating_sub(1)];
    let mut dark = vec![0f32; n];
    for (v, w) in weights.iter().enumerate() {
        let w: Vec<f64> = w[..slots].iter().map(|&x| f64::from(x.max(0.0))).collect();
        let mut s: f64 = w.iter().sum();
        // a sum within f32 rounding of one (rgb_product_4's terms sum to exactly 1 - r) is not an excess
        if s > 1.0 + SUM_TOLERANCE {
            counts.clamped_vertices += 1;
        }
        let scale = if s > 1.0 { 1.0 / s } else { 1.0 };
        let w: Vec<f64> = w.iter().map(|x| x * scale).collect();
        s = s.min(1.0);
        let mut prefix = w[0];
        for i in 1..slots {
            prefix += w[i];
            over[i - 1][v] = if prefix > 0.0 { (w[i] / prefix) as f32 } else { 0.0 };
        }
        if s < 1.0 - SUM_TOLERANCE {
            counts.darkening_vertices += 1;
            dark[v] = (1.0 - s) as f32;
        }
    }
    (over, dark)
}

/// The overlays of a triangle primitive: slot overlays in slot order, then the darkening overlay; each with the
/// triangles where some corner's written opacity is above zero (none: the overlay is left out). With
/// [`OverlayLimit::One`] each triangle keeps only its strongest overlay, with the two-layer opacities (see the
/// module doc); `positions` give the area of the triangles that lose one.
pub fn overlay_parts(
    weights: &[[f32; 4]],
    slots: usize,
    indices: &[u32],
    positions: &[[f32; 3]],
    limit: OverlayLimit,
    counts: &mut OverlayCounts,
) -> Vec<OverlayPart> {
    let (over, dark) = opacities(weights, slots, counts);
    let layers: Vec<(OverlayLayer, Vec<f32>)> = match limit {
        OverlayLimit::All => over
            .into_iter()
            .enumerate()
            .map(|(i, o)| (OverlayLayer::Slot(i + 1), o))
            .chain(std::iter::once((OverlayLayer::Darkening, dark.clone())))
            .collect(),
        OverlayLimit::One => pair_opacities(weights, slots, &dark),
    };
    let tris = indices.as_chunks::<3>().0;
    let mut keep = vec![vec![false; tris.len()]; layers.len()];
    for (t, tri) in tris.iter().enumerate() {
        let visible: Vec<usize> = (0..layers.len())
            .filter(|&k| tri.iter().any(|&i| decal_opacity(layers[k].1[i as usize])[0] > 0))
            .collect();
        match limit {
            OverlayLimit::All => visible.iter().for_each(|&k| keep[k][t] = true),
            OverlayLimit::One => {
                let strength = |k: usize| -> f64 {
                    tri.iter()
                        .map(|&i| f64::from(layer_weight(weights[i as usize], slots, &dark, i, layers[k].0)))
                        .sum()
                };
                // strongest first; ties to the earlier layer (lower slot, the darkening last)
                if let Some(&best) = visible
                    .iter()
                    .min_by(|&&x, &&y| strength(y).total_cmp(&strength(x)).then(x.cmp(&y)))
                {
                    keep[best][t] = true;
                }
            }
        }
        if visible.len() > 1 {
            counts.overlapping_triangles += 1;
            counts.overlapping_area += area(positions, tri);
        }
    }
    let mut out = Vec::new();
    for ((layer, opacity), keep) in layers.into_iter().zip(keep) {
        let indices: Vec<u32> = tris
            .iter()
            .zip(&keep)
            .filter(|(_, k)| **k)
            .flat_map(|(t, _)| *t)
            .collect();
        if !indices.is_empty() {
            out.push(OverlayPart {
                layer,
                indices,
                opacity,
            });
        }
    }
    out
}

/// The weight of `layer` at vertex `i`: a slot's weight scaled as in [`opacities`], the darkening's the black
/// remainder `1 - s` (its chain opacity, `dark[i]`).
fn layer_weight(w: [f32; 4], slots: usize, dark: &[f32], i: u32, layer: OverlayLayer) -> f32 {
    match layer {
        OverlayLayer::Slot(k) => {
            let s: f64 = w[..slots].iter().map(|&x| f64::from(x.max(0.0))).sum();
            let x = f64::from(w[k].max(0.0));
            (if s > 1.0 { x / s } else { x }) as f32
        }
        OverlayLayer::Darkening => dark[i as usize],
    }
}

/// Per-vertex two-layer opacities for [`OverlayLimit::One`]: each slot `k >= 1`, then the black remainder, over the
/// base alone: `w_k / (w_0 + w_k)` (0/0 = 0).
fn pair_opacities(weights: &[[f32; 4]], slots: usize, dark: &[f32]) -> Vec<(OverlayLayer, Vec<f32>)> {
    (1..slots)
        .map(OverlayLayer::Slot)
        .chain(std::iter::once(OverlayLayer::Darkening))
        .map(|layer| {
            let o = (0..weights.len())
                .map(|v| {
                    let i = v as u32;
                    let base = f64::from(layer_weight(weights[v], slots, dark, i, OverlayLayer::Slot(0)));
                    let own = f64::from(layer_weight(weights[v], slots, dark, i, layer));
                    if base + own > 0.0 {
                        (own / (base + own)) as f32
                    } else {
                        0.0
                    }
                })
                .collect();
            (layer, o)
        })
        .collect()
}

/// A triangle's area.
fn area(p: &[[f32; 3]], t: &[u32; 3]) -> f64 {
    let [a, b, c] = t.map(|i| p[i as usize].map(f64::from));
    let (u, v) = (
        [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
        [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
    );
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    0.5 * (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt()
}

fn weights_of(
    p: &primase_core::Primitive,
    l: &primase_core::LayeredMaterial,
) -> Result<Vec<[f32; 4]>, String> {
    use primase_core::Stream;
    let values: Vec<[f32; 4]> = match p.streams.get(&l.weights.attribute) {
        Some(Stream::F32x3(v)) => v.iter().map(|&[r, g, b]| [r, g, b, 1.]).collect(),
        Some(Stream::F32x4(v)) => v.clone(),
        Some(Stream::U8x4(v)) => v.iter().map(|v| v.map(|x| f32::from(x) / 255.)).collect(),
        Some(Stream::U16x4(v)) => v.iter().map(|v| v.map(|x| f32::from(x) / 65535.)).collect(),
        Some(_) => return Err("layer weights need a VEC3/VEC4 float or normalized unsigned stream".into()),
        None => return Err("a layered primitive has no weight stream".into()),
    };
    if values.len() != p.vertex_count() || !values.iter().flatten().all(|v| v.is_finite()) {
        return Err("invalid layered weight count or non-finite value".into());
    }
    Ok(values
        .into_iter()
        .map(|c| l.weights.mapping.evaluate(c))
        .collect())
}

/// `p` drawn with `layer`'s coordinates: `TexCoord(0)` the layer's transform of its selected set (a float pair
/// stream; another or none is an error). True where that changes them.
fn with_layer_uvs(p: &mut primase_core::Primitive, layer: &primase_core::Layer) -> Result<bool, String> {
    use primase_core::{Semantic, Stream};
    let set = layer.texcoord.unwrap_or(0);
    if set == 0 && layer.transform.is_none() {
        return Ok(false);
    }
    let uvs = match p.streams.get(&Semantic::TexCoord(set)) {
        Some(Stream::F32x2(v)) => v,
        Some(_) => {
            return Err(format!(
                "a layer's coordinates TexCoord({set}) are not float pairs"
            ));
        }
        None => {
            return Err(format!(
                "a layer samples TexCoord({set}), which its primitive lacks"
            ));
        }
    };
    let moved = uvs
        .iter()
        .map(|&uv| layer.transform.map_or(uv, |t| t.apply(uv)))
        .collect();
    p.streams.insert(Semantic::TexCoord(0), Stream::F32x2(moved));
    Ok(true)
}

/// A black material: a null slot's layer, and the darkening overlay.
fn black(key: String, class: crate::components::MaterialClass) -> crate::components::SkateMaterial {
    use crate::components::{AlphaMode, Domain, MaterialClass, SkateMaterial};
    let decal = class == MaterialClass::Decal;
    SkateMaterial {
        key,
        class,
        base: None,
        normal: None,
        mask: None,
        base_color: Some([0, 0, 0, 255]),
        tint: None,
        alpha: decal.then_some(AlphaMode::Blend),
        cutoff: None,
        two_sided: None,
        domain: decal.then_some(Domain::Decal),
        metallic: None,
        roughness: None,
        wrap: None,
        ocean: None,
        cast_shadows: None,
        emissive: None,
        vertex_alpha: None,
        other: Default::default(),
    }
}

/// One primitive of a lowered mesh: (source primitive, primitive, opacity, (layered material, overlay layer or the
/// base)).
type PlannedPart = (
    usize,
    primase_core::Primitive,
    Option<Vec<f32>>,
    Option<(Entity, Option<OverlayLayer>)>,
);

/// `lower-layered-reskate`: every `core.layered` primitive becomes its base layer (slot 0) and decal overlays
/// ([`overlay_parts`]) drawn right after it, each overlay's material a decal copy of its layer's `skate.material`
/// (black for a null slot and for the darkening overlay) marked [`VertexBlend`], its opacities in
/// [`BlendOpacity`]. The layered materials themselves are removed. The ReSkate writer emits the overlays with
/// Studio's `vertex_blend`, and the post-compile stage moves them onto the stock decal row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LowerLayeredReskate {
    overlays: OverlayLimit,
}

impl LowerLayeredReskate {
    /// With at most `overlays` overlays a triangle.
    pub fn new(overlays: OverlayLimit) -> LowerLayeredReskate {
        LowerLayeredReskate { overlays }
    }

    pub fn overlays(&self) -> OverlayLimit {
        self.overlays
    }

    /// From its options: `overlays`, required: `all` or `one`.
    pub fn from_options(o: &primase_core::pass::Options) -> Result<LowerLayeredReskate, String> {
        if let Some(k) = o.keys().find(|k| k.as_str() != "overlays") {
            return Err(format!("unknown option {k}"));
        }
        match o.get("overlays").map(String::as_str) {
            Some("all") => Ok(LowerLayeredReskate::new(OverlayLimit::All)),
            Some("one") => Ok(LowerLayeredReskate::new(OverlayLimit::One)),
            Some(v) => Err(format!("overlays={v} is not all or one")),
            None => Err("overlays is required: all (every layer, in slot order) or one".into()),
        }
    }
}

/// `lower-layered-reskate{overlays=all|one}` from its options (a [`primase_core::pass::PassFactory`]).
pub fn lower_layered_reskate(
    o: &primase_core::pass::Options,
) -> Result<Box<dyn primase_core::pass::Pass>, String> {
    Ok(Box::new(LowerLayeredReskate::from_options(o)?))
}

impl primase_core::pass::Pass for LowerLayeredReskate {
    fn name(&self) -> &'static str {
        "lower-layered-reskate"
    }
    fn options(&self) -> primase_core::pass::Options {
        let v = match self.overlays {
            OverlayLimit::All => "all",
            OverlayLimit::One => "one",
        };
        primase_core::pass::Options::from([("overlays".to_owned(), v.to_owned())])
    }
    // the blends are optional: a map without them has nothing to lower
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.material", "reskate.vertex_blend", "reskate.blend_opacity"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh", "core.material_variants", "skate.prototype"]
    }
    /// The layered form, and from the layered materials and meshes it folds: their raised `skate.material` and the
    /// mesh file the folded mesh no longer is.
    fn removes(&self) -> &[&'static str] {
        &["core.layered", "skate.material", "core.mesh_file"]
    }
    fn approximates(&self) -> &'static str {
        match self.overlays {
            OverlayLimit::One => {
                "at most one overlay per triangle: the strongest other layer over the base, renormalised (triangles and area that lose a layer are counted; neighbouring triangles that keep different layers meet in hard edges); decal overlays fade through the game's mask-shaped dissolve, not linear alpha; layer weights are evaluated per vertex and interpolated (the blend interpolates channels, then evaluates)"
            }
            OverlayLimit::All => {
                "where overlays overlap the mix needs them drawn in slot order, which rests on the writer's per-rank objects and the game's measured tie order (past the writer's object cap the order is unstable and the mix there can differ from the blend); decal overlays fade through the game's mask-shaped dissolve, not linear alpha; layer weights are evaluated per vertex and interpolated (the blend interpolates channels, then evaluates)"
            }
        }
    }
    fn run(&self, world: &mut World, log: &mut primase_core::pass::PassLog) -> Result<(), String> {
        use crate::components::{AlphaMode, Domain, MaterialClass, SkateMaterial};
        use primase_core::{LayeredMaterial, MaterialVariants, MeshFile};
        use std::collections::{BTreeMap, BTreeSet};
        let mut layered: BTreeMap<Entity, LayeredMaterial> = world
            .iter::<LayeredMaterial>()
            .map(|(e, l)| (e, l.clone()))
            .collect();
        // a blend no placed primitive draws: raise-skate leaves it unraised (no skate.material, counted there), and so
        // does this pass, its core.layered dropped (counted); one a placed primitive draws must have been raised
        let mut drawn: BTreeSet<Entity> = BTreeSet::new();
        for p in world
            .iter::<primase_core::Instance>()
            .map(|(_, i)| i.prototype)
            .collect::<BTreeSet<_>>()
        {
            if let Some(mesh) = world.get::<Mesh>(p) {
                drawn.extend(mesh.primitives().iter().filter_map(|q| q.material));
            }
            if let Some(v) = world.get::<MaterialVariants>(p) {
                drawn.extend(v.0.iter().flat_map(|o| o.values().copied()));
            }
        }
        let undrawn: Vec<Entity> = layered
            .keys()
            .filter(|e| !world.has::<SkateMaterial>(**e) && !drawn.contains(e))
            .copied()
            .collect();
        for e in &undrawn {
            layered.remove(e);
        }
        if layered.is_empty() {
            for e in &undrawn {
                world.remove::<LayeredMaterial>(*e);
            }
            if !undrawn.is_empty() {
                log.count(
                    "blends no placed primitive draws (not raised), left out",
                    undrawn.len() as u64,
                );
            }
            return Ok(());
        }
        if let Some(e) = layered
            .iter()
            .find(|(_, l)| l.weights.per_pixel())
            .map(|(e, _)| e)
        {
            return Err(format!(
                "{e:?}: per-pixel weight channels or the over composition: skate blends by vertex weights \
                 (bake-layer-channels first)"
            ));
        }
        for (e, f) in world.iter::<MeshFile>() {
            if !world.has::<Mesh>(e) && f.materials.iter().flatten().any(|m| layered.contains_key(m)) {
                return Err("load layered meshes into core.mesh before lower-layered-reskate".into());
            }
        }
        for (e, l) in &layered {
            if !world.has::<SkateMaterial>(*e) {
                return Err(format!(
                    "layered material {e:?} has no skate.material: its overlays need its key"
                ));
            }
            for layer in l.layers.iter().flatten() {
                if !world.has::<SkateMaterial>(layer.material) {
                    return Err(format!("layered material {e:?}: a layer is not a skate.material"));
                }
            }
        }
        // ---- plan every primitive before changing the world: nothing after planning can fail, so a refusal changes
        // nothing (the world and the log)
        let mut plans: Vec<(Entity, Vec<PlannedPart>)> = Vec::new();
        let mut counts = OverlayCounts::default();
        let mut tally: BTreeMap<&'static str, u64> = BTreeMap::new();
        let (mut base_area, mut overlay_area) = (0f64, 0f64);
        for (e, mesh) in world.iter::<Mesh>() {
            let variants = world.get::<MaterialVariants>(e);
            if !mesh
                .primitives()
                .iter()
                .any(|p| p.material.is_some_and(|m| layered.contains_key(&m)))
            {
                continue;
            }
            let mut parts: Vec<PlannedPart> = Vec::new();
            for (i, p) in mesh.primitives().iter().enumerate() {
                let Some((m, l)) = p.material.and_then(|m| layered.get(&m).map(|l| (m, l))) else {
                    parts.push((i, p.clone(), None, None));
                    continue;
                };
                if variants.is_some_and(|v| v.0.get(i).is_some_and(|x| !x.is_empty())) {
                    return Err("lower-layered-reskate refuses variants on a layered part".into());
                }
                if p.topology != primase_core::Topology::Triangles {
                    return Err("a layered primitive is not triangles".into());
                }
                let w = weights_of(p, l)?;
                let positions = match p.streams.get(&primase_core::Semantic::Position) {
                    Some(primase_core::Stream::F32x3(v)) => v.clone(),
                    _ => return Err("a layered primitive has no float3 positions".into()),
                };
                let idx = p
                    .indices
                    .clone()
                    .unwrap_or_else(|| (0..p.vertex_count() as u32).collect());
                let mut base = p.clone();
                base.indices = Some(idx.clone());
                if let Some(layer) = &l.layers[0]
                    && with_layer_uvs(&mut base, layer)?
                {
                    *tally
                        .entry("parts drawn at their layer's coordinates")
                        .or_default() += 1;
                }
                parts.push((i, base, None, Some((m, None))));
                *tally.entry("layered base triangles").or_default() += (idx.len() / 3) as u64;
                base_area += idx
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|t| area(&positions, t))
                    .sum::<f64>();
                for o in overlay_parts(&w, l.layers.len(), &idx, &positions, self.overlays, &mut counts) {
                    let mut q = p.clone();
                    q.indices = Some(o.indices.clone());
                    if let OverlayLayer::Slot(k) = o.layer
                        && let Some(layer) = &l.layers[k]
                        && with_layer_uvs(&mut q, layer)?
                    {
                        *tally
                            .entry("parts drawn at their layer's coordinates")
                            .or_default() += 1;
                    }
                    let n = (o.indices.len() / 3) as u64;
                    overlay_area += o
                        .indices
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .map(|t| area(&positions, t))
                        .sum::<f64>();
                    *tally.entry("overlay primitives").or_default() += 1;
                    *tally.entry("overlay triangles").or_default() += n;
                    if o.layer == OverlayLayer::Darkening {
                        *tally.entry("darkening overlay triangles").or_default() += n;
                    }
                    parts.push((i, q, Some(o.opacity), Some((m, Some(o.layer)))));
                }
            }
            plans.push((e, parts));
        }
        // ---- every per-primitive component of the lowered meshes follows its source primitive (World::
        // remap_primitives): tried on a copy of those meshes first, so a refusal still changes nothing
        let remaps: Vec<(Entity, Vec<usize>)> = plans
            .iter()
            .map(|(e, parts)| (*e, parts.iter().map(|(src, ..)| *src).collect()))
            .collect();
        let mut probe = world.subset(&remaps.iter().map(|(e, _)| *e).collect());
        for (e, from) in &remaps {
            probe
                .remap_primitives(*e, from)
                .map_err(|m| format!("{e:?}: {m}"))?;
        }
        drop(probe);
        for (name, n) in tally {
            log.count(name, n);
        }
        log.count(
            "vertices with weights above one (scaled)",
            counts.clamped_vertices as u64,
        );
        log.count(
            "vertices with weights below one (darkened)",
            counts.darkening_vertices as u64,
        );
        log.count("layered base area (m2)", base_area.round() as u64);
        log.count(
            "overlay area (m2, summed over overlays)",
            overlay_area.round() as u64,
        );
        let (tris, area) = match self.overlays {
            OverlayLimit::All => (
                "triangles with overlapping overlays",
                "area with overlapping overlays (m2)",
            ),
            OverlayLimit::One => (
                "triangles limited to one overlay",
                "area limited to one overlay (m2)",
            ),
        };
        log.count(tris, counts.overlapping_triangles as u64);
        log.count(area, counts.overlapping_area.round() as u64);
        // ---- materials: base = slot 0's material (black when null), overlays = decal copies
        let mut made: BTreeMap<(Entity, Option<OverlayLayer>), Entity> = BTreeMap::new();
        for (_, parts) in &plans {
            for (_, _, _, what) in parts {
                let Some((m, layer)) = *what else { continue };
                if made.contains_key(&(m, layer)) {
                    continue;
                }
                let l = &layered[&m];
                let key = world
                    .get::<SkateMaterial>(m)
                    .map(|s| s.key.clone())
                    .unwrap_or_default();
                let entity = match layer {
                    None => match &l.layers[0] {
                        Some(layer0) => layer0.material,
                        None => {
                            let e = world.spawn();
                            world.insert(e, black(format!("{key}~base"), MaterialClass::Opaque));
                            e
                        }
                    },
                    Some(o) => {
                        let (src, tag, mark) = match o {
                            OverlayLayer::Slot(k) => (
                                l.layers[k].as_ref().map(|x| x.material),
                                format!("~overlay{k}"),
                                VertexBlend::Overlay { slot: k as u8 },
                            ),
                            OverlayLayer::Darkening => {
                                (None, "~darkening".to_owned(), VertexBlend::Darkening)
                            }
                        };
                        let mut s = match src.and_then(|s| world.get::<SkateMaterial>(s).cloned()) {
                            Some(s) => s,
                            None => black(String::new(), MaterialClass::Decal),
                        };
                        s.key = format!("{key}{tag}");
                        s.class = MaterialClass::Decal;
                        s.domain = Some(Domain::Decal);
                        s.alpha = Some(AlphaMode::Blend);
                        let e = world.spawn();
                        world.insert(e, s);
                        world.insert(e, mark);
                        e
                    }
                };
                made.insert((m, layer), entity);
            }
        }
        // ---- apply
        for ((entity, parts), (_, from)) in plans.into_iter().zip(remaps) {
            let mut primitives = Vec::new();
            let mut opacity = Vec::new();
            for (_, mut prim, op, what) in parts {
                if let Some(key) = what {
                    prim.material = Some(made[&key]);
                }
                primitives.push(prim);
                opacity.push(op);
            }
            world
                .rebuild_primitives(entity, |_| Ok((primitives, from)))
                .expect("the same remap succeeded on the copy");
            world.remove::<MeshFile>(entity);
            if opacity.iter().any(Option::is_some) {
                world.insert(entity, BlendOpacity(opacity));
            }
        }
        for m in layered.keys() {
            world.remove::<LayeredMaterial>(*m);
            world.remove::<SkateMaterial>(*m);
        }
        for e in &undrawn {
            world.remove::<LayeredMaterial>(*e);
        }
        if !undrawn.is_empty() {
            log.count(
                "blends no placed primitive draws (not raised), left out",
                undrawn.len() as u64,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::{Component, WeightMapping};

    /// The opacity bytes written at one vertex: the slot overlays' red bytes, then the darkening overlay's.
    fn bytes(weights: [f32; 4], slots: usize, counts: &mut OverlayCounts) -> (Vec<u8>, u8) {
        let (over, dark) = opacities(&[weights], slots, counts);
        (
            over.iter().map(|o| decal_opacity(o[0])[0]).collect(),
            decal_opacity(dark[0])[0],
        )
    }

    /// The game's draw at one vertex: the base, each overlay lerped over it at its byte opacity in slot order, then the
    /// black darkening overlay.
    fn draw(layers: &[f64], over: &[u8], dark: u8) -> f64 {
        let q = |b: u8| f64::from(b) / 255.0;
        let mut c = layers[0];
        for (l, &b) in layers[1..].iter().zip(over) {
            c = (1.0 - q(b)) * c + q(b) * l;
        }
        c * (1.0 - q(dark))
    }

    fn lcg(seed: &mut u64) -> f64 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (*seed >> 11) as f64 / (1u64 << 53) as f64
    }

    #[test]
    fn an_overlay_slot_is_a_layer_above_the_base() {
        let mut w = World::new();
        let e = w.spawn();
        for slot in 1..=3 {
            assert_eq!(VertexBlend::Overlay { slot }.verify(e, &w), Ok(()));
            assert_eq!(VertexBlend::Overlay { slot }.chain_rank(), Some(slot));
        }
        for slot in [0, 4, 255] {
            assert!(
                VertexBlend::Overlay { slot }.verify(e, &w).is_err(),
                "slot {slot}"
            );
        }
        assert_eq!(VertexBlend::Darkening.chain_rank(), Some(4));
        assert_eq!(VertexBlend::VertexAlpha.chain_rank(), None);
    }

    /// An internal consistency bound, not an external oracle: over random weights (normalised, and rgb_product_4's,
    /// which sum below one) the bytes drawn in order stay within half a byte step per written opacity of the core
    /// blend `sum(w_i * L_i) / max(1, sum(w_i))` (`WeightMapping::evaluate`'s weights). The hand-worked tests below
    /// carry the contract.
    #[test]
    fn quantization_stays_within_half_a_byte_per_overlay_of_the_core_blend() {
        let mut seed = 7u64;
        let mut cases: Vec<([f32; 4], usize)> = Vec::new();
        for slots in 2..=4 {
            for _ in 0..2000 {
                let raw: Vec<f64> = (0..slots).map(|_| lcg(&mut seed)).collect();
                let total: f64 = raw.iter().sum();
                let mut w = [0f32; 4];
                for i in 0..slots {
                    w[i] = (raw[i] / total) as f32;
                }
                cases.push((w, slots));
            }
        }
        for _ in 0..2000 {
            let ch = [
                lcg(&mut seed) as f32,
                lcg(&mut seed) as f32,
                lcg(&mut seed) as f32,
                1.0,
            ];
            cases.push((WeightMapping::RgbProduct4.evaluate(ch), 4));
        }
        for (w, slots) in cases {
            let layers: Vec<f64> = (0..slots).map(|_| lcg(&mut seed)).collect();
            let (over, dark) = bytes(w, slots, &mut OverlayCounts::default());
            let got = draw(&layers, &over, dark);
            let ws: Vec<f64> = w[..slots].iter().map(|&x| f64::from(x)).collect();
            let norm = ws.iter().sum::<f64>().max(1.0);
            let want: f64 = ws.iter().zip(&layers).map(|(w, l)| w * l / norm).sum();
            let bound = (slots + 1) as f64 * 0.5 / 255.0 + 1e-9;
            assert!(
                (got - want).abs() <= bound,
                "{slots} slots {w:?}: {got} vs {want}"
            );
        }
    }

    /// Weights 0.5, 0.3, 0.2 by hand: slot 1's opacity 0.3 / 0.8 = 0.375 (x 255 = 95.6: byte 96), slot 2's 0.2 / 1.0
    /// (51), no darkening. Drawn over layers 0.9, 0.1, 0.5: 0.9 -> 0.5988 -> 0.5791 (the exact blend is 0.58).
    #[test]
    fn three_layers_write_hand_worked_bytes() {
        let mut c = OverlayCounts::default();
        let (over, dark) = bytes([0.5, 0.3, 0.2, 0.0], 3, &mut c);
        assert_eq!((over.as_slice(), dark), (&[96u8, 51][..], 0));
        assert_eq!((c.clamped_vertices, c.darkening_vertices), (0, 0));
        assert!((draw(&[0.9, 0.1, 0.5], &over, dark) - 0.579_059).abs() < 1e-6);
    }

    #[test]
    fn strongest_layer_not_layer_zero() {
        // the base is slot 0 even though slot 1 is strongest: slot 1's opacity is w1 / (w0 + w1) = 0.7 / 0.8 = 0.875
        // (x 255 = 223.1: byte 223), slot 2's 0.2 (51). Over layers 0.9, 0.1, 0.5: 0.9 -> 0.2004 -> 0.2603 (exact 0.26)
        let (over, dark) = bytes([0.1, 0.7, 0.2, 0.0], 3, &mut OverlayCounts::default());
        assert_eq!((over.as_slice(), dark), (&[223u8, 51][..], 0));
        assert!((draw(&[0.9, 0.1, 0.5], &over, dark) - 0.260_314).abs() < 1e-6);
    }

    /// rgb_product_4 at (r, g, b) = (0.5, 0, 0.5): weights 0.25, 0.25, 0, 0, summing to 0.5. Slot 1's opacity 0.25 /
    /// 0.5 (x 255 = 127.5: byte 128), slots 2 and 3 none, the darkening 1 - 0.5 (128). Over layers 0.8, 0.4, -, -:
    /// 0.8 -> 0.5992, darkened by 127/255 -> 0.2984 (exact 0.3).
    #[test]
    fn rgb_product_4_darkening_writes_hand_worked_bytes() {
        let w = WeightMapping::RgbProduct4.evaluate([0.5, 0.0, 0.5, 1.0]);
        assert_eq!(w, [0.25, 0.25, 0.0, 0.0]);
        let mut c = OverlayCounts::default();
        let (over, dark) = bytes(w, 4, &mut c);
        assert_eq!((over.as_slice(), dark), (&[128u8, 0, 0][..], 128));
        assert_eq!(c.darkening_vertices, 1);
        assert!((draw(&[0.8, 0.4, 0.0, 0.0], &over, dark) - 0.298_433).abs() < 1e-6);
    }

    /// Weights 0.8 and 0.6 sum to 1.4: scaled by 1 / 1.4 and counted; slot 1's opacity 0.6 / 1.4 (x 255 = 109.3: byte
    /// 109). Over layers 0.2, 1.0: 0.5420 (exact (0.16 + 0.6) / 1.4 = 0.5429).
    #[test]
    fn weights_above_one_are_scaled_and_counted() {
        let mut c = OverlayCounts::default();
        let (over, dark) = bytes([0.8, 0.6, 0.0, 0.0], 2, &mut c);
        assert_eq!((over.as_slice(), dark), (&[109u8][..], 0));
        assert_eq!((c.clamped_vertices, c.darkening_vertices), (1, 0));
        assert!((draw(&[0.2, 1.0], &over, dark) - 0.541_961).abs() < 1e-6);
    }

    #[test]
    fn all_zero_weights_are_black_and_zero_over_zero_is_zero() {
        let mut c = OverlayCounts::default();
        let (over, dark) = bytes([0.0; 4], 3, &mut c);
        assert_eq!((over.as_slice(), dark), (&[0u8, 0][..], 255));
        assert_eq!(draw(&[0.7, 0.4, 0.9], &over, dark), 0.0);
    }

    #[test]
    fn overlays_keep_only_triangles_with_a_visible_corner() {
        // four vertices, two triangles; slot 1 is visible only at vertex 3
        let w = [
            [1.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.5, 0.5, 0.0, 0.0],
        ];
        let mut c = OverlayCounts::default();
        let pos = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]];
        let parts = overlay_parts(&w, 2, &[0, 1, 2, 1, 2, 3], &pos, OverlayLimit::All, &mut c);
        assert_eq!(parts.len(), 1);
        assert_eq!(
            (parts[0].layer, parts[0].indices.clone()),
            (OverlayLayer::Slot(1), vec![1, 2, 3])
        );
    }

    const TRI: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];

    #[test]
    fn one_overlay_per_triangle_keeps_the_strongest_renormalised_and_counts() {
        let w = [[0.5, 0.3, 0.2, 0.0]; 3];
        let mut c = OverlayCounts::default();
        let parts = overlay_parts(&w, 3, &[0, 1, 2], &TRI, OverlayLimit::One, &mut c);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].layer, OverlayLayer::Slot(1));
        assert!(parts[0].opacity.iter().all(|&a| (a - 0.3 / 0.8).abs() < 1e-6));
        assert_eq!(c.overlapping_triangles, 1);
        assert!((c.overlapping_area - 0.5).abs() < 1e-9);
        // the same triangle with every layer: two overlays drawn over each other, counted as overlapping
        let mut c = OverlayCounts::default();
        assert_eq!(
            overlay_parts(&w, 3, &[0, 1, 2], &TRI, OverlayLimit::All, &mut c).len(),
            2
        );
        assert_eq!(c.overlapping_triangles, 1);
    }

    #[test]
    fn overlays_of_neighbouring_triangles_may_differ_but_never_overlap() {
        // slot 1 strongest on the first triangle, slot 2 on the second
        let w = [
            [0.5, 0.4, 0.1, 0.0],
            [0.5, 0.25, 0.25, 0.0],
            [0.5, 0.25, 0.25, 0.0],
            [0.5, 0.1, 0.4, 0.0],
        ];
        let pos = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]];
        let mut c = OverlayCounts::default();
        let parts = overlay_parts(&w, 3, &[0, 1, 2, 1, 2, 3], &pos, OverlayLimit::One, &mut c);
        let got: Vec<(OverlayLayer, Vec<u32>)> = parts.iter().map(|p| (p.layer, p.indices.clone())).collect();
        assert_eq!(
            got,
            [
                (OverlayLayer::Slot(1), vec![0, 1, 2]),
                (OverlayLayer::Slot(2), vec![1, 2, 3])
            ]
        );
        assert_eq!(c.overlapping_triangles, 2);
    }

    #[test]
    fn two_layer_surfaces_are_the_same_with_one_or_all() {
        let mut seed = 3u64;
        for _ in 0..500 {
            let w: Vec<[f32; 4]> = (0..3)
                .map(|_| {
                    let a = lcg(&mut seed) as f32;
                    [a, 1.0 - a, 0.0, 0.0]
                })
                .collect();
            let (mut c1, mut c2) = (OverlayCounts::default(), OverlayCounts::default());
            let one = overlay_parts(&w, 2, &[0, 1, 2], &TRI, OverlayLimit::One, &mut c1);
            let all = overlay_parts(&w, 2, &[0, 1, 2], &TRI, OverlayLimit::All, &mut c2);
            assert_eq!(one.len(), all.len());
            for (a, b) in one.iter().zip(&all) {
                assert_eq!((a.layer, &a.indices), (b.layer, &b.indices));
                for (x, y) in a.opacity.iter().zip(&b.opacity) {
                    assert!((x - y).abs() < 1e-6, "{x} vs {y}");
                }
            }
            assert_eq!(c1.overlapping_triangles, 0);
        }
    }

    #[test]
    fn the_black_remainder_competes_as_a_layer() {
        // weights sum to 0.4: black (0.6) is the strongest other layer, over the base 0.3 -> 0.6 / 0.9
        let w = [[0.3, 0.1, 0.0, 0.0]; 3];
        let mut c = OverlayCounts::default();
        let parts = overlay_parts(&w, 2, &[0, 1, 2], &TRI, OverlayLimit::One, &mut c);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].layer, OverlayLayer::Darkening);
        assert!((parts[0].opacity[0] - 0.6 / 0.9).abs() < 1e-6);
        assert_eq!((c.overlapping_triangles, c.darkening_vertices), (1, 3));
    }

    #[test]
    fn options_name_the_overlay_limit_and_round_trip() {
        use primase_core::pass::Options;
        // the limit is required and always named
        assert!(lower_layered_reskate(&Options::new()).is_err());
        for limit in [OverlayLimit::All, OverlayLimit::One] {
            let o = LowerLayeredReskate::new(limit).options();
            assert_eq!(lower_layered_reskate(&o).unwrap().options(), o);
        }
        let one = Options::from([("overlays".to_owned(), "one".to_owned())]);
        assert_eq!(LowerLayeredReskate::new(OverlayLimit::One).options(), one);
        let all = Options::from([("overlays".to_owned(), "all".to_owned())]);
        assert_eq!(LowerLayeredReskate::new(OverlayLimit::All).options(), all);
        let bad = Options::from([("overlays".to_owned(), "two".to_owned())]);
        assert!(
            lower_layered_reskate(&bad)
                .err()
                .unwrap()
                .contains("not all or one")
        );
        let unknown = Options::from([("limit".to_owned(), "one".to_owned())]);
        assert!(
            lower_layered_reskate(&unknown)
                .err()
                .unwrap()
                .contains("unknown option limit")
        );
    }

    use crate::components::{MaterialClass, SkateMaterial, SkateParts, SkatePrototype};
    use primase_core::pass::{Pass, PassLog};
    use primase_core::{Layer, LayerWeights, LayeredMaterial, Primitive, Semantic, Stream, Topology};
    use std::collections::BTreeMap;

    /// A two-layer material on one triangle whose last vertex is half layer 1; the layered entity is a
    /// skate.material when `keyed`, and the mesh has a prototype with `parts` parts.
    fn layered_world(keyed: bool, parts: Option<usize>) -> (World, Entity, Entity) {
        let mut world = World::new();
        let layer = |w: &mut World, key: &str| {
            let e = w.spawn();
            w.insert(e, black(key.into(), MaterialClass::Opaque));
            Some(Layer {
                material: e,
                texcoord: None,
                transform: None,
            })
        };
        let layers = vec![layer(&mut world, "a"), layer(&mut world, "b")];
        let m = world.spawn();
        world.insert(
            m,
            LayeredMaterial {
                layers,
                weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
            },
        );
        if keyed {
            world.insert(m, black("blend".into(), MaterialClass::Opaque));
        }
        let mesh = world.spawn();
        world.insert(
            mesh,
            Mesh::new(vec![Primitive {
                topology: Topology::Triangles,
                material: Some(m),
                indices: None,
                streams: BTreeMap::from([
                    (
                        Semantic::Position,
                        Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]]),
                    ),
                    (
                        Semantic::Color(1),
                        Stream::F32x4(vec![[1., 0., 0., 0.], [1., 0., 0., 0.], [0.5, 0.5, 0., 0.]]),
                    ),
                ]),
            }]),
        );
        if let Some(n) = parts {
            world.insert(
                mesh,
                SkatePrototype {
                    id: "p".into(),
                    glb: None,
                    key: None,
                    foliage: None,
                    ground: None,
                    collision: None,
                    other: Default::default(),
                },
            );
            world.insert(mesh, SkateParts(vec![Default::default(); n]));
        }
        (world, m, mesh)
    }

    fn refused_unchanged(mut world: World, want: &str) {
        let before = format!("{world:?}");
        let mut log = PassLog::default();
        let err = LowerLayeredReskate::new(OverlayLimit::All)
            .run(&mut world, &mut log)
            .unwrap_err();
        assert!(err.contains(want), "{err}");
        assert_eq!(format!("{world:?}"), before);
        assert!(log.counts.is_empty(), "{:?}", log.counts);
    }

    #[test]
    fn part_metadata_mismatch_refuses_all_changes() {
        refused_unchanged(layered_world(true, Some(0)).0, "skate.parts");
    }

    /// A reader dialect's per-primitive data, as it types it.
    #[derive(Clone, Debug, PartialEq)]
    struct Sources(Vec<&'static str>);
    impl primase_core::Component for Sources {
        const NAME: &'static str = "test.sources";
        const GEOMETRY: primase_core::Geometry = primase_core::Geometry::PerPrimitive;
        fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
            Some(primase_core::remap_list(&self.0, from).map(Sources))
        }
    }

    #[test]
    fn a_dialects_per_primitive_data_follows_the_overlays() {
        let (mut world, _, mesh) = layered_world(true, Some(1));
        world.insert(mesh, Sources(vec!["terrain"]));
        LowerLayeredReskate::new(OverlayLimit::All)
            .run(&mut world, &mut PassLog::default())
            .unwrap();
        // the base and its overlay both come from the source primitive
        assert_eq!(
            world.get::<Sources>(mesh),
            Some(&Sources(vec!["terrain", "terrain"]))
        );
        // one entry short: refused, nothing changed
        let (mut world, _, mesh) = layered_world(true, Some(1));
        world.insert(mesh, Sources(Vec::new()));
        refused_unchanged(world, "test.sources");
    }

    /// A blend a placed primitive draws must have been raised (its overlays need its key): refused, nothing changed.
    #[test]
    fn unkeyed_layered_material_refuses_all_changes() {
        let (mut world, _, mesh) = layered_world(false, Some(1));
        let at = world.spawn();
        world.insert(
            at,
            primase_core::Instance {
                prototype: mesh,
                variant: 0,
            },
        );
        refused_unchanged(world, "no skate.material");
    }

    /// A blend no placed primitive draws (raise-skate left it unraised) is left out: its core.layered dropped,
    /// counted, the unplaced mesh that draws it untouched.
    #[test]
    fn a_blend_no_placed_primitive_draws_is_left_out() {
        let (mut world, m, mesh) = layered_world(false, Some(1));
        let before = world.get::<Mesh>(mesh).cloned();
        let mut log = PassLog::default();
        LowerLayeredReskate::new(OverlayLimit::All)
            .run(&mut world, &mut log)
            .unwrap();
        assert!(!world.has::<LayeredMaterial>(m));
        assert_eq!(world.get::<Mesh>(mesh).cloned(), before);
        assert_eq!(
            log.counts["blends no placed primitive draws (not raised), left out"],
            1
        );
    }

    /// Each part draws its layer at the layer's coordinates: the base slot 0's transform of TexCoord(0), the overlay slot
    /// 1's transform of its selected set TexCoord(1), worked by hand; a selected set the primitive lacks is refused,
    /// nothing changed.
    #[test]
    fn each_part_draws_its_layer_at_the_layers_coordinates() {
        use primase_core::pbr::UvTransform;
        let (mut world, m, mesh) = layered_world(true, Some(1));
        {
            let l = world.get_mut::<LayeredMaterial>(m).unwrap();
            let base = l.layers[0].as_mut().unwrap();
            base.transform = Some(UvTransform {
                offset: [0.5, 0.0],
                scale: [2.0, 4.0],
                rotation: 0.0,
            });
            let over = l.layers[1].as_mut().unwrap();
            over.texcoord = Some(1);
            over.transform = Some(UvTransform {
                offset: [0.0, 1.0],
                scale: [3.0, 3.0],
                rotation: 0.0,
            });
        }
        // the triangle with the given coordinate sets
        let with = |world: &mut World, sets: Vec<(u8, Vec<[f32; 2]>)>| {
            let mut p = world.get::<Mesh>(mesh).unwrap().primitives()[0].clone();
            for (n, v) in sets {
                p.streams.insert(Semantic::TexCoord(n), Stream::F32x2(v));
            }
            world.insert(mesh, Mesh::new(vec![p]));
        };
        let mut refused = world.clone();
        with(
            &mut world,
            vec![
                (0, vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]),
                (1, vec![[0.25, 0.5], [0.5, 0.5], [0.25, 0.75]]),
            ],
        );
        let mut log = PassLog::default();
        LowerLayeredReskate::new(OverlayLimit::All)
            .run(&mut world, &mut log)
            .unwrap();
        let prims = world.get::<Mesh>(mesh).unwrap().primitives();
        let uv = |p: &Primitive| match &p.streams[&Semantic::TexCoord(0)] {
            Stream::F32x2(v) => v.clone(),
            s => panic!("{s:?}"),
        };
        assert_eq!(uv(&prims[0]), vec![[0.5, 0.0], [2.5, 0.0], [0.5, 4.0]]);
        assert_eq!(uv(&prims[1]), vec![[0.75, 2.5], [1.5, 2.5], [0.75, 3.25]]);
        assert_eq!(log.counts["parts drawn at their layer's coordinates"], 2);
        // the overlay's set missing: refused, nothing changed
        with(&mut refused, vec![(0, vec![[0.0, 0.0]; 3])]);
        refused_unchanged(refused, "TexCoord(1)");
    }

    #[test]
    fn lowering_draws_the_base_then_the_overlay_and_a_second_run_is_a_noop() {
        let (mut world, m, mesh) = layered_world(true, Some(1));
        let mut log = PassLog::default();
        LowerLayeredReskate::new(OverlayLimit::All)
            .run(&mut world, &mut log)
            .unwrap();
        assert_eq!(log.counts["overlay primitives"], 1);
        assert!(!world.has::<LayeredMaterial>(m) && !world.has::<SkateMaterial>(m));
        let prims = &world.get::<Mesh>(mesh).unwrap().primitives();
        assert_eq!(prims.len(), 2);
        assert_eq!(world.get::<SkateParts>(mesh).unwrap().0.len(), 2);
        let overlay = world.get::<SkateMaterial>(prims[1].material.unwrap()).unwrap();
        assert_eq!(
            (overlay.key.as_str(), overlay.class),
            ("blend~overlay1", MaterialClass::Decal)
        );
        assert_eq!(
            world.get::<VertexBlend>(prims[1].material.unwrap()),
            Some(&VertexBlend::Overlay { slot: 1 })
        );
        let op = &world.get::<BlendOpacity>(mesh).unwrap().0;
        assert_eq!(op[0], None);
        assert_eq!(op[1].as_deref(), Some(&[0.0, 0.0, 0.5][..]));
        let before = format!("{world:?}");
        let mut again = PassLog::default();
        LowerLayeredReskate::new(OverlayLimit::All)
            .run(&mut world, &mut again)
            .unwrap();
        assert!(again.counts.is_empty());
        assert_eq!(format!("{world:?}"), before);
    }
}
