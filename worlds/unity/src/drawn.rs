//! What a renderer other than Unity's can draw of a scene's mesh renderers (pass `unity-drawn`). A placement is left
//! out, counted by reason, when:
//! - its renderer's layer is one no camera drawing to the screen draws (no target texture; its culling mask without
//!   the layer's bit), as Unity culls it: what only a camera rendering into a texture sees (a minimap's markers) is
//!   not in the player's view. A camera still under a [`Condition`](primase_core::state::Condition) (run before
//!   `resolve-state`) counts as drawing. A world with no screen camera keeps every layer, counted;
//! - its prototype draws nothing (a mesh with no submesh that has indices: the prototype has no mesh);
//! - one of its materials is on a shader that builds its geometry in the vertex shader from component data
//!   ([`procedural`]): its mesh is a placeholder, and the GameObject's MonoBehaviours (`unity.behaviours`) hold the
//!   shape;
//! - every one of its primitives' materials is on a shader the lowering does not model ([`crate::lower`]) whose
//!   draw pass blends additively (destination factor One, source One or SrcAlpha): it adds light over what is
//!   behind and has no surface a metal-rough material can stand for.
//!
//! The draw pass is the one the player's render pipeline draws the surface with ([`UnityMaterial::draw_pass`]: URP's
//! forward pass, built-in's `ForwardBase`), in the subshader that pipeline takes. A blend factor bound to a material
//! property is that property's value.

use std::collections::BTreeMap;

use primase_core::pass::{Pass, PassLog};
use primase_core::{Entity, Instance, MaterialVariants, Mesh, MeshFile, World};

use crate::components::{UnityCamera, UnityMaterial, UnityMesh, UnityRenderSettings, UnityRenderer};
use crate::level::procedural;
use crate::lower::modelled;
use crate::read::player::RenderPipeline;

/// Whether a material is on a shader the lowering does not model whose draw pass under `pipeline` blends additively.
fn additive_unmodelled(m: &UnityMaterial, pipeline: &RenderPipeline) -> bool {
    use crate::material::blend_mode::{ONE, SRC_ALPHA};
    !modelled(m.shader.as_deref())
        && m.draw_pass(pipeline)
            .and_then(|p| m.blend_factors(p))
            .is_some_and(|(src, dst)| dst == ONE && (src == ONE || src == SRC_ALPHA))
}

/// The materials a placement draws: each primitive's, by its variant.
fn materials(w: &World, prototype: Entity, variant: u32) -> Vec<Option<Entity>> {
    let Some(mesh) = w.get::<Mesh>(prototype) else {
        return Vec::new();
    };
    let variants = w.get::<MaterialVariants>(prototype);
    mesh.primitives()
        .iter()
        .enumerate()
        .map(|(i, p)| {
            variants
                .and_then(|v| v.0.get(i))
                .and_then(|o| o.get(&variant))
                .copied()
                .or(p.material)
        })
        .collect()
}

/// Leave out what [`unity-drawn`](self) leaves out (see the module documentation).
pub fn unity_drawn(w: &mut World) -> PassLog {
    let mut log = PassLog::default();
    let mut gone: BTreeMap<Entity, String> = BTreeMap::new();
    // without a known pipeline no draw pass is known, and nothing is left out for its blending
    let pipeline = w
        .resource::<UnityRenderSettings>()
        .and_then(|s| s.pipeline.clone());
    // the layers the screen cameras draw; `None` without one, when no layer is judged
    let screen = w
        .iter::<UnityCamera>()
        .filter(|(_, c)| c.target_texture.is_none())
        .map(|(_, c)| c.culling_mask)
        .reduce(|a, b| a | b);
    if screen.is_none() {
        log.count("renderers' layers not judged: no camera draws to the screen", 1);
    }
    for (e, i) in w.iter::<Instance>() {
        let p = i.prototype;
        if let (Some(mask), Some(r)) = (screen, w.get::<UnityRenderer>(e))
            && 1u32.checked_shl(r.layer).is_some_and(|bit| mask & bit == 0)
        {
            gone.insert(
                e,
                format!("renderers left out: layer {} no screen camera draws", r.layer),
            );
            continue;
        }
        if !w.has::<Mesh>(p) && !w.has::<MeshFile>(p) {
            gone.insert(
                e,
                "renderers left out: their mesh draws nothing (no submesh with indices)".into(),
            );
            continue;
        }
        let ms: Vec<Option<&UnityMaterial>> = materials(w, p, i.variant)
            .into_iter()
            .map(|m| m.and_then(|m| w.get::<UnityMaterial>(m)))
            .collect();
        if let Some(shader) = ms
            .iter()
            .flatten()
            .filter_map(|m| m.shader.as_deref())
            .find(|s| procedural(s))
        {
            log.count(
                &format!("renderers left out: procedural geometry, shader {shader}"),
                1,
            );
            if let Some(u) = w.get::<UnityMesh>(p) {
                log.count(
                    &format!(
                        "renderers left out: procedural geometry, placeholder mesh {}/{}",
                        u.mesh.file, u.mesh.path_id
                    ),
                    1,
                );
            }
            gone.insert(e, String::new());
            continue;
        }
        let additive = |m: &&UnityMaterial| pipeline.as_ref().is_some_and(|pl| additive_unmodelled(m, pl));
        if !ms.is_empty() && ms.iter().all(|m| m.as_ref().is_some_and(additive)) {
            let shader = ms[0].and_then(|m| m.shader.clone()).unwrap_or_default();
            gone.insert(
                e,
                format!("renderers left out: additive blending on a shader not modelled, {shader}"),
            );
        }
    }
    for (e, why) in gone {
        let why = if why.is_empty() {
            "renderers left out"
        } else {
            why.as_str()
        };
        primase_core::ledger::despawn(w, e, &mut log, why);
    }
    log
}

