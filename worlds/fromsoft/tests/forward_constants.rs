//! The sky's forward programs (the vertex and pixel programs of the materials whose pixel program is named `_Fwd` and
//! which the area's read binds only with its sky) of Sekiro's m11_01 and Elden Ring's m60_42_36_00, read with their
//! sky and bound by `resolve-fromsoft-shaders`: each constant word a program reads is stated, by a role (from core, the
//! view or the game's data) or by a default the resolve's log counts with its reason; none is unbound nor bound as
//! `Unknown`. A consumer writes an unstated word as zero, and a program dividing by it then writes 0 / 0 (Sekiro's sky
//! dome and Elden Ring's sky programs, before their volumetric fog parameters were stated, drew black or a NaN colour).
//!
//! The words a program reads are the constant buffer components its value graph (iridia's, from the program's own
//! bytes) reaches from what it writes. A buffer read at an index computed at run time is read at the lines the index
//! can take, as far as the index is made of literals, sums, products by a literal and selections (Elden Ring's sky
//! programs take a light set's line of `cbLight` at `envData * 14`); a sum of such a part and one of no known value
//! reads the lines of the known part, the other stepping through the elements of the array it starts in (Sekiro's sky
//! programs take an instance's colour at `13 + instance * 17 + slot`: `aColor[slot]` of record `instance`); at every
//! line otherwise. A failure says whether a
//! divisor (the right operand of a division, the operand of an inverse square root) is made of the word, through
//! operations and the values a branch or a selection may give.
//!
//! A word of an array's later element is stated where the same place of its first element is: each element of an
//! array of instance records is another instance's record, whose fields a binding states at the first record's offsets.
//!
//! It prints, per area, the forward programs it checked and the words they read.
//!
//! Needs `installs.sekiro`, `keys.sekiro`, `installs.elden`, `keys.elden` and `tools.oodle6`.
mod inputs;

use fromsoft_formats::{Game, Keys, dcx::Decoder};
use iridia::flow::{At, BinaryOperator, Literal, MathFunction, Node, NodeId, Op, Values};
use iridia_interface::{BufferLayout, ConstantType, Interface, ResourceId, ResourceKind, Stage};
use primase_core::shader::{ConstantRole, Shader, ShaderBinding, StageBinding};
use primase_core::{Entity, Name, World};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro;
use std::collections::{BTreeMap, BTreeSet};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

/// A constant word of a program: its buffer (the stored interface's id) and the word.
type Word = (ResourceId, u32);

/// The values `id` can take as a line of a buffer of `lines` lines ([`module docs`](self)), `None` for any.
fn lines_of(values: &Values, id: NodeId, lines: u32) -> Option<BTreeSet<u32>> {
    let literal = |id: NodeId| match values.node(id) {
        Node::Literal(Literal(v)) if *v >= 0.0 && v.fract() == 0.0 && *v < f64::from(u32::MAX) => {
            Some(*v as u32)
        }
        _ => None,
    };
    let pairs = |a: &BTreeSet<u32>, b: &BTreeSet<u32>, f: fn(u32, u32) -> Option<u32>| -> BTreeSet<u32> {
        a.iter()
            .flat_map(|x| b.iter().filter_map(move |y| f(*x, *y)))
            .filter(|v| *v < lines)
            .collect()
    };
    match values.node(id) {
        Node::Literal(_) => literal(id).map(|v| BTreeSet::from([v])),
        Node::Op {
            op: Op::Select,
            operands,
        } => {
            let mut out = BTreeSet::new();
            for &arm in operands.iter().skip(1) {
                out.extend(lines_of(values, arm, lines)?);
            }
            Some(out)
        }
        Node::Merge { arms, .. } => {
            let mut out = BTreeSet::new();
            for &arm in arms {
                out.extend(lines_of(values, arm, lines)?);
            }
            Some(out)
        }
        Node::Op {
            op: Op::Convert(_),
            operands,
        } if operands.len() == 1 => lines_of(values, operands[0], lines),
        Node::Op {
            op: Op::Binary(BinaryOperator::Add),
            operands,
        } if operands.len() == 2 => {
            match (
                lines_of(values, operands[0], lines),
                lines_of(values, operands[1], lines),
            ) {
                (Some(x), Some(y)) => Some(pairs(&x, &y, u32::checked_add)),
                // a known part and an unknown one: the unknown steps through the elements of the array the known part
                // starts in, whose later elements read as its first
                (Some(known), None) | (None, Some(known)) => Some(known),
                (None, None) => None,
            }
        }
        Node::Op {
            op: Op::Binary(BinaryOperator::Multiply),
            operands,
        } if operands.len() == 2 => {
            let (a, b) = (operands[0], operands[1]);
            match (lines_of(values, a, lines), lines_of(values, b, lines)) {
                (Some(x), Some(y)) => Some(pairs(&x, &y, u32::checked_mul)),
                // any value times a literal: its multiples
                (None, _) | (_, None) => {
                    let k = literal(b).or_else(|| literal(a))?;
                    Some(if k == 0 {
                        BTreeSet::from([0])
                    } else {
                        (0..lines).step_by(k as usize).collect()
                    })
                }
            }
        }
        _ => None,
    }
}

