//! What every program of the shader database that reads lane x of `globalScalars` does with it.
//!
//! The lane is a word of the engine's frame constants (row 12 of the buffer the pixel programs bind). For every program
//! (one per distinct bytecode hash: programs of different effects share names, so names count nothing) whose value graph
//! reaches the word from an output lane, a discard's condition or a write, the shortest chain of operations from the word to
//! each such place is its *path* (`x>mul>T0.w`: the word multiplied into target 0's w; `x>sat>mul(7)>cvt>or>T0.w`:
//! clamped, times 7, converted and or-ed into it). The programs are counted by their set of paths, grouped by stage and by
//! the family a program's name starts with. The counts are printed (`USE stage [paths]: n programs (families)`).
//!
//! Each program is also put in one of six categories by where the word goes, and those counts are asserted: they are what
//! `ObjectParameter::Fade` and the binding's doc state (`engine_parts`). Programs of the three groups that are not an alpha
//! lane were read in their lifted text: the 27 `PS_LinearDepth` compare `x * alpha - ref` and steer the depth output (an
//! alpha test); the 9 `PS_Textured*` programs write `lerp(scene colour, lit colour, x * k)` with alpha 1 (a manual blend);
//! the 14 `cs_mirror_reflection*` compute programs store the word as the alpha of the reflection image.
//!
//! Needs `installs.gta` and `keys.gta`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod inputs;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use inputs::{Game, install, keys};
use iridia::flow::{At, BinaryOperator, MathFunction, Node, NodeId, Op, Values};
use primase_core::World;
use primase_core::shader::Shader;
use primase_gta::components::{GtaEffect, GtaShaderTable};
use primase_gta::passes::resolve_shaders::value_bytes;
use primase_gta::read::source::GtaSource;

/// What a step of a path is called.
fn step(values: &Values, id: NodeId) -> Option<String> {
    Some(match values.node(id) {
        Node::Op { op, operands } => match op {
            Op::Binary(BinaryOperator::Multiply) => {
                let literals: Vec<String> = operands
                    .iter()
                    .filter_map(|&o| match values.node(o) {
                        Node::Literal(l) => Some(format!("{}", l.0)),
                        _ => None,
                    })
                    .collect();
                if literals.is_empty() {
                    "mul".into()
                } else {
                    format!("mul({})", literals.join(","))
                }
            }
            Op::Binary(BinaryOperator::Add) => "add".into(),
            Op::Binary(BinaryOperator::Subtract) => "sub".into(),
            Op::Binary(BinaryOperator::Divide) => "div".into(),
            Op::Binary(
                BinaryOperator::Less
                | BinaryOperator::LessEqual
                | BinaryOperator::Greater
                | BinaryOperator::GreaterEqual
                | BinaryOperator::Equal
                | BinaryOperator::NotEqual,
            ) => "cmp".into(),
            Op::Binary(b) => format!("{b:?}").to_lowercase(),
            Op::Unary(u) => format!("{u:?}").to_lowercase(),
            Op::Math(MathFunction::Saturate) => "sat".into(),
            Op::Math(m) => format!("{m:?}").to_lowercase(),
            Op::Relational(r) => format!("{r:?}").to_lowercase(),
            Op::Select => "sel".into(),
            Op::Convert(_) => "cvt".into(),
            Op::Derivative(..) => "ddx".into(),
            Op::Index => "index".into(),
            Op::Subgroup => "subgroup".into(),
        },
        Node::Texel { .. } => "texel".into(),
        // a branch's arms and a loop's carried value pass the value on
        Node::Merge { .. } | Node::Loop { .. } => return None,
        _ => "?".into(),
    })
}

