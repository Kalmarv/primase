//! A named derivation from the generic forward Colour pixel program, never an albedo the game stored.
//!
//! The cut is the unique maximal RGB value on the output path whose value dependencies are diffuse layers,
//! mask, COLOR0 and material modulation/alpha controls. Texture coordinates remain the original expressions.
//! Lighting, fog, influence colour control and tone mapping stay in the original tail. This bounded derivation
//! refuses other families, extra value inputs and ambiguous frontiers; it never substitutes another permutation.
use anvil_formats::shaunwhite::shaders::{Database, Key, Payload};
use iridia::flow::{At, BinaryOperator, Node, NodeId, Op, UnaryOperator, Values};
use iridia::{Program, source_hash};
use iridia_d3d9::{Projection, Register};
use iridia_interface::{OutputBinding, OutputRole};
use primase_core::shader::StageBinding;
use std::collections::{BTreeMap, BTreeSet};

pub const NAME: &str = "shaunwhite.generic-albedo.v1";

/// The explicit influence choice of the consumer. The cut omits influence-dependent colour control;
/// this records that omission, rather than pretending that the source stored an influence-free output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Influence {
    Restored,
    Unrestored,
}

#[derive(Clone, Debug)]
pub struct Derivation {
    pub name: &'static str,
    pub record: u32,
    pub key: Key,
    pub pixel_key: u64,
    pub source_hash: String,
    pub projection: Projection,
    pub influence: Influence,
    /// These cut inputs are unresolved by the derivation, not silently bound to a guessed frame.
    pub cuts: Vec<String>,
    /// Native constant-bank words and varying labels contributing to forward RGB but not projected RGB.
    pub cut_inputs: Vec<String>,
}

/// An ephemeral shared-IR program and game-owned derivation. The original core.shader and its Colour binding
/// are unchanged. `binding.shader` still names that source shader; this program is consumed explicitly with
/// its derivation, never installed in place of the source bytecode. Alpha is a placeholder one, not opacity.
#[derive(Clone, Debug)]
pub struct DerivedAlbedo {
    pub program: Program,
    pub binding: StageBinding,
    pub derivation: Derivation,
    /// The original program with only the cut RGB replaced by DERIVED_RGB0 (v31.xyz).
    /// Used to prove the cut through the program's own light/fog/tone/peak-normalization tail.
    pub reencode: Program,
}

fn fingerprints(v: &Values) -> BTreeMap<NodeId, String> {
    fn visit(v: &Values, n: NodeId, out: &mut BTreeMap<NodeId, String>) -> String {
        if let Some(s) = out.get(&n) {
            return s.clone();
        }
        let children = v
            .operands(n)
            .into_iter()
            .map(|n| visit(v, n, out))
            .collect::<Vec<_>>();
        let head = match v.node(n) {
            Node::Op { op, .. } => format!("{op:?}"),
            Node::Texel {
                resource,
                lane,
                read,
                coordinate,
                ..
            } => format!("tex:{resource:?}:{lane}:{read:?}:{}", coordinate.len()),
            node => format!("{node:?}"),
        };
        let hash = source_hash(format!("{head}:{children:?}").as_bytes());
        out.insert(n, hash.clone());
        hash
    }
    let mut out = BTreeMap::new();
    for n in v.roots() {
        visit(v, n, &mut out);
    }
    out
}

