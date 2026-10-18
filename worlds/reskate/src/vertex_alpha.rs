//! `lower-vertex-alpha-reskate`: decals that fade by vertex alpha, drawn as skate. can draw them.
//!
//! A material multiplies its base colour by `COLOR_0` (glTF), so a blended decal whose primitives carry `COLOR_0`
//! alpha below one fades there (GTA's vertex-alpha decals and weathering: drawn at full strength they look black).
//! skate.'s only per-vertex opacity is the stock decal row's (`Color0.r`): every decal material (`skate.material`
//! class decal, which raise-skate gives a material on `core.draw_order`'s decal layer) with such a primitive is
//! marked [`VertexBlend::VertexAlpha`], and each primitive drawing it gets its `COLOR_0` alpha as its opacity
//! ([`BlendOpacity`]; 1 where a primitive has no `COLOR_0`). The writer emits it with Studio's `vertex_blend`, and
//! the post-compile stage writes the opacity onto the stock decal row
//! ([`crate::postcompile::vblend::decal_opacity`], never the all-zero sentinel). Materials already marked (a lowered
//! blend's overlays) are left alone, and so are materials whose source does not take `COLOR_0`
//! (`core.vertex_color` `ignored`), counted.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Pass, PassLog};
use primase_core::{Entity, MaterialVariants, Mesh, Primitive, Semantic, Stream, World};
use primase_skate_scene::{MaterialClass, SkateMaterial};

use crate::layered::{BlendOpacity, VertexBlend};

/// A primitive's `COLOR_0` alpha per vertex, 0..1 (`None`: no `COLOR_0`).
fn alpha(p: &Primitive) -> Result<Option<Vec<f32>>, String> {
    Ok(match p.streams.get(&Semantic::Color(0)) {
        None => None,
        Some(Stream::U8x4(v)) => Some(v.iter().map(|c| f32::from(c[3]) / 255.0).collect()),
        Some(Stream::U16x4(v)) => Some(v.iter().map(|c| f32::from(c[3]) / 65535.0).collect()),
        Some(Stream::F32x4(v)) => Some(v.iter().map(|c| c[3].clamp(0.0, 1.0)).collect()),
        // RGB colour: no alpha
        Some(Stream::F32x3(_)) => None,
        Some(_) => return Err("COLOR_0 is not an RGB(A) stream".into()),
    })
}

/// Whether a primitive's `COLOR_0` colour (not its alpha) is other than white somewhere.
fn coloured(p: &Primitive) -> bool {
    match p.streams.get(&Semantic::Color(0)) {
        Some(Stream::U8x4(v)) => v.iter().any(|c| c[..3] != [255; 3]),
        Some(Stream::U16x4(v)) => v.iter().any(|c| c[..3] != [u16::MAX; 3]),
        Some(Stream::F32x4(v)) => v.iter().any(|c| c[..3].iter().any(|&x| x < 1.0)),
        Some(Stream::F32x3(v)) => v.iter().any(|c| c.iter().any(|&x| x < 1.0)),
        _ => false,
    }
}

/// The materials primitive `i` of mesh entity `e` is drawn with: its own and its variants'
/// ([`MaterialVariants`]).
fn drawn_with(world: &World, e: Entity, i: usize, p: &Primitive) -> Vec<Entity> {
    let mut m: Vec<Entity> = p.material.into_iter().collect();
    if let Some(v) = world.get::<MaterialVariants>(e).and_then(|v| v.0.get(i)) {
        m.extend(v.values().copied());
    }
    m
}

/// `lower-vertex-alpha-reskate` (see the module documentation).
#[derive(Clone, Copy, Debug, Default)]
pub struct LowerVertexAlphaReskate;

