//! `resolve-frostbite-shaders`: a `frostbite.material`'s programs as `core.shader_binding`, every permutation of every
//! program that draws pixels as a variant, none chosen.
//!
//! - **Which programs.** A material's program row holds one to a dozen programs in no fixed order (a compute program, a
//!   depth or shadow program with a vertex stage alone, the passes that draw pixels). The programs that draw pixels
//!   are the ones with a raster permutation that has a pixel stage, and each is bound: which of them is the main pass
//!   is not decoded (no record word, and not the pass binding, tells the 1-output programs from the 5-output ones), so
//!   the choice is left to the consumer, as is the choice among a program's permutations. A row with none is left
//!   unbound, counted. Vertex-only and compute programs are not bound.
//! - **Variants.** Each distinct permutation of a drawing program (a program lists up to 72, many the same one: the
//!   listing has one entry per leaf of its selector tree, [`frostbite_formats::selector`]) is one vertex stage and one pixel stage
//!   under a [`VariantKey`] of `frostbite.variant`, its value the permutation's index among the binding's (dense, from
//!   0). The key's parts describe it: `program` (the program's place in the material's row), `outputs` (the pixel
//!   stage's output count) and `permutation` (the permutation's id). They never identify it. Which option values
//!   reach a permutation is not kept here: the tree is on the program entity ([`frostbite_formats::selector::Selector`]), walked
//!   for it. A permutation whose stages the world does not hold is left out, counted.
//! - **Inputs.** A vertex input is the element of the permutation's input layout its semantic index and name find
//!   ([`InputLayout::resolve`]); the element's usage names the stream: 1 the position, 2 and 4 the blend indices and
//!   weights, 33 and 34 the texture coordinate sets, 30 and 31 the colour sets (the usages [`crate::geometry`] decodes); any other usage, an element wider in the
//!   program than in its format, and an input without an element are `Unknown` with the reason. A pixel input is the
//!   vertex stage's output.
//! - **Constants.** A constant buffer at b0, b1 or b4 is the first group of the permutation's set 1, set 0 and pass
//!   binding ([`frostbite_formats::binding::Bindings`]); its placed parameters are constants at their entries' offsets, sized by
//!   the entries (`kind / 4` bytes). The parameters of set 0 (b1) are the material's: the depot block's parameter of
//!   the same name, type and count gives the words ([`ConstantRole::Material`]); those of set 1 and the pass binding
//!   (b0, b4) are the engine's and `Unknown`, as is a parameter the block does not hold. Bytes of a buffer no placed
//!   parameter covers (a parameter whose entry states no offset, a read past the last placed one) are `Unknown`, and
//!   a buffer with no source (b2, b3, any other register or space) is one `Unknown` constant. A constant is clipped
//!   to the extent the shader declares for the buffer. Adjacent `Unknown` constants of one reason are one constant
//!   over their bytes, the parameter names dropped (the group on the permutation entity names every parameter): on the
//!   census sample the engine's lists were 205,208 constants in 1,916 stages and are 3,248 runs.
//!   Of a vertex stage's pass binding (b4) the words its clip position is placed with are stated when the program has
//!   the camera-relative form ([`super::clip_position`]): its sixteen row words are
//!   [`ViewParameter::CameraRelativeViewProjection`](primase_core::shader::ViewParameter::CameraRelativeViewProjection)
//!   and its three camera words [`ViewParameter::CameraPosition`](primase_core::shader::ViewParameter::CameraPosition), cut
//!   out of the `Unknown` constants over them; a program that has not that form keeps them `Unknown`. The words differ by
//!   pass, so they come from the program, not from a fixed offset.
//!   Its buffers are stated by running the program ([`super::buffer_tables`]): when it reads an instance table and a matrix
//!   table the way the probe documents, they are [`BufferTable::Instances`] and [`BufferTable::HeaderedMatrices`] (three
//!   rows right after the header) or, for a program that skins, [`BufferTable::SkinnedMatrices`] (a translation element
//!   after the header, then each joint's three rows); every other buffer of a vertex stage, and every buffer of a program the probe finds nothing in, is `Unknown`
//!   with the reason.
//! - **Textures.** A slot the binding sets name is the depot block's parameter of that name, and the parameter's link
//!   ([`FrostbiteMaterial::textures`], made by [`Source::resolve_textures`](frostbite_formats::folder::Source::resolve_textures))
//!   gives the texture entity: `Texture` with the default sampler state (the sampler is not decoded). The texture is a
//!   2D map (`core.texture`) or an array, a cube or a volume (`core.texture_layers`), as the reader made it, and it
//!   has to be the dimension the slot reads (a 2D map for a plain 2D slot, an array for an arrayed one, and so on).
//!   A slot with no such link is `Missing`, and the log counts the slots by why: the sets name no entry for the slot,
//!   the block holds no reference for the name, the reference was not resolved (the reader step did not run), the
//!   link states why it leads nowhere, or the texture is not what the slot reads.
//! - **Skinned meshes.** A vertex input that is the blend indices or weights (usages 2 and 4) is
//!   `Stream(Joints(0))` or `Stream(Weights(0))`, but the mesh holds no such stream: core takes joint and weight
//!   streams with a `core.skin`, which needs rest transforms of the skeleton, and the skeleton asset states none (the
//!   rig's default pose holds them under degree-of-freedom ids that are not mapped to joints), so the elements stay
//!   raw ([`crate::geometry`], [`Raw::joints`]). The bone lists of the mesh's sections (`MeshSet` sections, the
//!   identity 0..n for the CAS hat bucket) name skeleton joints by index and no document of the mesh names the
//!   skeleton: [`SKELETON_POLICY`] takes the one skeleton asset read with as many joints as the lists need, links it
//!   (`frostbite.skeleton_link`), and each such mesh is approximated on, counted by the range of joints its vertices
//!   are weighted to and their names, as drawn at the bind pose. No skeleton with enough joints, or several, is
//!   counted in the same note and not linked.
//! - **Outputs.** The pixel stage's render targets are `Unknown`: which pass a program is is not decoded, so no layout
//!   names a target's lanes.
//! - **State.** Not decoded: the default (nothing stated).

use std::collections::{BTreeMap, BTreeSet};

use iridia_interface::{
    D3dClass, Interface, RenderState, ResourceId, ResourceKind, SamplerState, Stage, SystemValue, TextureDim,
};
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::shader::{
    BufferBinding, BufferSource, BufferTable, ConstantBinding, ConstantRole, InputBinding, InputRole,
    KeyPart, ObjectParameter, OutputBinding, OutputRole, SkinPrefix, VariantKey,
};
use primase_core::{Component, Entity, Lods, Semantic, Texture, TextureLayers, World};
use primase_core::{
    shader::Shader, shader::ShaderBinding, shader::StageBinding, shader::TextureBinding,
    shader::TextureSource,
};

