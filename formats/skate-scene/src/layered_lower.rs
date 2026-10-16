//! Explicit strongest-triangle approximation for PBR consumers of layered scenes.
use crate::SkatePrototype;
use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{Entity, LayeredMaterial, MaterialVariants, Mesh, MeshFile, World};
use std::collections::BTreeMap;

/// Requires loaded meshes and ordinary layers lowered to PBR. Every triangle
/// chooses the largest sum of corner weights, with (1-r) omitted for
/// rgb_product_4, as the reference skate writer does. No contour splitting.
/// Null slots choose opaque black. Logs triangles per slot, not per material.
#[derive(Debug, Default)]
pub struct LowerLayeredPbr;
impl Pass for LowerLayeredPbr {
    fn name(&self) -> &'static str {
        "lower-layered-pbr"
    }
    fn requires(&self) -> &[&'static str] {
        &["core.layered", "core.mesh", "pbr.material"]
    }
    fn provides(&self) -> &[&'static str] {
        &["pbr.material"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh", "core.material_variants", "skate.prototype"]
    }
    fn removes(&self) -> &[&'static str] {
        &["core.mesh_file"]
    }
    fn approximates(&self) -> &'static str {
        "strongest summed corner layer per triangle; no per-pixel blend, darkening or contour splitting"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        // Validate and prepare all changes before mutating the destination.
        let mut replacements = BTreeMap::<(Entity, usize), PbrMaterial>::new();
        let mut plans = Vec::new();
        let transformed_uv = (0..=u8::MAX).find(|&i| {
            world.iter::<Mesh>().all(|(_, m)| {
                m.primitives
                    .iter()
                    .all(|p| !p.streams.contains_key(&primase_core::Semantic::TexCoord(i)))
            })
        });
        for (entity, file) in world.iter::<MeshFile>() {
            if !world.has::<Mesh>(entity)
                && file
                    .materials
                    .iter()
                    .flatten()
                    .any(|m| world.has::<LayeredMaterial>(*m))
            {
                return Err("load layered meshes before strongest-layer lowering".into());
            }
        }
        for (entity, mesh) in world.iter::<Mesh>() {
            let variants = world.get::<MaterialVariants>(entity);
            if variants.is_some_and(|v| v.0.len() != mesh.primitives.len()) {
                return Err("material variant count differs from primitives".into());
            }
            let mut parts = Vec::new();
            let mut changed = false;
            for (i, primitive) in mesh.primitives.iter().enumerate() {
                let variant = variants.map(|v| v.0[i].clone()).unwrap_or_default();
                if variant.values().any(|m| world.has::<LayeredMaterial>(*m)) {
                    return Err("layered material is not allowed as a variant".into());
                }
                let layered = primitive
                    .material
                    .and_then(|e| world.get::<LayeredMaterial>(e).map(|m| (e, m)));
                let Some((material, layered)) = layered else {
                    parts.push((i, None, primitive.clone(), variant));
                    continue;
                };
                if !variant.is_empty() {
                    return Err("strongest-layer lowering refuses variants on a layered part".into());
                }
                let split =
                    primase_meshops::strongest_layer_parts(primitive, layered).map_err(|e| e.to_string())?;
                for (slot, mut part) in split {
                    let layer = layered.layers[slot].as_ref();
                    let mut pbr = match layer {
                        Some(layer) => world
                            .get::<PbrMaterial>(layer.material)
                            .cloned()
                            .ok_or_else(|| "lower ordinary layer materials to PBR first".to_owned())?,
                        None => PbrMaterial {
                            base_color: [0., 0., 0., 1.],
                            metallic: 0.,
                            ..Default::default()
                        },
                    };
                    if let Some(layer) = layer {
                        let mut texcoord = layer.texcoord.unwrap_or(0);
                        if let Some(transform) = layer.transform
                            && pbr.textures().next().is_some()
                        {
                            let Some(primase_core::Stream::F32x2(uv)) =
                                part.streams.get(&primase_core::Semantic::TexCoord(texcoord))
                            else {
                                return Err("selected layer requires a missing UV stream".into());
                            };
                            let uv = uv.iter().map(|&uv| transform.apply(uv)).collect();
                            texcoord = transformed_uv
                                .ok_or_else(|| "no free UV stream for layer transform".to_owned())?;
                            part.streams.insert(
                                primase_core::Semantic::TexCoord(texcoord),
                                primase_core::Stream::F32x2(uv),
                            );
                        }
                        for texture in [
                            &mut pbr.base_color_texture,
                            &mut pbr.normal_texture,
                            &mut pbr.metallic_roughness_texture,
                            &mut pbr.occlusion_texture,
                            &mut pbr.emissive_texture,
                        ]
                        .into_iter()
                        .flatten()
                        {
                            // The layer transform precedes each ordinary slot's
                            // transform. A separate stream preserves both even
                            // when their composition cannot be expressed as TRS.
                            texture.texcoord = texcoord;
                            if !matches!(
                                part.streams
                                    .get(&primase_core::Semantic::TexCoord(texture.texcoord)),
                                Some(primase_core::Stream::F32x2(_))
                            ) {
                                return Err("selected layer requires a missing UV stream".into());
                            }
                        }
                    }
                    replacements.insert((material, slot), pbr);
                    parts.push((i, Some((material, slot)), part, BTreeMap::new()));
                }
                changed = true;
            }
            if changed {
                if let Some(p) = world.get::<SkatePrototype>(entity)
                    && p.parts.len() != mesh.primitives.len()
                {
                    return Err("scene part metadata differs from primitives".into());
                }
                plans.push((entity, parts));
            }
        }
        let mut materials = BTreeMap::new();
        for (key, pbr) in replacements {
            let entity = world.spawn();
            world.insert(entity, pbr);
            materials.insert(key, entity);
        }
        for (entity, parts) in plans {
            let mut mesh = Mesh::default();
            let mut variants = Vec::new();
            let mut prototype = world.get::<SkatePrototype>(entity).cloned();
            let old_parts = prototype.as_ref().map(|p| p.parts.clone());
            if let Some(p) = &mut prototype {
                p.parts.clear();
            }
            for (source, key, mut part, variant) in parts {
                if let Some(key) = key {
                    part.material = Some(materials[&key]);
                    log.count(
                        &format!("triangles selected layer {}", key.1),
                        (part.indices.as_ref().expect("split indices").len() / 3) as u64,
                    );
                }
                mesh.primitives.push(part);
                variants.push(variant);
                if let Some(p) = &mut prototype {
                    p.parts
                        .push(old_parts.as_ref().expect("prototype parts")[source].clone());
                }
            }
            world.insert(entity, mesh);
            world.remove::<MeshFile>(entity);
            if world.has::<MaterialVariants>(entity) {
                world.insert(entity, MaterialVariants(variants));
            }
            if let Some(p) = prototype {
                world.insert(entity, p);
            }
        }
        Ok(())
    }
}