/// The constant words `id` reads, if it is a constant buffer's component: of `interface`'s buffer (the graph's id).
fn read_words(values: &Values, interface: &Interface, id: NodeId) -> Vec<(ResourceId, u32)> {
    let Node::Global {
        resource: Some(buffer),
        at,
    } = values.node(id)
    else {
        return Vec::new();
    };
    let Some(ResourceKind::ConstantBuffer { layout }) = interface.resource(*buffer).map(|r| &r.kind) else {
        return Vec::new();
    };
    let words = layout.size.div_ceil(4);
    match at {
        At::Component(word) => vec![(*buffer, *word)],
        At::Indexed { lane, index } => {
            let lines = index
                .first()
                .filter(|_| index.len() == 1)
                .and_then(|&i| lines_of(values, i, words.div_ceil(4)));
            match lines {
                Some(lines) => lines
                    .into_iter()
                    .map(|l| (*buffer, l * 4 + lane))
                    .filter(|(_, w)| *w < words)
                    .collect(),
                None => (0..words).map(|w| (*buffer, w)).collect(),
            }
        }
    }
}

/// The nodes `root` is made of ([`module docs`](self)): through operations and the values a branch or a selection may
/// give, not through the conditions that choose between them nor through a texture's coordinate.
fn made_of(values: &Values, root: NodeId) -> BTreeSet<NodeId> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        match values.node(id) {
            // the selected value is the accepted or the rejected one; the condition only chooses
            Node::Op {
                op: Op::Select,
                operands,
            } => stack.extend(operands.iter().skip(1)),
            Node::Op { operands, .. } => stack.extend(operands),
            Node::Merge { arms, .. } | Node::Loop { arms, .. } => stack.extend(arms),
            // a run-time index chooses the line: the line's word is what the value is made of
            Node::Global { .. }
            | Node::Texel { .. }
            | Node::Input { .. }
            | Node::Supplied { .. }
            | Node::Query { .. }
            | Node::Literal(_) => {}
        }
    }
    seen
}

/// The divisors of `values` ([`module docs`](self)).
fn divisors(values: &Values) -> Vec<NodeId> {
    values
        .reached_from(values.roots())
        .into_iter()
        .filter_map(|id| match values.node(id) {
            Node::Op {
                op: Op::Binary(BinaryOperator::Divide),
                operands,
            } => operands.get(1).copied(),
            Node::Op {
                op: Op::Math(MathFunction::InverseSqrt),
                operands,
            } => operands.first().copied(),
            _ => None,
        })
        .collect()
}

/// Where byte `at` of a value of type `ty` is in its first element: an array's later element read as its first (each
/// element of an array of instance records is another instance's record, whose fields a binding states as the first
/// record's, at the first record's offsets), and the name of the member that holds it.
fn in_first_element(ty: &ConstantType, at: u32) -> (u32, String) {
    match ty {
        ConstantType::Array { element, stride, .. } if *stride > 0 => in_first_element(element, at % stride),
        ConstantType::Struct { members } => members
            .iter()
            .find(|m| m.offset <= at && at < m.offset + m.ty.size().unwrap_or(4))
            .map_or((at, String::new()), |m| {
                let (inner, name) = in_first_element(&m.ty, at - m.offset);
                (m.offset + inner, format!(".{}{name}", m.name))
            }),
        _ => (at, String::new()),
    }
}