use super::buffer_tables::{MatrixLayout, SkinInputs, TableRoles, probe_tables_with};
use super::clip_position::{ClipRows, cut_out, shader_clip_rows, state_clip_roles};
use crate::components::{
    FrostbiteBindings, FrostbiteInputLayout, FrostbiteMaterial, FrostbiteMeshSet, FrostbitePermutation,
    FrostbiteProgram, FrostbiteSkeleton, FrostbiteSkeletonLink, FrostbiteVertexElements, MaterialDepot,
    MaterialTexture, TextureLink,
};
use crate::geometry::Raw;
use frostbite_formats::binding::{Bindings, Group};
use frostbite_formats::depot;
use frostbite_formats::inputlayout::{Found, InputLayout, dxgi_components};
use frostbite_formats::permutation::Kind;

/// See the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResolveFrostbiteShaders;

/// `resolve-frostbite-shaders`' declaration ([`primase_core::pass::PassRegistry::add`]): it takes no options.
pub const RESOLVE_FROSTBITE_SHADERS: PassSpec = PassSpec {
    name: "resolve-frostbite-shaders",
    options: &[],
    make: |_| Ok(Box::new(ResolveFrostbiteShaders)),
};

impl Pass for ResolveFrostbiteShaders {
    fn name(&self) -> &'static str {
        "resolve-frostbite-shaders"
    }
    fn requires(&self) -> &[&'static str] {
        &[
            FrostbiteMaterial::NAME,
            FrostbiteProgram::NAME,
            FrostbitePermutation::NAME,
            Shader::NAME,
        ]
    }
    fn provides(&self) -> &[&'static str] {
        &[ShaderBinding::NAME, FrostbiteSkeletonLink::NAME]
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let materials: Vec<(Entity, FrostbiteMaterial)> = world
            .iter::<FrostbiteMaterial>()
            .map(|(e, m)| (e, m.clone()))
            .collect();
        let mut clips = ClipCache::new();
        for (material, m) in materials {
            match bind_material(world, &m, &mut clips, log) {
                Ok(binding) => {
                    world.insert(material, binding);
                    log.count("materials bound", 1);
                }
                Err(why) => log.count(&format!("materials left unbound: {why}"), 1),
            }
        }
        link_skinned_meshes(world, log);
        Ok(())
    }
}

/// The policy that links a skinned mesh to a skeleton ([`choose_skeleton`]).
pub const SKELETON_POLICY: &str =
    "the only skeleton asset read with as many joints as the mesh's bone lists need";

/// The bones each mesh entity's sections need, by entity: the longest bone list of the level of detail the entity holds
/// (the MeshSet's entity holds level 0, the entities of its `core.lods` the others by their detail).
fn bones_needed(world: &World) -> BTreeMap<Entity, u32> {
    let mut needed = BTreeMap::new();
    for (root, set) in world.iter::<FrostbiteMeshSet>() {
        let mut levels = vec![(0usize, root)];
        if let Some(lods) = world.get::<Lods>(root) {
            levels.extend(lods.forms.iter().map(|f| (usize::from(f.detail), f.form)));
        }
        for (level, entity) in levels {
            let longest = set
                .meshset
                .lods
                .get(level)
                .into_iter()
                .flat_map(|l| &l.sections)
                .map(|s| s.bones.len() as u32)
                .max()
                .unwrap_or(0);
            needed.insert(entity, longest);
        }
    }
    needed
}

