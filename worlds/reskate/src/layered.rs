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
//! A null slot is a layer of black (core: it contributes black without redistributing its weight). An overlay keeps
//! only triangles with some corner whose opacity, as written ([`crate::postcompile::vblend::decal_opacity`]),
//! is above zero. The game's decal fades through a mask-shaped dissolve rather than linear alpha, and weights are
//! evaluated per vertex then interpolated (the core blend interpolates channels, then evaluates): both are named
//! approximations of the lowering pass, not of this arithmetic.

use primase_core::{Component, Entity, Mesh, World};
use serde::Serialize;

use crate::postcompile::vblend::decal_opacity;

/// What a vertex-blend material draws (`reskate.vertex_blend`, on material entities): the ReSkate writer emits it
/// with Studio's `vertex_blend` flag and its primitives' opacities ([`BlendOpacity`]) as the per-vertex weight, and
/// records it for the post-compile stage, which moves it onto the stock decal row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum VertexBlend {
    /// A layer of a lowered `core.layered` blend drawn over its base (slot `>= 1`).
    Overlay { slot: u8 },
    /// The black overlay of a lowered blend whose weights sum below one.
    Darkening,
    /// A decal whose opacity is its vertex alpha.
    VertexAlpha,
}

impl Component for VertexBlend {
    const NAME: &'static str = "reskate.vertex_blend";
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// Per-vertex opacity of a mesh's primitives (`reskate.blend_opacity`, on mesh entities; `None`: the primitive is not
/// blended), 0..1. Core has no vertex semantic for a decal's opacity, so it travels beside `core.mesh`.
#[derive(Clone, Debug, PartialEq)]
pub struct BlendOpacity(pub Vec<Option<Vec<f32>>>);

impl Component for BlendOpacity {
    const NAME: &'static str = "reskate.blend_opacity";
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        let mesh = w.get::<Mesh>(e).ok_or("blend opacity without core.mesh")?;
        if self.0.len() != mesh.primitives.len() {
            return Err(format!(
                "{} opacity lists for {} primitives",
                self.0.len(),
                mesh.primitives.len()
            ));
        }
        for (i, (o, p)) in self.0.iter().zip(&mesh.primitives).enumerate() {
            if let Some(o) = o {
                if o.len() != p.vertex_count() {
                    return Err(format!(
                        "primitive {i}: {} opacities for {} vertices",
                        o.len(),
                        p.vertex_count()
                    ));
                }
                if !o.iter().all(|x| x.is_finite() && (0.0..=1.0).contains(x)) {
                    return Err(format!("primitive {i}: opacity outside 0..1"));
                }
            }
        }
        Ok(())
    }
}

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OverlayCounts {
    /// Vertices whose weights summed above one and were scaled down.
    pub clamped_vertices: usize,
    /// Vertices whose weights summed below one (the darkening overlay draws there).
    pub darkening_vertices: usize,
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
/// triangles where some corner's written opacity is above zero (none: the overlay is left out).
pub fn overlay_parts(
    weights: &[[f32; 4]],
    slots: usize,
    indices: &[u32],
    counts: &mut OverlayCounts,
) -> Vec<OverlayPart> {
    let (over, dark) = opacities(weights, slots, counts);
    let layers = over
        .into_iter()
        .enumerate()
        .map(|(i, o)| (OverlayLayer::Slot(i + 1), o))
        .chain(std::iter::once((OverlayLayer::Darkening, dark)));
    let mut out = Vec::new();
    for (layer, opacity) in layers {
        let drawn = |i: u32| decal_opacity(opacity[i as usize])[0] > 0;
        let tris: Vec<u32> = indices
            .as_chunks::<3>()
            .0
            .iter()
            .filter(|t| t.iter().any(|&i| drawn(i)))
            .flatten()
            .copied()
            .collect();
        if !tris.is_empty() {
            out.push(OverlayPart {
                layer,
                indices: tris,
                opacity,
            });
        }
    }
    out
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

/// A black material: a null slot's layer, and the darkening overlay.
fn black(key: String, class: primase_skate_scene::MaterialClass) -> primase_skate_scene::SkateMaterial {
    use primase_skate_scene::{AlphaMode, Domain, MaterialClass, SkateMaterial};
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
#[derive(Debug, Default)]
pub struct LowerLayeredReskate;

impl primase_core::pass::Pass for LowerLayeredReskate {
    fn name(&self) -> &'static str {
        "lower-layered-reskate"
    }
    fn requires(&self) -> &[&'static str] {
        &["core.layered", "core.mesh", "skate.material"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.material", "reskate.vertex_blend", "reskate.blend_opacity"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh", "core.material_variants", "skate.prototype"]
    }
    fn removes(&self) -> &[&'static str] {
        &["core.layered"]
    }
    fn approximates(&self) -> &'static str {
        "decal overlays fade through the game's mask-shaped dissolve, not linear alpha; layer weights are evaluated per vertex and interpolated (the blend interpolates channels, then evaluates)"
    }
    fn run(&self, world: &mut World, log: &mut primase_core::pass::PassLog) -> Result<(), String> {
        use primase_core::{LayeredMaterial, MaterialVariants, MeshFile};
        use primase_skate_scene::{AlphaMode, Domain, MaterialClass, SkateMaterial, SkateParts};
        use std::collections::BTreeMap;
        let layered: BTreeMap<Entity, LayeredMaterial> = world
            .iter::<LayeredMaterial>()
            .map(|(e, l)| (e, l.clone()))
            .collect();
        if layered.is_empty() {
            return Ok(());
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
                if layer.texcoord.unwrap_or(0) != 0 || layer.transform.is_some() {
                    return Err(format!(
                        "layered material {e:?}: per-layer UV sets and transforms are not supported yet"
                    ));
                }
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
        for (e, mesh) in world.iter::<Mesh>() {
            let variants = world.get::<MaterialVariants>(e);
            if !mesh
                .primitives
                .iter()
                .any(|p| p.material.is_some_and(|m| layered.contains_key(&m)))
            {
                continue;
            }
            if world
                .get::<SkateParts>(e)
                .is_some_and(|p| p.0.len() != mesh.primitives.len())
            {
                return Err(format!("{e:?}: scene part metadata differs from primitives"));
            }
            let mut parts: Vec<PlannedPart> = Vec::new();
            for (i, p) in mesh.primitives.iter().enumerate() {
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
                let idx = p
                    .indices
                    .clone()
                    .unwrap_or_else(|| (0..p.vertex_count() as u32).collect());
                let mut base = p.clone();
                base.indices = Some(idx.clone());
                parts.push((i, base, None, Some((m, None))));
                *tally.entry("layered base triangles").or_default() += (idx.len() / 3) as u64;
                for o in overlay_parts(&w, l.layers.len(), &idx, &mut counts) {
                    let mut q = p.clone();
                    q.indices = Some(o.indices.clone());
                    let n = (o.indices.len() / 3) as u64;
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
        for (entity, parts) in plans {
            let old_parts = world.get::<SkateParts>(entity).cloned();
            let old_variants = world.get::<MaterialVariants>(entity).cloned();
            let mut mesh = Mesh::default();
            let mut opacity = Vec::new();
            let mut new_parts = old_parts.as_ref().map(|_| SkateParts::default());
            let mut variants = Vec::new();
            for (src, mut prim, op, what) in parts {
                if let Some(key) = what {
                    prim.material = Some(made[&key]);
                }
                mesh.primitives.push(prim);
                opacity.push(op);
                if let (Some(p), Some(old)) = (&mut new_parts, &old_parts) {
                    // planning checked one part per source primitive
                    p.0.push(old.0[src].clone());
                }
                if let Some(v) = &old_variants {
                    variants.push(v.0.get(src).cloned().unwrap_or_default());
                }
            }
            let any = opacity.iter().any(Option::is_some);
            world.insert(entity, mesh);
            world.remove::<MeshFile>(entity);
            if any {
                world.insert(entity, BlendOpacity(opacity));
            }
            if let Some(p) = new_parts {
                world.insert(entity, p);
            }
            if old_variants.is_some() {
                world.insert(entity, MaterialVariants(variants));
            }
        }
        for m in layered.keys() {
            world.remove::<LayeredMaterial>(*m);
            world.remove::<SkateMaterial>(*m);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::WeightMapping;

    /// Draw the base and the overlays in order at one vertex with the opacities as written (bytes), and compare
    /// with the core blend `sum(w_i * L_i)`.
    fn composite(layers: &[f64], weights: [f32; 4], slots: usize) -> (f64, f64) {
        let mut counts = OverlayCounts::default();
        let (over, dark) = opacities(&[weights], slots, &mut counts);
        let q = |a: f32| f64::from(decal_opacity(a)[0]) / 255.0;
        let mut c = layers[0];
        for i in 1..slots {
            let a = q(over[i - 1][0]);
            c = (1.0 - a) * c + a * layers[i];
        }
        c *= 1.0 - q(dark[0]);
        let w: Vec<f64> = weights[..slots].iter().map(|&x| f64::from(x)).collect();
        let s: f64 = w.iter().sum();
        let norm = if s > 1.0 { s } else { 1.0 };
        let want: f64 = w.iter().zip(layers).map(|(w, l)| w * l / norm).sum();
        (c, want)
    }

    /// Each written opacity is off by at most half a byte step, and each step moves the result by at most that times
    /// the colour range.
    fn bound(slots: usize) -> f64 {
        slots as f64 * 0.5 / 255.0 + 1e-9
    }

    fn lcg(seed: &mut u64) -> f64 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (*seed >> 11) as f64 / (1u64 << 53) as f64
    }

    #[test]
    fn two_three_and_four_layers_reproduce_the_core_blend() {
        let mut seed = 7u64;
        for slots in 2..=4 {
            for _ in 0..2000 {
                let mut w = [0f32; 4];
                let mut raw: Vec<f64> = (0..slots).map(|_| lcg(&mut seed)).collect();
                let total: f64 = raw.iter().sum();
                raw.iter_mut().for_each(|x| *x /= total);
                for i in 0..slots {
                    w[i] = raw[i] as f32;
                }
                let layers: Vec<f64> = (0..slots).map(|_| lcg(&mut seed)).collect();
                let (got, want) = composite(&layers, w, slots);
                assert!(
                    (got - want).abs() <= bound(slots + 1),
                    "{slots} slots {w:?}: {got} vs {want}"
                );
            }
        }
    }

    #[test]
    fn strongest_layer_not_layer_zero() {
        let layers = [0.9, 0.1, 0.5];
        let w = [0.1, 0.7, 0.2, 0.0];
        let (got, want) = composite(&layers, w, 3);
        assert!((got - want).abs() <= bound(4), "{got} vs {want}");
        // the base is slot 0 even though slot 1 is strongest; slot 1's opacity is w1 / (w0 + w1)
        let mut c = OverlayCounts::default();
        let (over, _) = opacities(&[w], 3, &mut c);
        assert!((over[0][0] - 0.7 / 0.8).abs() < 1e-6);
    }

    #[test]
    fn rgb_product_4_darkening_matches_evaluate() {
        let mut seed = 11u64;
        for _ in 0..2000 {
            let ch = [
                lcg(&mut seed) as f32,
                lcg(&mut seed) as f32,
                lcg(&mut seed) as f32,
                1.0,
            ];
            let w = WeightMapping::RgbProduct4.evaluate(ch);
            let layers: Vec<f64> = (0..4).map(|_| lcg(&mut seed)).collect();
            let (got, want) = composite(&layers, w, 4);
            assert!((got - want).abs() <= bound(5), "{ch:?}: {got} vs {want}");
        }
        let mut c = OverlayCounts::default();
        opacities(
            &[WeightMapping::RgbProduct4.evaluate([0.5, 0.2, 0.3, 1.0])],
            4,
            &mut c,
        );
        assert_eq!(c.darkening_vertices, 1);
    }

    #[test]
    fn weights_above_one_are_scaled_and_counted() {
        let w = [0.8, 0.6, 0.0, 0.0];
        let (got, want) = composite(&[0.2, 1.0], w, 2);
        assert!((got - want).abs() <= bound(3), "{got} vs {want}");
        let mut c = OverlayCounts::default();
        opacities(&[w], 2, &mut c);
        assert_eq!((c.clamped_vertices, c.darkening_vertices), (1, 0));
    }

    #[test]
    fn all_zero_weights_are_black_and_zero_over_zero_is_zero() {
        let (got, want) = composite(&[0.7, 0.4, 0.9], [0.0; 4], 3);
        assert!((got - want).abs() <= bound(4) && want == 0.0, "{got}");
        let mut c = OverlayCounts::default();
        let (over, dark) = opacities(&[[0.0; 4]], 3, &mut c);
        assert_eq!((over[0][0], over[1][0], dark[0]), (0.0, 0.0, 1.0));
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
        let parts = overlay_parts(&w, 2, &[0, 1, 2, 1, 2, 3], &mut c);
        assert_eq!(parts.len(), 1);
        assert_eq!(
            (parts[0].layer, parts[0].indices.clone()),
            (OverlayLayer::Slot(1), vec![1, 2, 3])
        );
    }

    use primase_core::pass::{Pass, PassLog};
    use primase_core::{Layer, LayerWeights, LayeredMaterial, Primitive, Semantic, Stream, Topology};
    use primase_skate_scene::{MaterialClass, SkateMaterial, SkateParts, SkatePrototype};
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
                weights: LayerWeights {
                    attribute: Semantic::Color(1),
                    mapping: WeightMapping::Identity,
                },
            },
        );
        if keyed {
            world.insert(m, black("blend".into(), MaterialClass::Opaque));
        }
        let mesh = world.spawn();
        world.insert(
            mesh,
            Mesh {
                primitives: vec![Primitive {
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
                }],
            },
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
        let err = LowerLayeredReskate.run(&mut world, &mut log).unwrap_err();
        assert!(err.contains(want), "{err}");
        assert_eq!(format!("{world:?}"), before);
        assert!(log.counts.is_empty(), "{:?}", log.counts);
    }

    #[test]
    fn part_metadata_mismatch_refuses_all_changes() {
        refused_unchanged(layered_world(true, Some(0)).0, "part metadata");
    }

    #[test]
    fn unkeyed_layered_material_refuses_all_changes() {
        refused_unchanged(layered_world(false, Some(1)).0, "no skate.material");
    }

    #[test]
    fn lowering_draws_the_base_then_the_overlay_and_a_second_run_is_a_noop() {
        let (mut world, m, mesh) = layered_world(true, Some(1));
        let mut log = PassLog::default();
        LowerLayeredReskate.run(&mut world, &mut log).unwrap();
        assert_eq!(log.counts["overlay primitives"], 1);
        assert!(!world.has::<LayeredMaterial>(m) && !world.has::<SkateMaterial>(m));
        let prims = &world.get::<Mesh>(mesh).unwrap().primitives;
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
        LowerLayeredReskate.run(&mut world, &mut again).unwrap();
        assert!(again.counts.is_empty());
        assert_eq!(format!("{world:?}"), before);
    }
}
