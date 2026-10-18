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
//! blend's overlays) are left alone.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Pass, PassLog};
use primase_core::{Entity, Mesh, Primitive, Semantic, Stream, World};
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
        let decals: BTreeSet<Entity> = world
            .iter::<SkateMaterial>()
            .filter(|(e, m)| m.class == MaterialClass::Decal && !world.has::<VertexBlend>(*e))
            .map(|(e, _)| e)
            .collect();
        if decals.is_empty() {
            return Ok(());
        }
        // the decals some primitive fades
        let mut fading = BTreeSet::new();
        for (_, mesh) in world.iter::<Mesh>() {
            for p in mesh.primitives() {
                let Some(m) = p.material.filter(|m| decals.contains(m)) else {
                    continue;
                };
                if alpha(p)?.is_some_and(|a| a.iter().any(|&x| x < 1.0)) {
                    fading.insert(m);
                }
            }
        }
        if fading.is_empty() {
            return Ok(());
        }
        // every primitive drawing one gets its opacity (planned before anything changes)
        let mut opacities: BTreeMap<Entity, Vec<Option<Vec<f32>>>> = BTreeMap::new();
        for (e, mesh) in world.iter::<Mesh>() {
            if !mesh
                .primitives()
                .iter()
                .any(|p| p.material.is_some_and(|m| fading.contains(&m)))
            {
                continue;
            }
            let mut o = world
                .get::<BlendOpacity>(e)
                .map_or_else(|| vec![None; mesh.primitives().len()], |b| b.0.clone());
            for (i, p) in mesh.primitives().iter().enumerate() {
                if !p.material.is_some_and(|m| fading.contains(&m)) {
                    continue;
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
}
