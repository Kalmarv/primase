//! Derive the generic base-diffuse UV composition from the exact stored vertex program.
//! TEXCOORD7.xy is a known generic interface, not an equation reimplemented here. Its active graph must
//! use only raw TEXCOORD0.xy and the six established base-matrix words. Frame and other outputs are cut.
use crate::components::MaterialRef;
use anvil_formats::shaunwhite::shaders::{Database, Key, Payload};
use iridia::{
    Program,
    flow::{At, Node, Values},
};
use iridia_interface::{ResourceId, ResourceKind, Stage};
use primase_core::shader::StageBinding;
use primase_core::shader::{Attribute, ConstantBinding, ConstantRole, InputBinding, InputRole, LaneStream};
use std::collections::BTreeSet;

pub const NAME: &str = "shaunwhite.generic-base-uv.v1";

#[derive(Clone, Debug)]
pub struct Derivation {
    pub name: &'static str,
    pub record: u32,
    pub key: Key,
    pub vertex_key: u64,
    pub source_hash: String,
    pub material: u32,
    pub output_register: u32,
    /// Active source constant-bank words, with original resource ids and word offsets.
    pub words: Vec<(ResourceId, u32)>,
    pub cuts: Vec<String>,
    pub cut_inputs: Vec<String>,
}

/// A game-owned derived invocation, not a replacement for the original VS or Colour binding.
/// Program Target0.xy returns the VS's base UV, with placeholder B=0/A=1. Inputs and constant bindings
/// retain source registers. Unused frame words remain Unknown; none of them affect the selected graph.
/// Consumers evaluate this graph on raw mesh UV0 at vertices, then interpolate the result as the VS does.
#[derive(Clone, Debug)]
pub struct DerivedBaseUv {
    pub program: Program,
    pub inputs: Vec<InputBinding>,
    pub constants: Vec<ConstantBinding>,
    pub derivation: Derivation,
}

fn describe(program: &Program, node: &Node) -> Option<String> {
    match node {
        Node::Input { input, lane } => {
            let v = &program.interface.inputs[*input];
            Some(format!("{}{} lane {lane}", v.label.name, v.label.index))
        }
        Node::Global { resource, at } => Some(format!("{resource:?} {at:?}")),
        _ => None,
    }
}

