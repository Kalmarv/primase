//! Generic layered-material lowering; dialect metadata remains with the caller.
use crate::{Error, Result, SelectionPolicy};
use primase_core::pass::PassLog;
use primase_core::pbr::PbrMaterial;
use primase_core::{Entity, LayeredMaterial, MaterialVariants, Mesh, MeshFile, Primitive, World};
use std::collections::{BTreeMap, BTreeSet};

type MaterialKey = (Entity, usize);
type PlannedPart = (usize, Option<MaterialKey>, Primitive, BTreeMap<u32, Entity>);

/// Source of an output primitive. A selected null layer still has Some(slot).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayeredPartSource {
    pub primitive: usize,
    pub layer: Option<usize>,
}
/// Metadata correspondence for one changed mesh, in output primitive order.
#[derive(Clone, Debug)]
pub struct LayeredMeshSource {
    pub entity: Entity,
    pub primitive_count: usize,
    pub parts: Vec<LayeredPartSource>,
}
#[derive(Debug)]
pub struct LayeredPbrPlan {
    policy: SelectionPolicy,
    replacements: BTreeMap<MaterialKey, PbrMaterial>,
    plans: Vec<(Entity, Vec<PlannedPart>)>,
    sources: Vec<LayeredMeshSource>,
    cleanup_candidates: BTreeSet<Entity>,
}
impl LayeredPbrPlan {
    pub fn sources(&self) -> &[LayeredMeshSource] {
        &self.sources
    }
    pub fn policy(&self) -> SelectionPolicy {
        self.policy
    }

    /// Per changed mesh, the source primitive of each output primitive (World::remap_primitives' `from`).
    fn remaps(&self) -> Vec<(Entity, Vec<usize>)> {
        self.plans
            .iter()
            .map(|(e, parts)| (*e, parts.iter().map(|(source, ..)| *source).collect()))
            .collect()
    }

    /// Apply a fully validated plan, privately called by lower_layered_pbr.
    /// Its immutable validation callback cannot change the world between planning and application.
    /// Every per-primitive component of a changed mesh follows its source primitive (World::remap_primitives),
    /// tried on a copy before (lower_layered_pbr); the plan reads no dialect component itself.
    /// Materials are spawned in sorted source/slot order; per-slot triangle counts are logged.
    fn apply(self, world: &mut World, log: &mut PassLog) {
        let remaps = self.remaps();
        let mut materials = BTreeMap::new();
        for (key, pbr) in self.replacements {
            let entity = world.spawn();
            world.insert(entity, pbr);
            materials.insert(key, entity);
        }
        for ((entity, parts), (_, from)) in self.plans.into_iter().zip(remaps) {
            let mut primitives = Vec::new();
            let mut variants = Vec::new();
            for (_source, key, mut part, variant) in parts {
                if let Some(key) = key {
                    part.material = Some(materials[&key]);
                    log.count(
                        &match self.policy {
                            SelectionPolicy::OmitRgbProduct4Darkening => {
                                format!("triangles selected layer {}", key.1)
                            }
                            SelectionPolicy::CoreWeights => {
                                format!("triangles selected with core weights layer {}", key.1)
                            }
                        },
                        (part.indices.as_ref().expect("split indices").len() / 3) as u64,
                    );
                }
                primitives.push(part);
                variants.push(variant);
            }
            world
                .rebuild_primitives(entity, |_| Ok((primitives, from)))
                .expect("the same remap succeeded on the copy");
            world.remove::<MeshFile>(entity);
            if world.has::<MaterialVariants>(entity) {
                world.insert(entity, MaterialVariants(variants));
            }
        }
        let retained = world.closure(world.entities().filter(|e| !self.cleanup_candidates.contains(e)));
        for entity in self.cleanup_candidates.difference(&retained) {
            primase_core::ledger::despawn(world, *entity, log, "layered sources no longer used");
        }
    }
}