/// The nodes a node's value comes from, each marked when it only decides which value (a condition).
fn sources(values: &Values, id: NodeId) -> Vec<(NodeId, bool)> {
    match values.node(id) {
        Node::Merge { condition, arms } => condition
            .iter()
            .map(|&c| (c, true))
            .chain(arms.iter().map(|&a| (a, false)))
            .collect(),
        Node::Loop { arms, exit } => exit
            .iter()
            .map(|&c| (c, true))
            .chain(arms.iter().map(|&a| (a, false)))
            .collect(),
        Node::Op {
            op: Op::Select,
            operands,
        } => operands.iter().enumerate().map(|(i, &o)| (o, i == 0)).collect(),
        Node::Texel {
            coordinate, controls, ..
        } => coordinate.iter().chain(controls).map(|&o| (o, true)).collect(),
        _ => values.operands(id).into_iter().map(|o| (o, false)).collect(),
    }
}

/// The shortest chain of operations from `root` down to `target`, outermost first, or `None` where `root` is not made
/// of it. A step through a condition (a select's, a merge's, a texel's coordinate) is written `ctl:`.
fn path(values: &Values, root: NodeId, target: NodeId) -> Option<Vec<String>> {
    let mut parent: BTreeMap<NodeId, (NodeId, bool)> = BTreeMap::new();
    let mut queue = VecDeque::from([root]);
    let mut seen = BTreeSet::from([root]);
    while let Some(id) = queue.pop_front() {
        if id == target {
            let mut chain = Vec::new();
            let mut at = id;
            while at != root {
                let (up, control) = parent[&at];
                if let Some(s) = step(values, up) {
                    chain.push(if control { format!("ctl:{s}") } else { s });
                }
                at = up;
            }
            // `chain` runs from the word's consumer up to the root, whose own step is the last
            return Some(chain);
        }
        for (next, control) in sources(values, id) {
            if seen.insert(next) {
                parent.insert(next, (id, control));
                queue.push_back(next);
            }
        }
    }
    None
}