/// The skeleton whose joints the bone lists of a mesh address, when no document of the mesh names it: the one of
/// `skeletons` with at least `bones` joints ([`SKELETON_POLICY`]). `Err` says why there is none: fewer joints in every
/// skeleton read than the lists need (the lists are refused, not clipped), or several that have enough.
fn choose_skeleton<'a>(
    skeletons: &[(Entity, &'a FrostbiteSkeleton)],
    bones: u32,
) -> Result<(Entity, &'a FrostbiteSkeleton), String> {
    let enough: Vec<_> = skeletons
        .iter()
        .filter(|(_, s)| s.joints.len() as u32 >= bones)
        .collect();
    match enough.as_slice() {
        [one] => Ok(**one),
        [] => Err(format!(
            "none of the {} skeletons read has the {bones} joints the bone lists need",
            skeletons.len()
        )),
        several => Err(format!(
            "{} skeletons read have the {bones} joints the bone lists need",
            several.len()
        )),
    }
}

/// Links each mesh the programs skin by joints to its skeleton, noting it: the range of joints its vertices are
/// weighted to, by name, and that it is drawn at its bind pose (the skeleton states no rest transforms, so the
/// palette is the bind pose's own). A mesh whose skeleton is not found is noted as such, and not linked.
fn link_skinned_meshes(world: &mut World, log: &mut PassLog) {
    let skeletons: Vec<(Entity, FrostbiteSkeleton)> = world
        .iter::<FrostbiteSkeleton>()
        .map(|(e, s)| (e, s.clone()))
        .collect();
    let candidates: Vec<(Entity, &FrostbiteSkeleton)> = skeletons.iter().map(|(e, s)| (*e, s)).collect();
    let needed = bones_needed(world);
    let meshes: Vec<(Entity, Option<(u16, u16)>)> = world
        .iter::<FrostbiteVertexElements>()
        .map(|(e, v)| (e, joints_used(&v.primitives)))
        .collect();
    let mut links = Vec::new();
    for (mesh, joints) in meshes {
        let Some((first, last)) = joints else {
            continue;
        };
        let bones = needed.get(&mesh).copied().unwrap_or(0).max(u32::from(last) + 1);
        match choose_skeleton(&candidates, bones) {
            Ok((skeleton, s)) => {
                let name = |j: u16| s.joints.get(usize::from(j)).map_or("?", |j| j.name.as_str());
                log.approximate_on(
                    mesh,
                    &format!(
                        "meshes skinned by joints {first}-{last} ({}..{}) of {}, {SKELETON_POLICY} ({bones} joints): drawn at their bind pose",
                        name(first),
                        name(last),
                        s.name
                    ),
                    1,
                );
                links.push((
                    mesh,
                    FrostbiteSkeletonLink {
                        skeleton,
                        bones,
                        policy: SKELETON_POLICY.to_owned(),
                    },
                ));
            }
            Err(why) => log.approximate_on(
                mesh,
                &format!(
                    "meshes skinned by joints {first}-{last} of a skeleton not found ({why}): drawn at their bind pose"
                ),
                1,
            ),
        }
    }
    for (mesh, link) in links {
        world.insert(mesh, link);
    }
}

/// The least and greatest joint index a mesh's vertices are weighted to (a joint at a weight of zero is padding), over
/// its sections' blend elements ([`Raw::joints`], [`Raw::weights`]); `None` for a mesh with none. A section with
/// indices and no weights counts all four of its indices.
fn joints_used(sections: &[Vec<Raw>]) -> Option<(u16, u16)> {
    let mut range: Option<(u16, u16)> = None;
    for raw in sections {
        let Some(joints) = raw.iter().find_map(Raw::joints) else {
            continue;
        };
        let weights = raw.iter().find_map(Raw::weights);
        for (v, lanes) in joints.iter().enumerate() {
            for (k, &joint) in lanes.iter().enumerate() {
                if weights
                    .as_ref()
                    .is_some_and(|w| w.get(v).is_none_or(|w| w[k] == 0))
                {
                    continue;
                }
                range = Some(range.map_or((joint, joint), |(a, b)| (a.min(joint), b.max(joint))));
            }
        }
    }
    range
}

/// The key field of a binding's variants.
const VARIANT_FIELD: &str = "frostbite.variant";

/// The key of the `index`th variant of a binding: the permutation of the program at `program` in the material's row,
/// whose pixel stage writes `outputs` values.
fn variant_key(index: u32, program: usize, outputs: usize, permutation: u64) -> VariantKey {
    let part = |name: &str, value: u64| KeyPart {
        name: name.to_owned(),
        value,
    };
    VariantKey {
        field: VARIANT_FIELD.to_owned(),
        value: index,
        parts: vec![
            part("program", program as u64),
            part("outputs", outputs as u64),
            part("permutation", permutation),
        ],
    }
}

fn bind_material(
    world: &World,
    m: &FrostbiteMaterial,
    clips: &mut ClipCache,
    log: &mut PassLog,
) -> Result<ShaderBinding, String> {
    if m.programs.is_empty() {
        return Err("it has no program row".into());
    }
    let mut stages: Vec<StageBinding> = Vec::new();
    let mut variants = 0u32;
    for (place, &p) in m.programs.iter().enumerate() {
        let Some(program) = world.get::<FrostbiteProgram>(p) else {
            continue;
        };
        // a program lists a permutation once for each leaf of its selector tree: bound once
        let mut done = BTreeSet::new();
        for &e in &program.permutations {
            let Some(f) = world.get::<FrostbitePermutation>(e) else {
                continue;
            };
            let permutation = &f.permutation;
            if permutation.kind != Kind::Raster || permutation.bytecode.len() != 2 {
                continue;
            }
            if !done.insert(permutation.id) {
                continue;
            }
            let shaders: Vec<(Stage, Entity, &Shader)> = [Stage::Vertex, Stage::Pixel]
                .into_iter()
                .zip(&f.shaders)
                .filter_map(|(stage, &s)| Some((stage, s, world.get::<Shader>(s)?)))
                .collect();
            let [_, (_, _, pixel)] = shaders.as_slice() else {
                log.count(
                    "permutations left out: a program entity of theirs has no core.shader",
                    1,
                );
                continue;
            };
            let key = variant_key(variants, place, pixel.interface.outputs.len(), permutation.id);
            let sets = world.get::<FrostbiteBindings>(e);
            let layout = world.get::<FrostbiteInputLayout>(e).map(|l| &l.0);
            if sets.is_none() {
                log.count(
                    "permutations with no binding sets read: every constant and texture Unknown",
                    1,
                );
            }
            let bindings = sets.map(FrostbiteBindings::bindings);
            for (i, (stage, shader, program)) in shaders.iter().enumerate() {
                let vertex = (*stage == Stage::Vertex).then(|| {
                    let skin = skin_inputs(&program.interface, layout);
                    vertex_roles_of(clips, *shader, program, skin, log)
                });
                let mut bound = stage_binding(
                    *stage,
                    *shader,
                    &program.interface,
                    bindings.as_ref(),
                    layout,
                    m.depot.as_ref(),
                    &m.textures,
                    &|t| texture_dimension(world, t),
                    i > 0,
                    vertex.as_ref(),
                    log,
                );
                bound.variant = Some(key.clone());
                stages.push(bound);
            }
            variants += 1;
            log.count("variants bound", 1);
        }
    }
    if stages.is_empty() {
        return Err("no program of its row draws pixels".into());
    }
    Ok(ShaderBinding {
        stages,
        state: RenderState::default(),
    })
}

#[allow(clippy::too_many_arguments)]
fn stage_binding(
    stage: Stage,
    shader: Entity,
    interface: &Interface,
    bindings: Option<&Bindings<'_>>,
    layout: Option<&InputLayout>,
    depot: Option<&MaterialDepot>,
    links: &[MaterialTexture],
    dimension: &dyn Fn(Entity) -> Option<(TextureDim, bool)>,
    vertex_bound: bool,
    vertex: Option<&VertexRoles>,
    log: &mut PassLog,
) -> StageBinding {
    let inputs = match stage {
        Stage::Vertex => vertex_inputs(interface, layout, log),
        _ => interface
            .inputs
            .iter()
            .filter(|v| v.system.is_none())
            .map(|v| InputBinding {
                label: v.label.clone(),
                role: if vertex_bound {
                    InputRole::Previous
                } else {
                    InputRole::Unknown {
                        why: "the vertex stage is not bound".into(),
                    }
                },
            })
            .collect(),
    };
    let mut constants = Vec::new();
    let mut textures = Vec::new();
    for r in &interface.resources {
        let Some(b) = r.provenance.d3d else { continue };
        match (&r.kind, b.class) {
            (ResourceKind::ConstantBuffer { layout: buffer }, D3dClass::ConstantBuffer) => {
                let group = (b.space == 0)
                    .then(|| bindings.and_then(|s| s.constant_buffer(b.register)))
                    .flatten();
                constants.extend(buffer_constants(
                    group,
                    b.register,
                    b.space,
                    r.id,
                    buffer.size,
                    depot,
                    log,
                ));
            }
            (ResourceKind::Texture { dim, arrayed, .. }, D3dClass::ShaderResource) => {
                let named = (b.space == 0)
                    .then(|| bindings.and_then(|s| s.resource(b.register)))
                    .flatten();
                let source = texture_source(named, depot, links, (*dim, *arrayed), dimension, log);
                textures.push(TextureBinding {
                    resource: r.id,
                    sampler: None,
                    source,
                });
            }
            _ => {}
        }
    }
    if let Some((buffer, rows)) = vertex.and_then(|v| v.clip) {
        if state_clip_roles(&mut constants, buffer, rows) {
            log.count(
                "vertex stages whose clip position rows and camera position are stated",
                1,
            );
        } else {
            log.count(
                "vertex stages whose clip position words are not all unknown constants, left as they are",
                1,
            );
        }
    }
    if let Some(base) = vertex.and_then(|v| v.tables.as_ref().ok()).and_then(|t| t.base) {
        let role = ConstantRole::Object(ObjectParameter::InstanceBase);
        if cut_out(&mut constants, base.buffer, (base.offset, 4), role, None) {
            log.count("vertex stages whose first instance record word is stated", 1);
        } else {
            log.count(
                "vertex stages whose first instance record word is not an unknown constant, left as it is",
                1,
            );
        }
    }
    let buffers = vertex.map_or_else(Vec::new, |v| buffer_bindings(interface, &v.tables));
    let outputs = if stage == Stage::Pixel {
        interface
            .outputs
            .iter()
            .filter_map(|v| {
                let target = match v.system {
                    None => v.label.index,
                    Some(SystemValue::Target(n)) => n,
                    Some(_) => return None,
                };
                Some(OutputBinding {
                    label: v.label.clone(),
                    lanes: ((1u16 << v.value.width) - 1) as u8,
                    role: OutputRole::unknown(format!(
                        "render target {target} of a pass that is not decoded (no record word tells the main pass from the others), so no layout names its lanes"
                    )),
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    StageBinding {
        inputs,
        constants,
        textures,
        buffers,
        outputs,
        ..StageBinding::new(stage, shader)
    }
}

/// What the pass reads out of a vertex program by walking or running it: where its clip position takes its rows and
/// camera from ([`super::clip_position`]) and which buffers are its instance and matrix tables ([`super::buffer_tables`]).
#[derive(Clone, Debug)]
struct VertexRoles {
    clip: Option<(ResourceId, ClipRows)>,
    /// The tables, or why the program's buffers are left unknown.
    tables: Result<TableRoles, String>,
}

/// [`VertexRoles`] by shader entity: a program is read once however many materials and permutations use it.
type ClipCache = BTreeMap<Entity, VertexRoles>;

/// The roles of the vertex program `shader`; the log counts each program once, by what was found.
fn vertex_roles_of(
    cache: &mut ClipCache,
    entity: Entity,
    shader: &Shader,
    skin: Option<SkinInputs>,
    log: &mut PassLog,
) -> VertexRoles {
    cache
        .entry(entity)
        .or_insert_with(|| {
            let clip = shader_clip_rows(shader).ok();
            log.count(
                if clip.is_some() {
                    "vertex programs whose clip position is a camera-relative view-projection"
                } else {
                    "vertex programs whose clip position has no camera-relative form, left unknown"
                },
                1,
            );
            let tables = match clip {
                Some((_, rows)) => probe_tables_with(shader, rows, skin),
                None => Err("the clip position is not the camera-relative form the table probe needs".to_owned()),
            };
            log.count(
                if tables.is_ok() {
                    "vertex programs whose instance and matrix tables the probe found"
                } else {
                    "vertex programs whose buffers are left unknown: the probe found no instance and matrix tables"
                },
                1,
            );
            VertexRoles { clip, tables }
        })
        .clone()
}

/// The inputs of `interface` the mesh's blend weights and joint indices feed (the vertex inputs whose role is
/// `Stream(Weights(0))` and `Stream(Joints(0))` by the permutation's input layout), if the program has both.
fn skin_inputs(interface: &Interface, layout: Option<&InputLayout>) -> Option<SkinInputs> {
    let roles = vertex_inputs(interface, layout, &mut PassLog::default());
    // the roles are of the non-system inputs, in order: back to indices in the interface's inputs
    let at = |semantic: Semantic| -> Option<usize> {
        let role = roles
            .iter()
            .position(|r| r.role == InputRole::Stream(semantic.into()))?;
        interface
            .inputs
            .iter()
            .enumerate()
            .filter(|(_, v)| v.system.is_none())
            .nth(role)
            .map(|(k, _)| k)
    };
    Some(SkinInputs {
        weights: at(Semantic::Weights(0))?,
        joints: at(Semantic::Joints(0))?,
    })
}

/// The buffer slots of a vertex stage: the instance and matrix tables the probe found, as the roles [`BufferTable`] gives
/// them; every other buffer of the program, and every buffer of a program the probe found nothing in, is `Unknown` with the
/// reason.
fn buffer_bindings(interface: &Interface, tables: &Result<TableRoles, String>) -> Vec<BufferBinding> {
    let mut buffers = Vec::new();
    if let Ok(t) = tables {
        buffers.push(BufferBinding {
            resource: t.matrices,
            source: BufferSource::Table(match t.layout {
                MatrixLayout::Plain => BufferTable::HeaderedMatrices { rows: 3 },
                MatrixLayout::Skinned { prefix, joint_stride } => BufferTable::SkinnedMatrices {
                    rows: 3,
                    joint_stride,
                    prefix,
                    first: SkinPrefix::Translation,
                },
            }),
        });
        buffers.push(BufferBinding {
            resource: t.instances,
            source: BufferSource::Table(BufferTable::Instances {
                stride: t.stride,
                matrix_start: t.matrix_start,
                matrices: t.matrices,
            }),
        });
    }
    for r in &interface.resources {
        if matches!(r.kind, ResourceKind::Buffer(_)) && !buffers.iter().any(|b| b.resource == r.id) {
            let why = match tables {
                Ok(_) => "the table probe reads two buffers, the instance table and the matrix table; this is neither".to_owned(),
                Err(why) => format!("not read by the table probe: {why}"),
            };
            buffers.push(BufferBinding {
                resource: r.id,
                source: BufferSource::Unknown { why },
            });
        }
    }
    buffers
}

/// The dimension a shader reads texture `t` as and whether it is arrayed: a 2D map is a plain 2D texture, a texture of
/// several images is the dimension its shape states ([`TextureShape::dimension`](primase_core::TextureShape::dimension)).
fn texture_dimension(w: &World, t: Entity) -> Option<(TextureDim, bool)> {
    if w.has::<Texture>(t) {
        Some((TextureDim::D2, false))
    } else {
        w.get::<TextureLayers>(t).map(|l| l.shape.dimension())
    }
}

/// What the texture slot the binding sets name as `named` (a group and the index of its parameter) samples: the
/// texture of the material's link for the parameter's name, if it is the `slot` (dimension, arrayed) the shader reads
/// it as (`dimension` is the one a texture entity has), else `Missing`, the log counting why.
fn texture_source(
    named: Option<(&Group, usize)>,
    depot: Option<&MaterialDepot>,
    links: &[MaterialTexture],
    slot: (TextureDim, bool),
    dimension: &dyn Fn(Entity) -> Option<(TextureDim, bool)>,
    log: &mut PassLog,
) -> TextureSource {
    let Some((group, i)) = named else {
        log.count("texture slots no binding set entry names, left Missing", 1);
        return TextureSource::Missing {
            why: "no binding set entry names the texture slot".into(),
        };
    };
    let name = group.params[i].name;
    match links.iter().find(|t| t.name == name).map(|t| &t.link) {
        Some(TextureLink::Texture(texture)) => {
            if let Some(have) = dimension(*texture).filter(|have| *have != slot) {
                log.count(
                    &format!(
                        "texture slots left Missing, the texture is {} and the slot reads {}",
                        describe(have),
                        describe(slot)
                    ),
                    1,
                );
                return TextureSource::Missing {
                    why: format!(
                        "the texture is {} and the slot reads {}",
                        describe(have),
                        describe(slot)
                    ),
                };
            }
            log.count(
                "texture slots bound to the texture of the depot block's reference",
                1,
            );
            TextureSource::Texture {
                texture: *texture,
                sampler: SamplerState::default(),
            }
        }
        Some(TextureLink::Missing(why)) => {
            log.count(
                &format!("texture slots left Missing, the reference leads nowhere: {why}"),
                1,
            );
            TextureSource::Missing {
                why: format!("the texture reference leads nowhere: {why}"),
            }
        }
        None => {
            let held = depot.is_some_and(|d| {
                d.parameters
                    .iter()
                    .any(|p| p.name == name && p.texture_ref(&d.block.data).is_some())
            });
            log.count(
                if held {
                    "texture slots the depot block holds a reference for, not resolved (the texture step did not run), left Missing"
                } else {
                    "texture slots the sets name and the material's depot block holds no reference for, left Missing"
                },
                1,
            );
            TextureSource::Missing {
                why: if held {
                    "the depot reference was not resolved by the texture step"
                } else {
                    "the material depot holds no reference for the named slot"
                }
                .into(),
            }
        }
    }
}

/// A dimension and whether it is arrayed, as the log says it.
fn describe((dim, arrayed): (TextureDim, bool)) -> String {
    format!("{}{dim:?}", if arrayed { "an array of " } else { "a " })
}

/// The vertex inputs of a program as the streams of the permutation's input layout.
fn vertex_inputs(
    interface: &Interface,
    layout: Option<&InputLayout>,
    log: &mut PassLog,
) -> Vec<InputBinding> {
    let wanted: Vec<_> = interface.inputs.iter().filter(|v| v.system.is_none()).collect();
    let names: Vec<(&str, u32)> = wanted
        .iter()
        .map(|v| (v.label.name.as_str(), v.label.index))
        .collect();
    let found = layout.map(|l| l.resolve(&names));
    wanted
        .iter()
        .enumerate()
        .map(|(k, v)| {
            let role = match (layout, found.as_ref().and_then(|f| f[k])) {
                (None, _) => {
                    log.count(
                        "vertex inputs of a permutation with no input layout read, Unknown",
                        1,
                    );
                    InputRole::Unknown {
                        why: "the permutation's input layout was not read".into(),
                    }
                }
                (Some(_), None) => {
                    log.count(
                        "vertex inputs no element of the input layout is told for, Unknown",
                        1,
                    );
                    InputRole::Unknown {
                        why: format!(
                            "no one element of the input layout is {}{}",
                            v.label.name, v.label.index
                        ),
                    }
                }
                (Some(l), Some(Found::ByName(i) | Found::ByElimination(i))) => {
                    element_role(l, i, u32::from(v.value.width), log)
                }
            };
            InputBinding {
                label: v.label.clone(),
                role,
            }
        })
        .collect()
}

/// The role of the element `i` of `layout` read as `width` components.
pub fn element_role(layout: &InputLayout, i: usize, width: u32, log: &mut PassLog) -> InputRole {
    let usage = layout.usages[i];
    let semantic = match usage {
        1 => Semantic::Position,
        2 => Semantic::Joints(0),
        4 => Semantic::Weights(0),
        33 => Semantic::TexCoord(0),
        34 => Semantic::TexCoord(1),
        30 => Semantic::Color(0),
        31 => Semantic::Color(1),
        _ => {
            log.count(
                "vertex inputs of a usage the geometry reader does not decode, Unknown",
                1,
            );
            return InputRole::Unknown {
                why: format!("the element's usage {usage} is not a stream the geometry reader decodes"),
            };
        }
    };
    let format = layout.elements[i].dxgi();
    if dxgi_components(format).is_some_and(|stored| width > u32::from(stored)) {
        log.count("vertex inputs read wider than their element's format, Unknown", 1);
        return InputRole::Unknown {
            why: format!("the program reads {width} components of an element of DXGI format {format}"),
        };
    }
    log.count("vertex inputs that are a stream of the mesh", 1);
    InputRole::Stream(semantic.into())
}

/// The words a depot parameter holds: its value bytes after the 20-byte record header.
fn words(block: &depot::Block, p: &depot::Param) -> Vec<u32> {
    block.data[p.at + 20..p.at + p.len]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|w| u32::from_le_bytes(*w))
        .collect()
}

/// The constants of the constant buffer at `register` of `space` (`resource`, of `size` bytes in the shader): the
/// `group` the register rule names, if any, then `Unknown` over the bytes no placed parameter covers.
fn buffer_constants(
    group: Option<&Group>,
    register: u32,
    space: u32,
    resource: ResourceId,
    size: u32,
    depot: Option<&MaterialDepot>,
    log: &mut PassLog,
) -> Vec<ConstantBinding> {
    let whole = |why: String| {
        vec![ConstantBinding {
            resource,
            offset: 0,
            size,
            name: None,
            role: ConstantRole::Unknown { why },
            lines: None,
        }]
    };
    let Some(group) = group else {
        log.count(
            "constant buffers with no binding set (the register rule has none), one Unknown",
            1,
        );
        return whole(format!(
            "the constant buffer at b{register} (space {space}) has no binding set: the register rule names b0, b1 and b4 of space 0"
        ));
    };
    let engine = match register {
        0 => "set 1's constants (b0)",
        4 => "the pass binding's constants (b4)",
        _ => "",
    };
    let mut placed: Vec<ConstantBinding> = Vec::new();
    for (param, entry) in group.params.iter().zip(&group.entries) {
        let bytes = u32::from(entry.kind) / 4;
        if entry.at == u32::MAX || bytes == 0 || entry.at >= size {
            continue;
        }
        let bytes = bytes.min(size - entry.at);
        let role = if register == 1 {
            // the material's: the block's parameter of the same name, type and count
            match depot.and_then(|d| d.parameters.iter().find(|p| p.name == param.name).map(|p| (d, p))) {
                Some((d, p)) if p.ty == param.ty && p.count == param.count => {
                    let mut w = words(&d.block, p);
                    w.truncate((bytes / 4) as usize);
                    log.count("constants set from the material's depot block", 1);
                    ConstantRole::Material(w)
                }
                Some(_) => {
                    log.count(
                        "constants the depot block holds under another type or count, Unknown",
                        1,
                    );
                    ConstantRole::Unknown {
                        why: "the material's depot block holds this parameter under another type or count"
                            .into(),
                    }
                }
                None => {
                    log.count("constants the depot block holds no parameter for, Unknown", 1);
                    ConstantRole::Unknown {
                        why: "the material's depot block holds no parameter of this name".into(),
                    }
                }
            }
        } else {
            log.count("constants of the engine's sets (b0, b4), Unknown", 1);
            ConstantRole::Unknown {
                why: format!("the engine fills {engine}: no role resolved"),
            }
        };
        placed.push(ConstantBinding {
            resource,
            offset: entry.at,
            size: bytes,
            name: Some(format!("{:#018x}", param.name)),
            role,
            lines: None,
        });
    }
    placed.sort_by_key(|c| (c.offset, c.size));
    // overlapping parameters keep the first
    let mut kept: Vec<ConstantBinding> = Vec::new();
    for c in placed {
        if kept.last().is_none_or(|l| c.offset >= l.offset + l.size) {
            kept.push(c);
        }
    }
    let mut gaps = Vec::new();
    let mut at = 0;
    for c in &kept {
        if c.offset > at {
            gaps.push((at, c.offset - at));
        }
        at = c.offset + c.size;
    }
    if at < size {
        gaps.push((at, size - at));
    }
    for (offset, bytes) in gaps {
        log.count("constant buffer bytes no placed parameter covers, Unknown", 1);
        kept.push(ConstantBinding {
            resource,
            offset,
            size: bytes,
            name: None,
            role: ConstantRole::Unknown {
                why: format!(
                    "bytes of b{register} no placed parameter of its group covers (a parameter whose entry states no offset, or a read past the last placed one)"
                ),
            },
            lines: None,
        });
    }
    kept.sort_by_key(|c| c.offset);
    merge_unknown_runs(kept, log)
}

/// Adjacent `Unknown` constants of one reason as one constant over their bytes (its name dropped: the group on the
/// permutation entity still names every parameter). A constant with a role of its own, and one next to a different
/// reason or past a gap, stays as it is. Most of the pass binding's and of set 1's parameters are one run, so a stage
/// holds a few constants for them instead of a hundred.
fn merge_unknown_runs(constants: Vec<ConstantBinding>, log: &mut PassLog) -> Vec<ConstantBinding> {
    let mut out: Vec<ConstantBinding> = Vec::with_capacity(constants.len());
    for c in constants {
        if let (Some(last), ConstantRole::Unknown { why }) = (out.last_mut(), &c.role)
            && matches!(&last.role, ConstantRole::Unknown { why: before } if before == why)
            && last.resource == c.resource
            && last.offset + last.size == c.offset
        {
            last.size += c.size;
            last.name = None;
            log.count("unknown constants merged into the run before them", 1);
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "hand-worked fixtures"
)]
mod tests {
    use super::*;
    use frostbite_formats::binding::{Entry, Param};
    use frostbite_formats::depot::{Block, TEXTURE};
    use primase_core::shader::Attribute;

    const FLOAT: u32 = 0x2bb6_5c2d;
    const FLOAT4: u32 = 0x7f99_9829;

    /// A group of the parameters `(name, type, offset, bytes)`; an offset of `u32::MAX` is a parameter placed nowhere.
    fn group(params: &[(u64, u32, u32, u16)]) -> Group {
        Group {
            name: 1,
            size: 64,
            flags: [0; 4],
            words: [0; 4],
            params: params
                .iter()
                .map(|&(name, ty, _, _)| Param {
                    name,
                    ty,
                    count: 1,
                    extra: 0,
                })
                .collect(),
            entries: params
                .iter()
                .map(|&(_, _, at, bytes)| Entry {
                    at,
                    marker: 0xffff,
                    kind: bytes * 4,
                })
                .collect(),
        }
    }

    /// A depot block holding a float `0xA1` of 2.5 and a float4 `0xA2` of 1, 2, 3, 4, as a block lays them out: the
    /// record header (name, type, count, extra), a word of the value count, the values.
    fn material() -> MaterialDepot {
        let mut data = Vec::new();
        data.extend(2u32.to_le_bytes());
        for (name, ty, values) in [
            (0xA1u64, FLOAT, vec![2.5f32]),
            (0xA2, FLOAT4, vec![1.0, 2.0, 3.0, 4.0]),
        ] {
            data.extend(name.to_le_bytes());
            data.extend(ty.to_le_bytes());
            data.extend(1u16.to_le_bytes());
            data.extend(0u16.to_le_bytes());
            data.extend(1u32.to_le_bytes());
            for v in values {
                data.extend(v.to_le_bytes());
            }
        }
        let parameters = depot::params(&data).unwrap();
        MaterialDepot {
            resource: "depot".into(),
            bundle: "bundle".into(),
            block: Block { hash: 7, data },
            parameters,
        }
    }

    #[test]
    fn set_zero_parameters_are_the_materials_and_the_gaps_are_unknown() {
        let g = group(&[
            (0xA1, FLOAT, 4, 4),
            (0xA2, FLOAT4, 16, 16),
            (0xA3, FLOAT, u32::MAX, 4),
        ]);
        let d = material();
        let mut log = PassLog::default();
        let c = buffer_constants(Some(&g), 1, 0, ResourceId(3), 48, Some(&d), &mut log);
        let at: Vec<(u32, u32, bool)> = c
            .iter()
            .map(|c| (c.offset, c.size, matches!(c.role, ConstantRole::Material(_))))
            .collect();
        // bytes 0..4 and 8..16 and 32..48 are no placed parameter's
        assert_eq!(
            at,
            [
                (0, 4, false),
                (4, 4, true),
                (8, 8, false),
                (16, 16, true),
                (32, 16, false)
            ]
        );
        assert_eq!(c[1].role, ConstantRole::Material(vec![2.5f32.to_bits()]));
        assert_eq!(
            c[3].role,
            ConstantRole::Material([1.0f32, 2.0, 3.0, 4.0].map(f32::to_bits).to_vec())
        );
        assert_eq!(log.counts["constants set from the material's depot block"], 2);
        assert_eq!(
            log.counts["constant buffer bytes no placed parameter covers, Unknown"],
            3
        );
    }

    #[test]
    fn a_texture_slot_samples_the_texture_its_parameters_link_leads_to() {
        let g = group(&[
            (0xC1, TEXTURE, 0, 0),
            (0xC2, TEXTURE, 1, 0),
            (0xC3, TEXTURE, 2, 0),
        ]);
        let d = material();
        let links = [
            MaterialTexture {
                name: 0xC1,
                guid: [1; 16],
                link: TextureLink::Texture(Entity::from_index(9)),
            },
            MaterialTexture {
                name: 0xC2,
                guid: [2; 16],
                link: TextureLink::Missing("texture format 31".into()),
            },
        ];
        let mut log = PassLog::default();
        assert_eq!(
            texture_source(
                Some((&g, 0)),
                Some(&d),
                &links,
                (TextureDim::D2, false),
                &|_| None,
                &mut log
            ),
            TextureSource::Texture {
                texture: Entity::from_index(9),
                sampler: SamplerState::default()
            }
        );
        // a link that leads nowhere, a parameter with no link (the step did not run), a slot no entry names
        for (named, why) in [
            (
                Some((&g, 1)),
                "texture slots left Missing, the reference leads nowhere: texture format 31",
            ),
            (
                Some((&g, 2)),
                "texture slots the sets name and the material's depot block holds no reference for, left Missing",
            ),
            (None, "texture slots no binding set entry names, left Missing"),
        ] {
            assert!(matches!(
                texture_source(
                    named,
                    Some(&d),
                    &links,
                    (TextureDim::D2, false),
                    &|_| None,
                    &mut log
                ),
                TextureSource::Missing { why } if !why.is_empty()
            ));
            assert_eq!(log.counts[why], 1);
        }
        assert_eq!(
            log.counts["texture slots bound to the texture of the depot block's reference"],
            1
        );
    }

    #[test]
    fn a_texture_is_bound_only_to_a_slot_that_reads_its_dimension() {
        let g = group(&[(0xC1, TEXTURE, 0, 0)]);
        let d = material();
        let links = [MaterialTexture {
            name: 0xC1,
            guid: [1; 16],
            link: TextureLink::Texture(Entity::from_index(9)),
        }];
        let array = |_: Entity| Some((TextureDim::D2, true));
        let mut log = PassLog::default();
        // an array in an arrayed 2D slot is bound; the same array in a plain 2D slot, and a plain map in an arrayed one, are not
        assert!(matches!(
            texture_source(
                Some((&g, 0)),
                Some(&d),
                &links,
                (TextureDim::D2, true),
                &array,
                &mut log
            ),
            TextureSource::Texture { .. }
        ));
        for (slot, have, why) in [
            (
                (TextureDim::D2, false),
                (TextureDim::D2, true),
                "texture slots left Missing, the texture is an array of D2 and the slot reads a D2",
            ),
            (
                (TextureDim::D2, true),
                (TextureDim::D2, false),
                "texture slots left Missing, the texture is a D2 and the slot reads an array of D2",
            ),
            (
                (TextureDim::D3, false),
                (TextureDim::Cube, false),
                "texture slots left Missing, the texture is a Cube and the slot reads a D3",
            ),
        ] {
            let source = texture_source(Some((&g, 0)), Some(&d), &links, slot, &|_| Some(have), &mut log);
            assert!(
                matches!(source, TextureSource::Missing { why } if why == format!("the texture is {} and the slot reads {}", describe(have), describe(slot)))
            );
            assert_eq!(log.counts[why], 1);
        }
    }

    #[test]
    fn the_engines_buffers_and_a_buffer_with_no_source_are_unknown() {
        let g = group(&[(0xB1, FLOAT4, 0, 16)]);
        let mut log = PassLog::default();
        let c = buffer_constants(Some(&g), 4, 0, ResourceId(0), 16, None, &mut log);
        assert!(matches!(&c[0].role, ConstantRole::Unknown { why } if why.contains("pass binding")));
        assert_eq!(c[0].name.as_deref(), Some("0x00000000000000b1"));
        // b2 has no binding set: one Unknown over the buffer
        let none = buffer_constants(None, 2, 0, ResourceId(1), 96, None, &mut log);
        assert_eq!((none.len(), none[0].offset, none[0].size), (1, 0, 96));
        // a parameter the block holds under another type, or not at all
        let other = group(&[(0xA1, FLOAT4, 0, 16), (0xB9, FLOAT, 16, 4)]);
        let d = material();
        let c = buffer_constants(Some(&other), 1, 0, ResourceId(0), 32, Some(&d), &mut log);
        assert!(matches!(&c[0].role, ConstantRole::Unknown { why } if why.contains("another type")));
        assert!(matches!(&c[1].role, ConstantRole::Unknown { why } if why.contains("no parameter")));
    }

    #[test]
    fn adjacent_unknown_constants_of_one_reason_are_one_run() {
        // b4: three engine parameters, two adjacent (0..32) and one after a gap (48..64)
        let g = group(&[
            (0xB1, FLOAT4, 0, 16),
            (0xB2, FLOAT4, 16, 16),
            (0xB3, FLOAT4, 48, 16),
        ]);
        let mut log = PassLog::default();
        let c = buffer_constants(Some(&g), 4, 0, ResourceId(0), 64, None, &mut log);
        let runs: Vec<(u32, u32, Option<&str>)> =
            c.iter().map(|c| (c.offset, c.size, c.name.as_deref())).collect();
        // the first two are one run without a name, the gap has its own reason, the lone parameter keeps its name
        assert_eq!(
            runs,
            [
                (0, 32, None),
                (32, 16, None),
                (48, 16, Some("0x00000000000000b3"))
            ]
        );
        assert_eq!(log.counts["unknown constants merged into the run before them"], 1);
        // the parameters are still counted one by one
        assert_eq!(log.counts["constants of the engine's sets (b0, b4), Unknown"], 3);
        // set 0: a material's own words are never merged, nor are Unknowns of two reasons (the block holds 0xA2 as a
        // float4, not a float); the two parameters it holds nothing for are one run
        let g = group(&[
            (0xA1, FLOAT, 0, 4),
            (0xA2, FLOAT, 4, 4),
            (0xB9, FLOAT, 8, 4),
            (0xA3, FLOAT, 12, 4),
        ]);
        let d = material();
        let c = buffer_constants(
            Some(&g),
            1,
            0,
            ResourceId(0),
            16,
            Some(&d),
            &mut PassLog::default(),
        );
        let shape: Vec<(u32, u32, &str)> = c
            .iter()
            .map(|c| {
                let what = match &c.role {
                    ConstantRole::Material(_) => "material",
                    ConstantRole::Unknown { why } if why.contains("another type") => "type",
                    ConstantRole::Unknown { why } if why.contains("no parameter") => "none",
                    _ => "other",
                };
                (c.offset, c.size, what)
            })
            .collect();
        assert_eq!(shape, [(0, 4, "material"), (4, 4, "type"), (8, 8, "none")]);
    }

    #[test]
    fn a_constant_is_clipped_to_the_shaders_extent() {
        let g = group(&[(0xA2, FLOAT4, 8, 16)]);
        let d = material();
        let mut log = PassLog::default();
        // the shader declares 16 bytes: the float4 at 8 is cut to 8 and so are its words
        let c = buffer_constants(Some(&g), 1, 0, ResourceId(0), 16, Some(&d), &mut log);
        let placed = c.iter().find(|c| c.offset == 8).unwrap();
        assert_eq!(placed.size, 8);
        assert_eq!(
            placed.role,
            ConstantRole::Material([1.0f32, 2.0].map(f32::to_bits).to_vec())
        );
        // a parameter wholly past the extent is left out
        let far = group(&[(0xA1, FLOAT, 64, 4)]);
        let c = buffer_constants(Some(&far), 1, 0, ResourceId(0), 16, Some(&d), &mut log);
        assert_eq!(c.len(), 1);
        assert!(matches!(c[0].role, ConstantRole::Unknown { .. }));
        assert_eq!(TEXTURE, 0x14c5_fc30);
    }

    #[test]
    fn a_vertex_element_is_the_stream_its_usage_names_when_the_program_reads_no_more_than_it_holds() {
        use frostbite_formats::inputlayout::Element;
        let element = |dxgi: u32| Element {
            first: 0,
            last: 0,
            words: [0, 0],
            format: dxgi << 8,
        };
        // a position as R32G32B32_FLOAT (6), a texture coordinate as R16G16_FLOAT (34), a tangent space (52), blend
        // indices as R16G16B16A16_UINT (12) and weights as R8G8B8A8_UNORM (28)
        let layout = InputLayout {
            id: 1,
            elements: vec![element(6), element(34), element(10), element(12), element(28)],
            usages: vec![1, 33, 52, 2, 4],
        };
        let mut log = PassLog::default();
        assert_eq!(
            element_role(&layout, 0, 3, &mut log),
            InputRole::Stream(Attribute::Position)
        );
        assert_eq!(
            element_role(&layout, 1, 2, &mut log),
            InputRole::Stream(Attribute::TexCoord(0))
        );
        assert!(
            matches!(element_role(&layout, 1, 4, &mut log), InputRole::Unknown { why } if why.contains("4 components"))
        );
        assert!(
            matches!(element_role(&layout, 2, 4, &mut log), InputRole::Unknown { why } if why.contains("usage 52"))
        );
        assert_eq!(
            element_role(&layout, 3, 4, &mut log),
            InputRole::Stream(Attribute::Joints(0))
        );
        assert_eq!(
            element_role(&layout, 4, 4, &mut log),
            InputRole::Stream(Attribute::Weights(0))
        );
    }

    #[test]
    fn a_mesh_skinned_by_joints_is_noted_by_the_range_of_joints_it_is_weighted_to() {
        use frostbite_formats::meshset::{format, usage};
        let joints = |lanes: &[[u16; 4]]| Raw {
            usage: usage::BLEND_INDICES,
            format: format::USHORT4,
            size: 8,
            data: lanes.iter().flatten().flat_map(|j| j.to_le_bytes()).collect(),
        };
        let weights = |lanes: &[[u8; 4]]| Raw {
            usage: usage::BLEND_WEIGHTS,
            format: format::UBYTE4N,
            size: 4,
            data: lanes.iter().flatten().copied().collect(),
        };
        // the zero-weight lanes' indices (3 and 900) are padding
        let a = vec![
            joints(&[[3, 101, 3, 107], [3, 3, 900, 102]]),
            weights(&[[0, 255, 0, 0], [0, 0, 0, 255]]),
        ];
        assert_eq!(joints_used(std::slice::from_ref(&a)), Some((101, 102)));
        // a second section widens the range; one with indices and no weights counts all four of its indices
        let b = vec![joints(&[[5, 6, 7, 8]])];
        assert_eq!(joints_used(&[a.clone(), b.clone()]), Some((5, 102)));
        // a mesh with no blend element is not skinned
        let c = vec![Raw {
            usage: usage::TANGENT_SPACE,
            format: format::UBYTE4N,
            size: 4,
            data: vec![0; 4],
        }];
        assert_eq!(joints_used(&[c]), None);
        assert_eq!(joints_used(&[]), None);
    }

    #[test]
    fn a_skeleton_is_chosen_by_the_joints_the_bone_lists_need_and_never_clipped() {
        use frostbite_formats::skeleton::{SkeletonJoint, name_hash};
        let skeleton = |n: usize| FrostbiteSkeleton {
            name: format!("s{n}"),
            joints: (0..n)
                .map(|i| SkeletonJoint {
                    name: format!("j{i}"),
                    name_hash: name_hash(&format!("j{i}")),
                    parent: None,
                    child: None,
                    sibling: None,
                    kind: 1,
                })
                .collect(),
        };
        let (a, b, c) = (skeleton(27), skeleton(246), skeleton(395));
        let mut w = World::new();
        let ids: Vec<Entity> = (0..3).map(|_| w.spawn()).collect();
        let all = [(ids[0], &a), (ids[1], &b), (ids[2], &c)];
        // one has enough
        let (e, s) = choose_skeleton(&all, 386).unwrap();
        assert_eq!((e, s.joints.len()), (ids[2], 395));
        // none has enough: refused, not clipped to the largest
        assert!(
            choose_skeleton(&all, 396)
                .unwrap_err()
                .contains("none of the 3 skeletons")
        );
        // two have enough: ambiguous
        assert!(
            choose_skeleton(&all, 200)
                .unwrap_err()
                .contains("2 skeletons read have the 200")
        );
        assert!(choose_skeleton(&[], 1).is_err());
    }

    #[test]
    fn a_variants_key_is_a_dense_value_that_its_parts_only_describe() {
        let key = variant_key(3, 1, 5, 0xABCD);
        assert_eq!((key.field.as_str(), key.value), ("frostbite.variant", 3));
        let parts: Vec<(&str, u64)> = key.parts.iter().map(|p| (p.name.as_str(), p.value)).collect();
        assert_eq!(parts, [("program", 1), ("outputs", 5), ("permutation", 0xABCD)]);
        // the same value with other parts is another description of a variant of the same index
        assert_eq!(variant_key(3, 2, 1, 7).value, key.value);
    }

    #[test]
    fn the_pass_has_no_options() {
        assert!(
            RESOLVE_FROSTBITE_SHADERS
                .instance(&primase_core::pass::Options::new())
                .is_ok()
        );
        let o = primase_core::pass::Options::from([("x".to_owned(), "y".to_owned())]);
        assert!(RESOLVE_FROSTBITE_SHADERS.instance(&o).is_err());
    }
}
