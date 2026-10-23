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
//!   listing has one entry per leaf of its selector tree, [`crate::selector`]) is one vertex stage and one pixel stage
//!   under a [`VariantKey`] of `frostbite.variant`, its value the permutation's index among the binding's (dense, from
//!   0). The key's parts describe it: `program` (the program's place in the material's row), `outputs` (the pixel
//!   stage's output count) and `permutation` (the permutation's id). They never identify it. Which option values
//!   reach a permutation is not kept here: the tree is on the program entity ([`crate::selector::Selector`]), walked
//!   for it. A permutation whose stages the world does not hold is left out, counted.
//! - **Inputs.** A vertex input is the element of the permutation's input layout its semantic index and name find
//!   ([`InputLayout::resolve`]); the element's usage names the stream: 1 the position, 33 and 34 the texture coordinate
//!   sets, 30 and 31 the colour sets (the usages [`crate::geometry`] decodes); any other usage, an element wider in the
//!   program than in its format, and an input without an element are `Unknown` with the reason. A pixel input is the
//!   vertex stage's output.
//! - **Constants.** A constant buffer at b0, b1 or b4 is the first group of the permutation's set 1, set 0 and pass
//!   binding ([`crate::binding::Bindings`]); its placed parameters are constants at their entries' offsets, sized by
//!   the entries (`kind / 4` bytes). The parameters of set 0 (b1) are the material's: the depot block's parameter of
//!   the same name, type and count gives the words ([`ConstantRole::Material`]); those of set 1 and the pass binding
//!   (b0, b4) are the engine's and `Unknown`, as is a parameter the block does not hold. Bytes of a buffer no placed
//!   parameter covers (a parameter whose entry states no offset, a read past the last placed one) are `Unknown`, and
//!   a buffer with no source (b2, b3, any other register or space) is one `Unknown` constant. A constant is clipped
//!   to the extent the shader declares for the buffer.
//! - **Textures.** Every texture is `Missing`: the depot block holds a texture reference (two GUIDs) for the slots the
//!   sets name, and linking it to a texture asset is not done here. The log counts the slots by whether the sets name
//!   them and the block holds a reference.
//! - **Outputs.** The pixel stage's render targets are `Unknown` by index.
//! - **State.** Not decoded: the default (nothing stated).

use std::collections::BTreeSet;

use primase_core::pass::{Pass, PassLog};
use primase_core::shader::{
    ConstantBinding, ConstantRole, D3dClass, InputBinding, InputRole, Interface, KeyPart, OutputBinding,
    OutputRole, RenderState, ResourceId, ResourceKind, Shader, ShaderBinding, Stage, StageBinding,
    SystemValue, TextureBinding, TextureSource, VariantKey,
};
use primase_core::{Component, Entity, Semantic, World};

use crate::binding::{Bindings, Group};
use crate::components::{
    FrostbiteBindings, FrostbiteInputLayout, FrostbiteMaterial, FrostbitePermutation, FrostbiteProgram,
    MaterialDepot,
};
use crate::depot;
use crate::inputlayout::{Found, InputLayout, dxgi_components};
use crate::permutation::Kind;

/// See the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResolveFrostbiteShaders;

impl ResolveFrostbiteShaders {
    /// From its options (it has none).
    pub fn from_options(o: &primase_core::pass::Options) -> Result<Self, String> {
        match o.keys().next() {
            Some(k) => Err(format!("unknown option {k}")),
            None => Ok(ResolveFrostbiteShaders),
        }
    }
}

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
        &[ShaderBinding::NAME]
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let materials: Vec<(Entity, FrostbiteMaterial)> = world
            .iter::<FrostbiteMaterial>()
            .map(|(e, m)| (e, m.clone()))
            .collect();
        for (material, m) in materials {
            match bind_material(world, &m, log) {
                Ok(binding) => {
                    world.insert(material, binding);
                    log.count("materials bound", 1);
                }
                Err(why) => log.count(&format!("materials left unbound: {why}"), 1),
            }
        }
        Ok(())
    }
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

fn bind_material(world: &World, m: &FrostbiteMaterial, log: &mut PassLog) -> Result<ShaderBinding, String> {
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
                let mut bound = stage_binding(
                    *stage,
                    *shader,
                    &program.interface,
                    bindings.as_ref(),
                    layout,
                    m.depot.as_ref(),
                    i > 0,
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
    vertex_bound: bool,
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
            (ResourceKind::Texture { .. }, D3dClass::ShaderResource) => {
                let named = (b.space == 0)
                    .then(|| bindings.and_then(|s| s.resource(b.register)))
                    .flatten();
                log.count(
                    match (named, depot) {
                        (None, _) => "texture slots no binding set entry names, left Missing",
                        (Some((group, i)), Some(d))
                            if d.parameters.iter().any(|p| {
                                p.name == group.params[i].name && p.texture_ref(&d.block.data).is_some()
                            }) =>
                        {
                            "texture slots the material's depot block holds a reference for (not linked to a texture asset), left Missing"
                        }
                        _ => "texture slots the sets name and the material's depot block holds no reference for, left Missing",
                    },
                    1,
                );
                textures.push(TextureBinding {
                    resource: r.id,
                    sampler: None,
                    source: TextureSource::Missing,
                });
            }
            _ => {}
        }
    }
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
                    role: OutputRole::Unknown(target),
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    StageBinding {
        stage,
        variant: None,
        shader,
        inputs,
        constants,
        textures,
        outputs,
    }
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
    InputRole::Stream(semantic)
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
        });
    }
    kept.sort_by_key(|c| c.offset);
    kept
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "hand-worked fixtures"
)]
mod tests {
    use super::*;
    use crate::binding::{Entry, Param};
    use crate::depot::{Block, TEXTURE};

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
        use crate::inputlayout::Element;
        let element = |dxgi: u32| Element {
            first: 0,
            last: 0,
            words: [0, 0],
            format: dxgi << 8,
        };
        // a position as R32G32B32_FLOAT (6), a texture coordinate as R16G16_FLOAT (34), a tangent space (52)
        let layout = InputLayout {
            id: 1,
            elements: vec![element(6), element(34), element(10)],
            usages: vec![1, 33, 52],
        };
        let mut log = PassLog::default();
        assert_eq!(
            element_role(&layout, 0, 3, &mut log),
            InputRole::Stream(Semantic::Position)
        );
        assert_eq!(
            element_role(&layout, 1, 2, &mut log),
            InputRole::Stream(Semantic::TexCoord(0))
        );
        assert!(
            matches!(element_role(&layout, 1, 4, &mut log), InputRole::Unknown { why } if why.contains("4 components"))
        );
        assert!(
            matches!(element_role(&layout, 2, 4, &mut log), InputRole::Unknown { why } if why.contains("usage 52"))
        );
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
        assert!(ResolveFrostbiteShaders::from_options(&primase_core::pass::Options::new()).is_ok());
        let o = primase_core::pass::Options::from([("x".to_owned(), "y".to_owned())]);
        assert!(ResolveFrostbiteShaders::from_options(&o).is_err());
    }
}