/// The family of a program name: its leading words, up to the hash of the effect.
fn family(name: &str) -> String {
    let words: Vec<&str> = name.split('_').collect();
    let cut = words
        .iter()
        .position(|w| w.len() == 8 && w.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or(words.len());
    words[..cut].join("_")
}

#[test]
fn what_the_programs_do_with_global_scalars_x() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).unwrap();
    let mut world = World::new();
    source.add_shader_database(&mut world).unwrap();
    let mut seen = BTreeSet::new();
    let mut hashes = BTreeSet::new();
    // (stage, paths) -> family -> distinct programs
    let mut census: BTreeMap<(String, String), BTreeMap<String, usize>> = BTreeMap::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut categories: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, effect) in world.iter::<GtaEffect>() {
        for p in &effect.programs {
            let Some(e) = p.shader else { continue };
            if !seen.insert(e) {
                continue;
            }
            let (Some(sh), Some(table)) = (
                world.get::<Shader>(e),
                world.get::<GtaShaderTable>(e).map(|t| &t.0),
            ) else {
                continue;
            };
            let hash = iridia::source_hash(&sh.bytecode);
            if !hashes.insert(hash.clone()) {
                continue;
            }
            *counts.entry("programs".into()).or_default() += 1;
            let mut word = None;
            for (i, r) in table.resources.iter().enumerate() {
                if r.constants == 0 {
                    continue;
                }
                for c in table.constants_of(i) {
                    if table.name_of(c.name) == Some("globalScalars") && value_bytes(c.flags).is_some() {
                        word = Some((u32::from(r.space), u32::from(r.register), u32::from(c.offset) / 4));
                    }
                }
            }
            let Some((space, register, first)) = word else {
                continue;
            };
            *counts
                .entry("programs declaring globalScalars".into())
                .or_default() += 1;
            let Ok(lifted) = iridia_dxil::lift(&sh.bytecode) else {
                *counts.entry("refused: not lifted".into()).or_default() += 1;
                continue;
            };
            let Ok(graph) = lifted.graph() else {
                *counts.entry("refused: no value graph".into()).or_default() += 1;
                continue;
            };
            let Some(buffer) = graph.reads.buffer_at(space, register) else {
                continue;
            };
            if !graph.reads.reads(buffer, first * 4, 4) {
                continue;
            }
            let values = &graph.values;
            let Some(target) = values.reached_from(values.roots()).into_iter().find(|&id| {
                matches!(values.node(id), Node::Global { resource: Some(r), at: At::Component(w) } if *r == buffer && *w == first)
            }) else {
                continue;
            };
            let stage = format!("{:?}", p.stage);
            *counts
                .entry(format!("{stage} programs reading the word"))
                .or_default() += 1;
            let mut paths = BTreeSet::new();
            for (&(output, lane), &id) in values.outputs() {
                if let Some(chain) = path(values, id, target) {
                    let v = &graph.interface.outputs[output];
                    let label = match v.system {
                        Some(iridia::SystemValue::Target(n)) => format!("T{n}"),
                        _ => format!("{}{}", v.label.name, v.label.index),
                    };
                    paths.insert(format!(
                        "x>{}>{label}.{}",
                        chain.into_iter().collect::<Vec<_>>().join(">"),
                        "xyzw".as_bytes()[usize::from(lane)] as char
                    ));
                }
            }
            for conditions in values.discards() {
                for &c in conditions {
                    if let Some(chain) = path(values, c, target) {
                        paths.insert(format!(
                            "x>{}>discard",
                            chain.into_iter().collect::<Vec<_>>().join(">")
                        ));
                    }
                }
            }
            for write in values.writes() {
                for &w in write {
                    if let Some(chain) = path(values, w, target) {
                        paths.insert(format!(
                            "x>{}>write",
                            chain.into_iter().collect::<Vec<_>>().join(">")
                        ));
                    }
                }
            }
            let ends: Vec<&str> = paths.iter().map(|p| p.rsplit('>').next().unwrap_or("")).collect();
            let category = if stage == "Compute" {
                "compute: the reflection image's alpha"
            } else if ends
                .iter()
                .any(|e| (e.starts_with('T') && e.ends_with(".w")) || *e == "discard")
            {
                "pixel: an alpha lane or an alpha-test discard"
            } else if ends.iter().all(|e| *e == "T1.z") {
                "pixel: the emission of the reflection G-buffer, which has no alpha"
            } else if ends.iter().all(|e| *e == "SV_Depth0.x") {
                "pixel: the depth of a depth-based alpha test"
            } else if ends.iter().all(|e| matches!(*e, "T0.x" | "T0.y" | "T0.z")) {
                "pixel: the mix between the surface and the scene behind it, in the colour lanes"
            } else {
                "other"
            };
            *categories.entry(category).or_default() += 1;
            *census
                .entry((stage, paths.into_iter().collect::<Vec<_>>().join("; ")))
                .or_default()
                .entry(family(&p.name))
                .or_default() += 1;
        }
    }
    println!("{counts:?}");
    println!("{categories:#?}");
    for ((stage, paths), families) in &census {
        let total: usize = families.values().sum();
        let mut f: Vec<_> = families.iter().collect();
        f.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        let f: Vec<String> = f.iter().take(5).map(|(n, c)| format!("{n}:{c}")).collect();
        println!("USE {stage} [{paths}]: {total} programs ({})", f.join(" "));
    }
    // the uses the role's meaning rests on: every program that reads the word, by where its value goes
    let expect = |key: &str, n: usize| assert_eq!(categories.get(key).copied().unwrap_or(0), n, "{key}");
    expect("pixel: an alpha lane or an alpha-test discard", 4_224);
    expect(
        "pixel: the emission of the reflection G-buffer, which has no alpha",
        112,
    );
    expect("pixel: the depth of a depth-based alpha test", 27);
    expect(
        "pixel: the mix between the surface and the scene behind it, in the colour lanes",
        9,
    );
    expect("compute: the reflection image's alpha", 14);
    expect("other", 0);
    // no vertex program reads it
    assert_eq!(counts.get("Pixel programs reading the word"), Some(&4_372));
    assert_eq!(counts.get("Compute programs reading the word"), Some(&14));
    assert!(counts.keys().all(|k| !k.starts_with("Vertex")));
}