/// Derive the consumer's explicitly selected exact forward Colour record. This chooses no pass variant.
/// The Colour binding is used only to validate record provenance; its constants are not reused for the VS.
/// Material controls are packed against the VS's own CTAB allocation. No original binding is changed.
pub fn derive(
    database: &Database,
    record: u32,
    colour: &StageBinding,
    material: &MaterialRef,
) -> Result<DerivedBaseUv, String> {
    let r = database
        .records
        .get(record as usize)
        .ok_or("base UV: record outside SDB2 table")?;
    if r.key.kind != 4
        || r.key.permutation & 63 != 3
        || (r.key.permutation >> 20) & 15 != 2
        || r.key.permutation & ((1 << 6) | (1 << 25)) != 0
    {
        return Err("base UV: not a static generic forward Colour record".into());
    }
    let Payload::Effect(effect) = &r.payload else {
        return Err("base UV: record has no effect".into());
    };
    if effect.pixel.is_none() {
        return Err("base UV: stored Colour program missing".into());
    }
    let v = colour
        .variant
        .as_ref()
        .ok_or("base UV: exact Colour variant missing")?;
    if colour.stage != Stage::Pixel
        || v.field != "shaunwhite.shader.record"
        || v.value != record
        || !v
            .parts
            .iter()
            .any(|p| p.name == "SDB2 permutation" && p.value == r.key.permutation)
        || !v
            .parts
            .iter()
            .any(|p| p.name == "native pixel cache key" && p.value == effect.pixel_key)
    {
        return Err("base UV: binding does not name the exact Colour record/cache key".into());
    }
    let slot = material
        .slots
        .get("diffuse")
        .ok_or("base UV: diffuse slot fields not read")?;
    if slot.texcoord != 0 {
        return Err("base UV: stored generic VS expects base UV0, not another set".into());
    }
    let bytes = effect
        .vertex
        .as_deref()
        .ok_or("base UV: stored vertex program missing")?;
    let source = iridia_d3d9::read(bytes).map_err(|e| e.to_string())?;
    if source.stage != Stage::Vertex || source.instructions.len() > 512 {
        return Err("base UV: not a bounded SM3 vertex program".into());
    }
    let output = source
        .outputs
        .iter()
        .find(|o| o.usage == 5 && o.index == 7 && o.mask & 3 == 3)
        .ok_or("base UV: TEXCOORD7.xy not declared")?;
    let full = iridia_d3d9::lift(bytes).map_err(|e| e.to_string())?;
    let graph = Values::of_program(&full)?;
    let oi = full
        .interface
        .outputs
        .iter()
        .position(|o| o.label.name == "TEXCOORD" && o.label.index == 7)
        .ok_or("base UV: original TEXCOORD7 not lifted")?;
    let roots = [
        graph.output(oi, 0).ok_or("base UV: TEXCOORD7.x not written")?,
        graph.output(oi, 1).ok_or("base UV: TEXCOORD7.y not written")?,
    ];
    let active = graph.reached_from(roots);
    let mut allowed = BTreeSet::new();
    for resource in &full.interface.resources {
        if let ResourceKind::ConstantBuffer { layout } = &resource.kind {
            for c in &layout.constants {
                if c.name == "g_VectorArray" {
                    if !c.offset.is_multiple_of(4) || c.ty.size().is_none_or(|n| n < 64) {
                        return Err("base UV: base matrix allocation incomplete".into());
                    }
                    for relative in [8, 9, 10, 12, 13, 14] {
                        allowed.insert((resource.id, c.offset / 4 + relative));
                    }
                }
            }
        }
    }
    if allowed.len() != 6 {
        return Err("base UV: base matrix allocation missing or ambiguous".into());
    }
    let mut seen = BTreeSet::new();
    let mut input_lanes = BTreeSet::new();
    for &n in &active {
        match graph.node(n) {
            Node::Literal(v) if v.0.is_finite() => {}
            Node::Input { input, lane }
                if full.interface.inputs[*input].label.name == "TEXCOORD"
                    && full.interface.inputs[*input].label.index == 0
                    && *lane < 2 =>
            {
                input_lanes.insert((*input, *lane));
            }
            Node::Global {
                resource: Some(id),
                at: At::Component(word),
            } if allowed.contains(&(*id, *word)) => {
                seen.insert((*id, *word));
            }
            Node::Op { .. } => {}
            other => return Err(format!("base UV: unresolved/non-material dependency {other:?}")),
        }
    }
    if seen != allowed
        || input_lanes.len() != 2
        || input_lanes.iter().map(|(i, _)| i).collect::<BTreeSet<_>>().len() != 1
    {
        return Err("base UV: graph is not the complete UV0/base-matrix composition".into());
    }
    let program = iridia_d3d9::project_vertex_output(bytes, output.register, 3).map_err(|e| e.to_string())?;
    let constants = super::material::bind_constants(&program.interface, &[], material)?;
    for &(id, word) in &seen {
        let at = word * 4;
        let known=constants.iter().any(|c|c.resource==id&&(c.offset..c.offset+c.size).contains(&at)&&matches!(&c.role,ConstantRole::Material(words) if words.get(((at-c.offset)/4) as usize).is_some_and(|w|f32::from_bits(*w).is_finite())));
        if !known {
            return Err(format!("base UV: unresolved material word {id:?} byte {at}"));
        }
    }
    let inputs = program
        .interface
        .inputs
        .iter()
        .map(|v| InputBinding {
            label: v.label.clone(),
            role: if v.label.name == "TEXCOORD" && v.label.index == 0 {
                InputRole::Lanes(vec![LaneStream {
                    semantic: Attribute::TexCoord(0),
                    first: 0,
                    component: 0,
                    count: 2,
                }])
            } else {
                InputRole::Unknown {
                    why: "excluded from derived base-UV graph".into(),
                }
            },
        })
        .collect();
    let cut_inputs = graph
        .reached_from(graph.roots())
        .difference(&active)
        .filter_map(|n| describe(&full, graph.node(*n)))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(DerivedBaseUv {
        program,
        inputs,
        constants,
        derivation: Derivation {
            name: NAME,
            record,
            key: r.key,
            vertex_key: effect.vertex_key,
            source_hash: iridia::source_hash(bytes),
            material: material.id,
            output_register: output.register,
            words: seen.into_iter().collect(),
            cuts: vec![
                "clip position, lighting, fog, influence and other VS outputs: excluded, unresolved".into(),
                "TEXCOORD7.zw and placeholder B/A: not base UV or opacity".into(),
            ],
            cut_inputs,
        },
    })
}