/// Derive only from the exact retained source record and its matching binding. The material/pass resolver must
/// have selected that record explicitly. Unknown remaining bindings keep their original reasons.
pub fn derive(
    database: &Database,
    record: u32,
    binding: &StageBinding,
    influence: Influence,
) -> Result<DerivedAlbedo, String> {
    let r = database
        .records
        .get(record as usize)
        .ok_or("albedo: SDB2 record outside table")?;
    if r.key.kind != 4 || r.key.permutation & 63 != 3 || (r.key.permutation >> 20) & 7 != 2 {
        return Err("albedo: not a generic forward Colour record".into());
    }
    if binding.stage != iridia::Stage::Pixel {
        return Err("albedo: requires a pixel-stage binding".into());
    }
    if binding
        .variant
        .as_ref()
        .is_none_or(|v| v.field != "shaunwhite.shader.record" || v.value != record)
    {
        return Err("albedo: binding does not name the exact SDB2 record".into());
    }
    let Payload::Effect(effect) = &r.payload else {
        return Err("albedo: record has no effect".into());
    };
    let variant = binding.variant.as_ref().ok_or("albedo: missing record variant")?;
    for (name, value) in [
        ("SDB2 permutation", r.key.permutation),
        ("native pixel cache key", effect.pixel_key),
    ] {
        if !variant.parts.iter().any(|p| p.name == name && p.value == value) {
            return Err(format!(
                "albedo: binding {name} does not match the exact SDB2 record"
            ));
        }
    }
    let bytes = effect
        .pixel
        .as_deref()
        .ok_or("albedo: missing stored pixel program")?;
    let source = iridia_d3d9::read(bytes).map_err(|e| e.to_string())?;
    if source.instructions.len() > 512 {
        return Err("albedo: more than 512 instructions outside the bounded derivation".into());
    }
    let full = iridia_d3d9::lift(bytes).map_err(|e| e.to_string())?;
    let values = Values::of_program(&full)?;
    let hashes = fingerprints(&values);
    let output = full
        .interface
        .outputs
        .iter()
        .position(|v| v.label.name == "SV_Target" && v.label.index == 0)
        .ok_or("albedo: no Colour target")?;
    let colour = values.reached_from((0..3).filter_map(|l| values.output(output, l)));
    let reaches = colour.iter().map(|n| hashes[n].clone()).collect::<BTreeSet<_>>();
    let textures = effect
        .samplers
        .iter()
        .filter_map(|s| match s.texture.as_str() {
            "g_Layer1_DiffuseTexture" | "g_Layer2_DiffuseTexture" | "g_Layer3_DiffuseTexture" => {
                Some(s.address & 65535)
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let base = effect
        .samplers
        .iter()
        .find(|s| s.texture == "g_Layer1_DiffuseTexture")
        .ok_or("albedo: missing base diffuse sampler")?
        .address
        & 65535;
    if textures.is_empty() {
        return Err("albedo: no diffuse layers".into());
    }
    let masks = effect
        .samplers
        .iter()
        .filter(|s| s.texture == "g_MaskTexture")
        .map(|s| s.address & 65535)
        .collect::<BTreeSet<_>>();
    let mut allowed = BTreeSet::new();
    if let Some(table) = &source.constants {
        for c in &table.constants {
            if matches!(
                c.name.as_str(),
                "g_Layer1_ModulationColor"
                    | "g_Layer2_ModulationColor"
                    | "g_Layer3_ModulationColor"
                    | "g_FlagsUseTexAlpha"
            ) {
                allowed.extend(u32::from(c.register) * 4..u32::from(c.register + c.count) * 4);
            }
            // Native generic buffer packing: row 8 of g_VectorArray is Layer2_ModulationColor.
            // The SDB2/CTAB name retains the allocation; only these four value words enter the cut.
            if c.name == "g_VectorArray" && c.count > 8 {
                allowed.extend((u32::from(c.register) + 8) * 4..(u32::from(c.register) + 9) * 4);
            }
        }
    }
    let mut candidates = Vec::new();
    for i in &source.instructions {
        let Some(d) = i.destination else { continue };
        let Register::Temporary(temporary) = d.register else {
            continue;
        };
        if d.mask.count_ones() < 3 {
            continue;
        }
        for lanes in [[0, 1, 2], [0, 1, 3], [0, 2, 3], [1, 2, 3]] {
            if lanes.iter().any(|n| d.mask & (1 << n) == 0) {
                continue;
            }
            let cut = Projection {
                after_word: i.word,
                temporary,
                lanes,
            };
            let p = iridia_d3d9::project(bytes, cut).map_err(|e| e.to_string())?;
            let g = Values::of_program(&p)?;
            let h = fingerprints(&g);
            let roots = (0..3).filter_map(|l| g.output(0, l)).collect::<Vec<_>>();
            if roots.len() != 3 || roots.iter().any(|n| !reaches.contains(&h[n])) {
                continue;
            }
            let mut valid = true;
            for (lane, root) in roots.iter().enumerate() {
                let mut pending = vec![*root];
                let mut seen = BTreeSet::new();
                let mut sampled = BTreeSet::new();
                let mut base_lane = false;
                while let Some(n) = pending.pop() {
                    if !seen.insert(n) {
                        continue;
                    }
                    match g.node(n) {
                        Node::Literal(_) => {}
                        Node::Input { input, .. }
                            if p.interface.inputs[*input].label.name == "COLOR"
                                && p.interface.inputs[*input].label.index == 0 => {}
                        Node::Global {
                            at: At::Component(word),
                            ..
                        } if allowed.contains(word) => {}
                        Node::Texel {
                            resource: Some(id),
                            lane: tl,
                            ..
                        } => {
                            let resource = &p.interface.resources[id.0 as usize];
                            let slot = resource.provenance.d3d.as_ref().map(|d| d.register);
                            if let Some(s) = slot.filter(|s| textures.contains(s) || masks.contains(s)) {
                                if textures.contains(&s) && *tl != lane as u8 && *tl != 3 {
                                    valid = false;
                                }
                                sampled.insert(s);
                                if s == base && *tl == lane as u8 {
                                    base_lane = true;
                                }
                            } else {
                                valid = false;
                            }
                            // Coordinates/LOD stay authored and explicitly unresolved. They are not RGB values.
                        }
                        Node::Op {
                            op:
                                Op::Binary(
                                    BinaryOperator::Add | BinaryOperator::Subtract | BinaryOperator::Multiply,
                                )
                                | Op::Unary(UnaryOperator::Negate),
                            operands,
                        } => pending.extend(operands),
                        _ => valid = false,
                    }
                }
                valid &= base_lane && textures.is_subset(&sampled);
            }
            if valid {
                let ancestors = g
                    .reached_from(roots.iter().copied())
                    .iter()
                    .map(|n| h[n].clone())
                    .collect::<BTreeSet<_>>();
                let roots = roots.iter().map(|n| h[n].clone()).collect::<Vec<_>>();
                candidates.push((cut, p, roots, ancestors));
            }
        }
    }
    let maximal = candidates
        .iter()
        .enumerate()
        .filter(|(_, a)| candidates.iter().all(|b| b.2.iter().all(|n| a.3.contains(n))))
        .map(|(n, _)| n)
        .collect::<Vec<_>>();
    let Some(&chosen) = maximal.last() else {
        return Err("albedo: no unique material-only RGB frontier in the Colour graph".into());
    };
    if maximal.iter().any(|&n| candidates[n].2 != candidates[chosen].2) {
        return Err("albedo: ambiguous material-only RGB frontiers".into());
    }
    let (cut, mut program, _, _) = candidates.swap_remove(chosen);
    program.notes.push(format!("{NAME}: derived from forward Colour; lighting, fog, influence colour control and tone mapping cut; alpha placeholder"));
    let mut b = binding.clone();
    b.outputs = vec![
        OutputBinding {
            label: program.interface.outputs[0].label.clone(),
            lanes: 7,
            role: OutputRole::Albedo,
        },
        OutputBinding {
            label: program.interface.outputs[0].label.clone(),
            lanes: 8,
            role: OutputRole::unknown("derivation placeholder one, not source opacity"),
        },
    ];
    let projected_values = Values::of_program(&program)?;
    let projected_hashes = fingerprints(&projected_values);
    let mut cut_inputs = BTreeSet::new();
    let kept = projected_hashes.values().collect::<BTreeSet<_>>();
    for n in colour {
        if kept.contains(&hashes[&n]) {
            continue;
        }
        match values.node(n) {
            Node::Global {
                at: At::Component(word),
                ..
            } => {
                cut_inputs.insert(format!(
                    "c{} lane {}: unresolved forward input",
                    word / 4,
                    word % 4
                ));
            }
            Node::Input { input, lane } => {
                let label = &full.interface.inputs[*input].label;
                cut_inputs.insert(format!(
                    "{}{} lane {lane}: unresolved forward input",
                    label.name, label.index
                ));
            }
            Node::Texel {
                resource: Some(id), ..
            } => {
                cut_inputs.insert(format!(
                    "{}: unresolved forward texture",
                    full.interface.resources[id.0 as usize].provenance.name
                ));
            }
            _ => {}
        }
    }
    let reencode = iridia_d3d9::reencode(bytes, cut).map_err(|e| e.to_string())?;
    Ok(DerivedAlbedo {
        program,
        binding: b,
        reencode,
        derivation: Derivation {
            name: NAME,
            record,
            key: r.key,
            pixel_key: effect.pixel_key,
            source_hash: source_hash(bytes),
            projection: cut,
            influence,
            cut_inputs: cut_inputs.into_iter().collect(),
            cuts: vec![
                "lighting and self illumination: unresolved, neutral in proof".into(),
                "fog: unresolved, neutral in proof".into(),
                "influence colour control: unresolved; consumer mode explicit".into(),
                "tone mapping and peak normalization: excluded from albedo; original graph tail retained for proof".into(),
                "packed vertex varyings and texture coordinates: retained, unresolved bindings".into(),
                "source alpha and suffix coverage: excluded, not opacity".into(),
            ],
        },
    })
}