/// [`unity_drawn`] as a pass.
#[derive(Debug, Clone, Copy)]
pub struct UnityDrawn;

impl Pass for UnityDrawn {
    fn name(&self) -> &'static str {
        "unity-drawn"
    }
    fn requires(&self) -> &[&'static str] {
        &["core.instance"]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn approximates(&self) -> &'static str {
        "procedural geometry, additive surfaces of shaders not modelled and layers no screen camera draws are left out"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let r = unity_drawn(w);
        log.absorb(r);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::UnityObject;
    use primase_core::MeshFile;

    fn obj(path_id: i64) -> UnityObject {
        UnityObject {
            file: "level2".into(),
            path_id,
        }
    }

    /// A placement of a mesh with no materials (none left out for its shaders) by a renderer on `layer`.
    fn placement(w: &mut World, prototype: Entity, layer: u32) -> Entity {
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
            UnityRenderer {
                renderer: obj(i64::from(layer)),
                layer,
                materials: Vec::new(),
                submeshes: None,
                cast_shadows: 1,
                receive_shadows: None,
            },
        );
        e
    }

    fn camera(w: &mut World, culling_mask: u32, target_texture: Option<UnityObject>) {
        let e = w.spawn();
        w.insert(
            e,
            UnityCamera {
                camera: obj(1),
                culling_mask,
                target_texture,
            },
        );
    }

    /// SkateBIRD level2's cameras: the screen camera's mask leaves layer 28 out, the minimap camera (into a texture)
    /// draws it. Its markers on layer 28 are left out, counted by layer; the rest stay.
    #[test]
    fn layers_no_screen_camera_draws_are_left_out() {
        let mut w = World::default();
        let p = w.spawn();
        w.insert(
            p,
            MeshFile {
                path: "m.glb".into(),
                mesh: 0,
                materials: Vec::new(),
            },
        );
        let ground = placement(&mut w, p, 0);
        let marker = placement(&mut w, p, 28);
        let odd = placement(&mut w, p, 40);
        camera(&mut w, 0xe003_ff17, None);
        camera(&mut w, 0xf003_ff17, Some(obj(1283)));
        let log = unity_drawn(&mut w);
        assert!(w.has::<Instance>(ground));
        assert!(!w.has::<Instance>(marker));
        // a layer beyond Unity's 32 is no bit of a mask: not judged
        assert!(w.has::<Instance>(odd));
        let reasons: Vec<&str> = log.drops.keys().map(|(_, why)| why.as_str()).collect();
        assert_eq!(reasons, ["renderers left out: layer 28 no screen camera draws"]);

        // two screen cameras draw the union of their masks
        camera(&mut w, 1 << 28, None);
        let marker = placement(&mut w, p, 28);
        unity_drawn(&mut w);
        assert!(w.has::<Instance>(marker));
    }

    /// Without a camera drawing to the screen (only one into a texture, or none), no layer is judged, counted.
    #[test]
    fn without_a_screen_camera_every_layer_stays() {
        let mut w = World::default();
        let p = w.spawn();
        w.insert(
            p,
            MeshFile {
                path: "m.glb".into(),
                mesh: 0,
                materials: Vec::new(),
            },
        );
        let marker = placement(&mut w, p, 28);
        camera(&mut w, 0, Some(obj(1283)));
        let log = unity_drawn(&mut w);
        assert!(w.has::<Instance>(marker));
        assert_eq!(
            log.counts
                .get("renderers' layers not judged: no camera draws to the screen")
                .copied(),
            Some(1)
        );
        assert!(log.drops.is_empty());
    }
}