/// The word of `layout` that states `word` ([`in_first_element`]) and the name of the field that holds it.
fn field_of(layout: &BufferLayout, buffer: &str, word: u32) -> (u32, String) {
    let byte = word * 4;
    layout
        .constants
        .iter()
        .find(|c| c.offset <= byte && byte < c.offset + c.ty.size().unwrap_or(4))
        .map_or_else(
            || (word, format!("{buffer} (no field)")),
            |c| {
                let (at, member) = in_first_element(&c.ty, byte - c.offset);
                ((c.offset + at) / 4, format!("{buffer}.{}{member}", c.name))
            },
        )
}

/// The field of `interface`'s constant buffer `buffer` that holds `word`, by name.
fn field_name(interface: &Interface, buffer: ResourceId, word: u32) -> String {
    match interface.resource(buffer) {
        Some(r) => match &r.kind {
            ResourceKind::ConstantBuffer { layout } => field_of(layout, &r.provenance.name, word).1,
            _ => r.provenance.name.clone(),
        },
        None => "?".into(),
    }
}

/// Whether `stage` states `word` of its program's buffer `buffer` (the stored interface's id, `layout` its layout): a
/// binding with a role other than `Unknown` covers it, or covers the word that holds its place in an array's first
/// element ([`in_first_element`]).
fn stated(stage: &StageBinding, layout: &BufferLayout, (buffer, word): Word) -> bool {
    let covers = |word: u32| {
        let byte = word * 4;
        stage.constants.iter().any(|c| {
            c.resource == buffer
                && c.offset <= byte
                && byte < c.offset + c.size
                && !matches!(c.role, ConstantRole::Unknown { .. })
        })
    };
    covers(word) || covers(field_of(layout, "", word).0)
}

/// What a program reads: each word it reads (the stored interface's), with its field's name and whether a divisor is
/// made of it.
struct Reads {
    words: BTreeMap<Word, (String, bool)>,
}

/// The words `shader`'s program reads ([`Reads`]).
fn reads(shader: &Shader, program: &str) -> Result<Reads> {
    let lifted = match shader.format {
        iridia_interface::ShaderFormat::Dxbc => iridia_dxbc::lift(&shader.bytecode)?,
        _ => iridia_dxil::lift(&shader.bytecode)?,
    };
    let graph = lifted.graph().map_err(|e| format!("{program}: {e}"))?;
    // the graph's buffers by their bind point, as the stored interface numbers them
    let stored = |id: ResourceId| -> Result<ResourceId> {
        let point = graph.interface.resource(id).and_then(|r| r.provenance.d3d);
        shader
            .interface
            .resources
            .iter()
            .find(|r| point.is_some() && r.provenance.d3d == point)
            .map(|r| r.id)
            .ok_or_else(|| format!("{program}: a buffer the stored interface lacks").into())
    };
    let values = &graph.values;
    let divided: BTreeSet<NodeId> = divisors(values)
        .into_iter()
        .flat_map(|d| made_of(values, d))
        .collect();
    let mut words = BTreeMap::new();
    for id in values.reached_from(values.roots()) {
        for (buffer, word) in read_words(values, &graph.interface, id) {
            let entry = words
                .entry((stored(buffer)?, word))
                .or_insert_with(|| (field_name(&graph.interface, buffer, word), false));
            entry.1 |= divided.contains(&id);
        }
    }
    Ok(Reads { words })
}

/// What `check` counts of an area: forward materials, programs, words read.
struct Checked {
    materials: usize,
    programs: usize,
    words: usize,
}