/// Prepare every geometry/material/UV/variant change without mutating the world.
/// Layer UV transforms precede texture transforms; ordinary layer materials must already be PBR.
/// This is a read-only census; use lower_layered_pbr to apply changes atomically with metadata validation.
/// The first unused texture-coordinate set is selected world-wide. A subset world may choose
/// a different free set, although both preserve the same layer-before-texture UV semantics.
/// Lower once before partitioning when consumers require identical UV set indices across
/// partitions. Independent world.subset() copies may choose different sets while preserving
/// the referenced UV values and transforms; each output must carry its own texture references.
pub fn plan_layered_pbr(world: &World, policy: SelectionPolicy) -> Result<LayeredPbrPlan> {
    // Validate and prepare all changes before mutating the destination.
    let mut replacements = BTreeMap::<(Entity, usize), PbrMaterial>::new();
    let mut plans = Vec::new();
    let mut replaced_hosts = BTreeSet::new();
    let transformed_uv = (0..=u8::MAX).find(|&i| {
        world.iter::<Mesh>().all(|(_, m)| {
            m.primitives()
                .iter()
                .all(|p| !p.streams.contains_key(&primase_core::Semantic::TexCoord(i)))
        })
    });
    for (entity, file) in world.iter::<MeshFile>() {
        if !world.has::<Mesh>(entity)
            && (file
                .materials
                .iter()
                .flatten()
                .any(|m| world.has::<LayeredMaterial>(*m))
                || world.get::<MaterialVariants>(entity).is_some_and(|variants| {
                    variants
                        .0
                        .iter()
                        .flat_map(|v| v.values())
                        .any(|m| world.has::<LayeredMaterial>(*m))
                }))
        {
            return Err(Error(
                "load layered meshes before strongest-layer lowering".into(),
            ));
        }
    }
    for (entity, mesh) in world.iter::<Mesh>() {
        let variants = world.get::<MaterialVariants>(entity);
        if variants.is_some_and(|v| v.0.len() != mesh.primitives().len()) {
            return Err(Error("material variant count differs from primitives".into()));
        }
        let mut parts = Vec::new();
        let mut changed = false;
        for (i, primitive) in mesh.primitives().iter().enumerate() {
            let variant = variants.map(|v| v.0[i].clone()).unwrap_or_default();
            if variant.values().any(|m| world.has::<LayeredMaterial>(*m)) {
                return Err(Error("layered material is not allowed as a variant".into()));
            }
            let layered = primitive
                .material
                .and_then(|e| world.get::<LayeredMaterial>(e).map(|m| (e, m)));
            let Some((material, layered)) = layered else {
                parts.push((i, None, primitive.clone(), variant));
                continue;
            };
            if !variant.is_empty() {
                return Err(Error(
                    "strongest-layer lowering refuses variants on a layered part".into(),
                ));
            }
            // Even an empty split replaces its source host.
            replaced_hosts.insert(material);
            let split = crate::strongest_layer_parts(primitive, layered, policy)?;
            for (slot, mut part) in split {
                let layer = layered.layers[slot].as_ref();
                let mut pbr = match layer {
                    Some(layer) => world
                        .get::<PbrMaterial>(layer.material)
                        .cloned()
                        .ok_or_else(|| Error("lower ordinary layer materials to PBR first".to_owned()))?,
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
                            return Err(Error("selected layer requires a missing UV stream".into()));
                        };
                        let uv = uv.iter().map(|&uv| transform.apply(uv)).collect();
                        texcoord = transformed_uv
                            .ok_or_else(|| Error("no free UV stream for layer transform".to_owned()))?;
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
                            return Err(Error("selected layer requires a missing UV stream".into()));
                        }
                    }
                }
                replacements.insert((material, slot), pbr);
                parts.push((i, Some((material, slot)), part, BTreeMap::new()));
            }
            changed = true;
        }
        if changed {
            plans.push((entity, parts));
        }
    }
    let sources = plans
        .iter()
        .map(|(entity, parts)| LayeredMeshSource {
            entity: *entity,
            primitive_count: world
                .get::<Mesh>(*entity)
                .expect("planned mesh")
                .primitives()
                .len(),
            parts: parts
                .iter()
                .map(|(primitive, key, _, _)| LayeredPartSource {
                    primitive: *primitive,
                    layer: key.map(|(_, slot)| slot),
                })
                .collect(),
        })
        .collect();
    Ok(LayeredPbrPlan {
        policy,
        replacements,
        plans,
        sources,
        cleanup_candidates: world.closure(replaced_hosts),
    })
}

/// Plan, validate dialect metadata against the unchanged world, then apply generic changes.
/// The callback cannot mutate the world; it may prepare metadata for the caller to publish
/// after success. Any planning/validation error leaves both world and pass log unchanged.
/// As with plan_layered_pbr, plan the whole world when partitions must agree on UV set indices.
/// Per-pixel layered writers should retain core.layered instead of using this approximation.
/// Replaced hosts and their dependencies are pruned only if unreachable from every entity
/// outside that original dependency closure; shared resources and unrelated entities survive.
/// This trusts Component::references(): dialect entity references must be declared there.
/// Independent world.subset() copies are safe. Do not lower a split_off() part and absorb it
/// back into its parent: references from the detached remainder are invisible to pruning.
pub fn lower_layered_pbr(
    world: &mut World,
    policy: SelectionPolicy,
    log: &mut PassLog,
    validate: impl FnOnce(&World, &[LayeredMeshSource]) -> Result<()>,
) -> Result<()> {
    let plan = plan_layered_pbr(world, policy)?;
    validate(world, plan.sources())?;
    // every per-primitive component of the changed meshes follows its source primitive: tried on a copy of those
    // meshes first, so a refusal (a component that does not have a source index) still changes nothing
    let remaps = plan.remaps();
    let mut probe = world.subset(&remaps.iter().map(|(e, _)| *e).collect());
    for (e, from) in &remaps {
        probe
            .remap_primitives(*e, from)
            .map_err(|m| Error(format!("{e:?}: {m}")))?;
    }
    drop(probe);
    plan.apply(world, log);
    Ok(())
}