impl Pass for LowerVertexAlphaReskate {
    fn name(&self) -> &'static str {
        "lower-vertex-alpha-reskate"
    }
    // no `requires`: a part without decals has nothing to lower
    fn provides(&self) -> &[&'static str] {
        &["reskate.vertex_blend", "reskate.blend_opacity"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["reskate.blend_opacity"]
    }
    fn approximates(&self) -> &'static str {
        "decals fade through the game's mask-shaped dissolve, not linear alpha"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut decals: BTreeSet<Entity> = world
            .iter::<SkateMaterial>()
            .filter(|(e, m)| m.class == MaterialClass::Decal && !world.has::<VertexBlend>(*e))
            .map(|(e, _)| e)
            .collect();
        let ignoring = decals.len();
        decals.retain(|&m| primase_core::uses_color0(world, m));
        let ignoring = ignoring - decals.len();
        if ignoring > 0 {
            log.count(
                "decal materials ignoring COLOR_0 (core.vertex_color), not faded",
                ignoring as u64,
            );
        }
        if decals.is_empty() {
            return Ok(());
        }
        // the decals some primitive fades, drawn with its own material or a variant's
        let mut fading = BTreeSet::new();
        for (e, mesh) in world.iter::<Mesh>() {
            for (i, p) in mesh.primitives().iter().enumerate() {
                let ms: Vec<Entity> = drawn_with(world, e, i, p)
                    .into_iter()
                    .filter(|m| decals.contains(m))
                    .collect();
                if !ms.is_empty() && alpha(p)?.is_some_and(|a| a.iter().any(|&x| x < 1.0)) {
                    fading.extend(ms);
                }
            }
        }
        if fading.is_empty() {
            return Ok(());
        }
        // every primitive drawing one gets its opacity (planned before anything changes)
        let mut opacities: BTreeMap<Entity, Vec<Option<Vec<f32>>>> = BTreeMap::new();
        for (e, mesh) in world.iter::<Mesh>() {
            let fades =
                |i: usize, p: &Primitive| drawn_with(world, e, i, p).iter().any(|m| fading.contains(m));
            if !mesh.primitives().iter().enumerate().any(|(i, p)| fades(i, p)) {
                continue;
            }
            let mut o = world
                .get::<BlendOpacity>(e)
                .map_or_else(|| vec![None; mesh.primitives().len()], |b| b.0.clone());
            for (i, p) in mesh.primitives().iter().enumerate() {
                if !fades(i, p) {
                    continue;
                }
                if coloured(p) {
                    // the decal row's Color0 carries the opacity: the source's colour is not drawn
                    log.count(
                        "faded decal primitives whose COLOR_0 colour is not white (colour not drawn)",
                        1,
                    );
                }
                let a = match alpha(p)? {
                    Some(a) => {
                        log.count(
                            "faded decal vertices",
                            a.iter().filter(|&&x| x < 1.0).count() as u64,
                        );
                        a
                    }
                    None => {
                        log.count("decal primitives without COLOR_0 (opacity 1)", 1);
                        vec![1.0; p.vertex_count()]
                    }
                };
                o[i] = Some(a);
                log.count("faded decal primitives", 1);
            }
            opacities.insert(e, o);
        }
        for (e, o) in opacities {
            world.insert(e, BlendOpacity(o));
        }
        for m in fading {
            world.insert(m, VertexBlend::VertexAlpha);
            log.count("decal materials faded by vertex alpha", 1);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::Topology;

    fn decal(w: &mut World, class: MaterialClass) -> Entity {
        let e = w.spawn();
        w.insert(
            e,
            SkateMaterial {
                key: format!("m{}", e.index()),
                class,
                base: None,
                normal: None,
                mask: None,
                base_color: None,
                tint: None,
                alpha: None,
                cutoff: None,
                two_sided: None,
                domain: None,
                metallic: None,
                roughness: None,
                wrap: None,
                ocean: None,
                cast_shadows: None,
                emissive: None,
                vertex_alpha: None,
                other: Default::default(),
            },
        );
        e
    }

    fn prim(m: Entity, colour: Option<Vec<[u8; 4]>>) -> Primitive {
        let mut streams = BTreeMap::from([(
            Semantic::Position,
            Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]]),
        )]);
        if let Some(c) = colour {
            streams.insert(Semantic::Color(0), Stream::U8x4(c));
        }
        Primitive {
            topology: Topology::Triangles,
            streams,
            indices: None,
            material: Some(m),
        }
    }

    /// A decal faded on one primitive is marked on every primitive drawing it (1 where one has no COLOR_0); an
    /// unfaded decal and a faded non-decal are left alone.
    #[test]
    fn faded_decals_get_their_vertex_alpha_as_opacity() {
        let mut w = World::new();
        let faded = decal(&mut w, MaterialClass::Decal);
        let solid = decal(&mut w, MaterialClass::Decal);
        let alpha_surface = decal(&mut w, MaterialClass::Alpha);
        let mesh = w.spawn();
        let fade = vec![[255, 255, 255, 255], [255, 255, 255, 51], [255, 255, 255, 0]];
        w.insert(
            mesh,
            Mesh::new(vec![
                prim(faded, Some(fade.clone())),
                prim(faded, None),
                prim(solid, Some(vec![[255; 4]; 3])),
                prim(alpha_surface, Some(fade)),
            ]),
        );
        let mut log = PassLog::default();
        LowerVertexAlphaReskate.run(&mut w, &mut log).unwrap();
        assert_eq!(w.get::<VertexBlend>(faded), Some(&VertexBlend::VertexAlpha));
        assert!(!w.has::<VertexBlend>(solid) && !w.has::<VertexBlend>(alpha_surface));
        let o = &w.get::<BlendOpacity>(mesh).unwrap().0;
        assert_eq!(o[0].as_deref(), Some(&[1.0, 0.2, 0.0][..]));
        assert_eq!(o[1].as_deref(), Some(&[1.0; 3][..]));
        assert_eq!((o[2].clone(), o[3].clone()), (None, None));
        assert_eq!(log.counts["decal materials faded by vertex alpha"], 1);
        assert_eq!(log.counts["faded decal vertices"], 2);
    }
    /// A decal reached only through a material variant fades too; a faded primitive whose colour is not white is
    /// counted (the decal row's Color0 carries the opacity, so the colour is not drawn).
    #[test]
    fn a_variant_decal_fades_and_a_coloured_one_is_counted() {
        let mut w = World::new();
        let plain = decal(&mut w, MaterialClass::Alpha);
        let variant = decal(&mut w, MaterialClass::Decal);
        let mesh = w.spawn();
        let fade = vec![[255, 128, 128, 255], [255, 128, 128, 51], [255, 128, 128, 0]];
        w.insert(mesh, Mesh::new(vec![prim(plain, Some(fade))]));
        w.insert(mesh, MaterialVariants(vec![BTreeMap::from([(1, variant)])]));
        let mut log = PassLog::default();
        LowerVertexAlphaReskate.run(&mut w, &mut log).unwrap();
        assert_eq!(w.get::<VertexBlend>(variant), Some(&VertexBlend::VertexAlpha));
        assert!(!w.has::<VertexBlend>(plain));
        assert_eq!(
            w.get::<BlendOpacity>(mesh).unwrap().0[0].as_deref(),
            Some(&[1.0, 0.2, 0.0][..])
        );
        assert_eq!(
            log.counts["faded decal primitives whose COLOR_0 colour is not white (colour not drawn)"],
            1
        );
    }
}