/// Every forward material of `world` named in `scope` checked: its stages' unstated words as failures, by area,
/// program, field and whether a divisor is made of them, with the words and one material that binds the program so.
fn check(
    world: &World,
    area: &str,
    scope: &BTreeSet<String>,
    failures: &mut BTreeMap<String, (BTreeSet<u32>, String)>,
) -> Result<Checked> {
    let mut programs: BTreeMap<Entity, (String, Reads)> = BTreeMap::new();
    let mut materials = 0;
    for (material, binding) in world.iter::<ShaderBinding>() {
        let Some(pixel) = binding.stage(Stage::Pixel) else {
            continue;
        };
        let forward = world
            .get::<Name>(pixel.shader)
            .is_some_and(|n| n.0.contains("_Fwd"));
        let name = world
            .get::<Name>(material)
            .map(|n| n.0.clone())
            .unwrap_or_default();
        if !forward || !scope.contains(&name) {
            continue;
        }
        materials += 1;
        for stage in [Stage::Vertex, Stage::Pixel] {
            let Some(bound) = binding.stage(stage) else {
                continue;
            };
            if let std::collections::btree_map::Entry::Vacant(e) = programs.entry(bound.shader) {
                let program = world
                    .get::<Name>(bound.shader)
                    .map(|n| n.0.clone())
                    .unwrap_or_default();
                let shader = world.get::<Shader>(bound.shader).ok_or("no shader")?;
                let read = reads(shader, &program)?;
                e.insert((program, read));
            }
            let (program, read) = &programs[&bound.shader];
            let shader = world.get::<Shader>(bound.shader).ok_or("no shader")?;
            for (&word, (field, divides)) in &read.words {
                let layout = shader.interface.resource(word.0).and_then(|r| match &r.kind {
                    ResourceKind::ConstantBuffer { layout } => Some(layout),
                    _ => None,
                });
                if !layout.is_some_and(|layout| stated(bound, layout, word)) {
                    let divides = if *divides { ", a divisor is made of it" } else { "" };
                    failures
                        .entry(format!("{area}: {program}: {field}{divides}"))
                        .or_insert_with(|| (BTreeSet::new(), name.clone()))
                        .0
                        .insert(word.1);
                }
            }
        }
    }
    let words = programs.values().map(|p| p.1.words.len()).sum();
    println!(
        "{area}: {materials} forward materials, {} programs, {words} words read; programs: {:?}",
        programs.len(),
        programs.values().map(|p| &p.0).collect::<BTreeSet<_>>()
    );
    Ok(Checked {
        materials,
        programs: programs.len(),
        words,
    })
}

/// The names of the materials `world` binds.
fn material_names(world: &World) -> BTreeSet<String> {
    world
        .iter::<ShaderBinding>()
        .filter_map(|(material, _)| world.get::<Name>(material).map(|n| n.0.clone()))
        .collect()
}

#[test]
fn every_word_a_sky_program_reads_is_stated() -> Result<()> {
    let (Some(sekiro_root), Some(elden_root), Some(elden_keys), Some(dll)) = (
        inputs::install(inputs::Game::Sekiro),
        inputs::install(inputs::Game::Elden),
        inputs::keys(inputs::Game::Elden),
        inputs::oodle(inputs::Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let Some(sekiro_install) = common::sekiro(&sekiro_root)? else {
        return Ok(());
    };
    let elden_install = common::install(
        elden_root,
        Game::EldenRing,
        Keys::from_folder(elden_keys, Game::EldenRing, Game::EldenRing.param_key_sha256())?,
    );
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let mut failures = BTreeMap::new();
    for (install, area) in [(&sekiro_install, "m11_01"), (&elden_install, "m60_42_36_00")] {
        let read = |sky| -> Result<World> {
            let mut world = World::default();
            sekiro::read_game_area(
                install,
                area,
                &decoder,
                &sekiro::Options {
                    sky,
                    ..Default::default()
                },
                &mut world,
            )?;
            common::run(&mut world, "resolve-fromsoft-shaders{unbound-textures=preview}")?;
            Ok(world)
        };
        let world = read(sekiro::sky::Sky::Include)?;
        // the sky's materials: those the read with the sky binds and the read without it does not
        let without = material_names(&read(sekiro::sky::Sky::None)?);
        let sky: BTreeSet<String> = material_names(&world).difference(&without).cloned().collect();
        let checked = check(&world, area, &sky, &mut failures)?;
        // the sky is drawn forward in both games: a read that found none of its programs checked nothing
        assert!(
            checked.materials > 0 && checked.programs > 0 && checked.words > 0,
            "{area}: {} forward materials, {} programs, {} words",
            checked.materials,
            checked.programs,
            checked.words
        );
    }
    let lines: Vec<String> = failures
        .iter()
        .map(|(at, (words, material))| {
            let (first, last) = (words.first().unwrap_or(&0), words.last().unwrap_or(&0));
            format!(
                "{at}: {} words unstated ({first}..={last}), e.g. {material}",
                words.len()
            )
        })
        .collect();
    assert!(
        lines.is_empty(),
        "{} fields with unstated words:\n{}",
        lines.len(),
        lines.join("\n")
    );
    Ok(())
}
