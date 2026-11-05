//! The models effects' mesh particles draw (`fromsoft.effect_model`), each made the `core.mesh` its particles draw on its
//! own entity: posed by its bones as `fromsoft-apply-rigid-skinning` poses a placed model, and each mesh's level-0 face
//! set decoded as `lower-fromsoft-geometry` decodes one (positions, normals and UV0 in the world frame, with the further
//! UV sets and the vertex colours a material binary's programs read, [`source_streams`]). Each FLVER material is a
//! `pbr.material` of its own, unlit: its colour texture (the effect texture its `_a` path names) times the vertex colour,
//! alpha-blended. The material definitions (`S[A]_add_GXFlver` and the like) are not read, so what their programs do
//! beyond that is not drawn, counted on each material.
use super::super::geometry::source_streams;
use crate::passes::rigid::lower_model;
use crate::sekiro::components::effect::{EffectModel, EffectResource, ResourceKind, model_texture};
use crate::sekiro::source::LayerLayout;
use fromsoft_formats::flver::Model;
use fromsoft_formats::fxr::resources::TextureKind;
use primase_core::pass::PassLog;
use primase_core::pbr::{AlphaMode, PbrMaterial, Shading, TextureRef};
use primase_core::{Entity, Mesh, Name, Primitive, Semantic, Stream, Topology, World};
use std::collections::{BTreeMap, BTreeSet};

/// What every lowered material approximates.
pub const UNREAD_DEFINITION: &str = "effect model materials drawn unlit as their colour texture times the vertex colour (their definitions not read)";

/// Every effect model of `world` made the `core.mesh` of its entity, its materials `pbr.material` entities of their own;
/// `resources` are the effect resource entities by kind and number, where a material's textures are found. Returns the
/// models lowered; one that is already a mesh is an error (the pass ran twice), one that draws nothing or does not pose
/// is left without a mesh, counted.
pub(super) fn lower(
    world: &mut World,
    resources: &BTreeMap<(ResourceKind, i32), Entity>,
    log: &mut PassLog,
) -> Result<BTreeSet<Entity>, String> {
    let models: Vec<(Entity, i32, EffectModel)> = world
        .iter::<EffectModel>()
        .filter_map(|(e, m)| {
            let r = world.get::<EffectResource>(e)?;
            (r.kind == ResourceKind::Model).then(|| (e, r.id, m.clone()))
        })
        .collect();
    let mut lowered = BTreeSet::new();
    for (e, id, model) in models {
        if world.has::<Mesh>(e) {
            return Err(format!("effect model {id} ({e:?}) already has core.mesh"));
        }
        let posed = match lower_model(&model.0, log) {
            Ok(posed) => posed,
            Err(_) => {
                log.omit("effect models that do not pose rigidly, left out", 1);
                continue;
            }
        };
        let (primitives, sides) =
            primitives(&posed, log).map_err(|why| format!("effect model {id}: {why}"))?;
        if primitives.is_empty() {
            log.omit("effect models with no level-0 triangles, left out", 1);
            continue;
        }
        // a material entity for each FLVER material the primitives draw
        let mut materials = BTreeMap::new();
        for (&index, &double_sided) in &sides {
            let m = posed
                .materials
                .get(index)
                .ok_or_else(|| format!("effect model {id}: material {index} past the model's"))?;
            let material = PbrMaterial {
                base_color_texture: colour_texture(&m.textures, resources, log),
                metallic: 0.0,
                alpha_mode: AlphaMode::Blend,
                double_sided,
                shading: Shading::Unlit,
                ..PbrMaterial::default()
            };
            let entity = world.spawn();
            log.approximate_on(entity, UNREAD_DEFINITION, 1);
            let definition = m.mtd.rsplit(['\\', '/']).next().unwrap_or(&m.mtd);
            world.insert(
                entity,
                Name(format!("sfx model {id} material {} ({definition})", m.name)),
            );
            world.insert(entity, material);
            materials.insert(index, entity);
        }
        let primitives = primitives
            .into_iter()
            .map(|(index, mut p)| {
                p.material = materials.get(&index).copied();
                p
            })
            .collect();
        world.insert(e, Mesh::new(primitives));
        log.count("effect models lowered", 1);
        lowered.insert(e);
    }
    Ok(lowered)
}

/// The primitives of a model, each with the FLVER material index it draws (its `material` unset), and for each
/// material drawn whether any of its face sets draws back faces.
type Primitives = (Vec<(usize, Primitive)>, BTreeMap<usize, bool>);

/// The primitives of `model`'s level-0 face sets.
fn primitives(model: &Model, log: &mut PassLog) -> Result<Primitives, String> {
    let mut out = Vec::new();
    let mut sides: BTreeMap<usize, bool> = BTreeMap::new();
    for (index, mesh) in model.meshes.iter().enumerate() {
        let Some(face_set) = mesh.faces.iter().position(|f| f.is_lod0()) else {
            log.omit("effect model meshes with no level-0 face set", 1);
            continue;
        };
        let others = mesh.faces.len() - 1;
        if others > 0 {
            log.approximate(
                "effect model face sets other than the first of level 0 not lowered",
                others as u64,
            );
        }
        let Some(part) = crate::geometry::decode(model, index, face_set).map_err(|e| e.to_string())? else {
            continue;
        };
        part.mesh.validate().map_err(|e| e.to_string())?;
        let layout = LayerLayout::of(model, index).map_err(|e| e.to_string())?;
        let mut streams = BTreeMap::from([
            (Semantic::Position, Stream::F32x3(part.mesh.positions)),
            (Semantic::Normal, Stream::F32x3(part.mesh.normals)),
            (Semantic::TexCoord(0), Stream::F32x2(part.mesh.uv)),
        ]);
        streams.extend(source_streams(&mesh.vertices, &layout, &part.source_vertices));
        *sides.entry(mesh.material).or_default() |= !mesh.faces[face_set].cull;
        out.push((
            mesh.material,
            Primitive {
                topology: Topology::Triangles,
                streams,
                indices: Some(part.mesh.indices),
                material: None,
            },
        ));
    }
    Ok((out, sides))
}

/// The colour texture of a material's `textures`: the effect texture its first colour (`_a`) path names, as the read
/// found it (a `core.texture`, or a `core.texture_absent` where it could not give it). The material's other textures are
/// not drawn, and a material with no colour texture (a crystal's, whose bindings name no file) draws in its particles'
/// colour alone, both counted.
fn colour_texture(
    textures: &[fromsoft_formats::flver::Texture],
    resources: &BTreeMap<(ResourceKind, i32), Entity>,
    log: &mut PassLog,
) -> Option<TextureRef> {
    let mut colour = None;
    for t in textures.iter().filter(|t| !t.path.is_empty()) {
        match model_texture(&t.path) {
            Some((TextureKind::Albedo, number)) if colour.is_none() => {
                match resources.get(&(ResourceKind::Albedo, number)) {
                    Some(&e) => {
                        if t.scale != [1.0, 1.0] {
                            log.approximate("effect model colour textures' UV scale not applied", 1);
                        }
                        colour = Some(TextureRef::new(e));
                    }
                    None => log.omit("effect model colour textures the read did not keep", 1),
                }
            }
            Some(_) => log.approximate("effect model textures other than the colour texture not drawn", 1),
            None => log.omit("effect model textures whose path names no effect texture", 1),
        }
    }
    if colour.is_none() {
        log.approximate(
            "effect model materials with no colour texture, drawn in their particles' colour alone",
            1,
        );
    }
    colour
}
