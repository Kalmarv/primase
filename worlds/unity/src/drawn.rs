//! What a renderer other than Unity's can draw of a scene's mesh renderers (pass `unity-drawn`). A placement is left
//! out, counted by reason, when:
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

use crate::components::{UnityMaterial, UnityMesh, UnityRenderSettings};
use crate::level::procedural;
use crate::lower::modelled;
use crate::player::RenderPipeline;

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
    for (e, i) in w.iter::<Instance>() {
        let p = i.prototype;
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
        if !why.is_empty() {
            log.count(&why, 1);
        }
        w.despawn(e);
    }
    log
}

/// [`unity_drawn`] as a pass.
#[derive(Debug, Clone, Copy, Default)]
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
        "procedural geometry and additive surfaces of shaders not modelled are left out"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let r = unity_drawn(w);
        log.absorb(r);
        Ok(())
    }
}
