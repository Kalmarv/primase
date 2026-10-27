//! Independently RUN RPCS3 disassembly, indexed by raw microcode hash. Repeated
//! packaged programs are byte-compared to an already checked distinct program.
mod inputs;

use inputs::{Depth, Game, OracleSet, Query, depth, expect_found, install, oracle, scratch, seed};
use primase_shred::read::pak::Pak;
use rsx_reader::{
    cgbinary::{Stage, parse_prefix},
    fragment::{self, Control, Register, SourceKind, flow},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    path::{Path, PathBuf},
};

fn archives(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let p = entry.expect("directory entry").path();
        if p.is_dir() {
            archives(&p, out);
        } else if p.extension().is_some_and(|s| s.eq_ignore_ascii_case("pak")) {
            out.push(p);
        }
    }
}
fn swizzle(s: [u8; 4]) -> String {
    let mut text: String = s
        .into_iter()
        .map(|i| char::from(b"xyzw"[usize::from(i)]))
        .collect();
    if s == [0, 1, 2, 3] {
        text.clear();
    } else if s.iter().all(|&c| c == s[0]) {
        text.truncate(1);
    }
    if text.is_empty() { text } else { format!(".{text}") }
}

// This normalizes only RPCS3's condition spelling, keeping the known ambiguity
// visible in a counted discrepancy. The library uses NV test names: 0 FL,
// 2 EQ, 7 TR. Original TaskFP spells those TR, FL, GE respectively.
fn oracle_condition(p: fragment::Predicate) -> String {
    let name = match p.test {
        0 => "TR",
        2 => "FL",
        7 => "GE",
        _ => p.condition(),
    };
    format!("{name}{}", swizzle(p.swizzle))
}

fn compare(p: &fragment::Program, reference: &Value) -> (usize, usize, usize) {
    let mut constant_nops = 0;
    let expected: Vec<_> = reference["instructions"]
        .as_array()
        .expect("instruction array")
        .iter()
        .filter(|e| {
            let at = e["offset"].as_u64().expect("offset") as usize;
            if p.instructions.iter().any(|i| i.offset == at) {
                return true;
            }
            // Confirmed TaskFP bug: an unprinted sampling operand owns an inline
            // constant slot which the tool visits as NOP. Keep the original oracle
            // intact, validate this exact explanation, and count the discrepancy.
            let owner = p
                .instructions
                .iter()
                .find(|i| i.constant.is_some() && i.offset + 16 == at)
                .expect("unexplained extra oracle instruction");
            let printed = match owner.opcode {
                fragment::Opcode::TXL | fragment::Opcode::TXB => 1,
                fragment::Opcode::TXD => 2,
                _ => panic!("unexpected oracle constant instruction"),
            };
            assert!(
                owner.sources[..printed]
                    .iter()
                    .all(|s| s.kind != SourceKind::Constant)
            );
            assert_eq!(owner.sources[printed].kind, SourceKind::Constant);
            assert_eq!(e["opcode"], "NOP");
            assert_eq!(e["end"], false);
            assert!(e["sources"].as_array().expect("sources").is_empty());
            constant_nops += 1;
            false
        })
        .collect();
    assert_eq!(p.instructions.len(), expected.len());
    let mut condition_discrepancies = 0;
    let mut omitted_sources = 0;
    for (i, e) in p.instructions.iter().zip(expected) {
        assert_eq!(e["opcode"], i.opcode.mnemonic(), "offset {}", i.offset);
        assert_eq!(e["offset"], i.offset);
        assert_eq!(e["end"], i.end);
        let destination = &e["destination"];
        if i.opcode.writes_destination() {
            assert_eq!(destination["index"], i.destination.index);
            assert_eq!(destination["mask"], i.write_mask);
            if i.destination.index != 63 {
                assert_eq!(destination["half"], i.destination.half);
            }
        } else {
            assert!(destination.is_null());
        }
        assert_eq!(e["unit"], i.texture_unit.map(Value::from).unwrap_or(Value::Null));
        let sources = e["sources"].as_array().expect("sources");
        let omitted = match i.opcode {
            fragment::Opcode::TXL | fragment::Opcode::TXB | fragment::Opcode::TXD => 1,
            _ => 0,
        };
        omitted_sources += omitted;
        assert_eq!(
            sources.len() + omitted,
            i.sources.len(),
            "{:?} at {}",
            i.opcode,
            i.offset
        );
        for (s, e) in i.sources.iter().zip(sources) {
            assert_eq!(e["negate"], s.negate);
            assert_eq!(e["absolute"], s.absolute);
            assert_eq!(e["swizzle"], serde_json::json!(s.swizzle));
            match s.kind {
                SourceKind::Register(r) => {
                    assert_eq!(e["kind"], "register");
                    assert_eq!(e["index"], r.index);
                    assert_eq!(e["half"], r.half);
                }
                SourceKind::Input(n) => {
                    assert_eq!(e["kind"], "input");
                    assert_eq!(e["index"], n);
                    assert_eq!(e["perspective"], i.perspective_input);
                }
                SourceKind::Constant => {
                    assert_eq!(e["kind"], "constant");
                    assert_eq!(
                        e["value"],
                        serde_json::json!(i.constant.expect("inline constant"))
                    );
                }
            }
        }
        if let Some(text) = e["condition"].as_str() {
            assert_eq!(text, oracle_condition(i.predicate));
            let correct = format!("{}{}", i.predicate.condition(), swizzle(i.predicate.swizzle));
            condition_discrepancies += usize::from(correct != text);
        }
    }
    let markers = reference["markers"].as_array().expect("markers");
    let mut expected_markers = Vec::new();
    for i in &p.instructions {
        if let Some(Control::If {
            else_offset,
            end_offset,
        }) = i.control
        {
            if else_offset < p.byte_len {
                expected_markers.push(("ELSE", else_offset));
            }
            if end_offset < p.byte_len {
                expected_markers.push(("ENDIF", end_offset));
            }
        }
    }
    let mut actual: Vec<(&str, usize)> = markers
        .iter()
        .map(|m| {
            (
                m["kind"].as_str().expect("marker"),
                m["offset"].as_u64().expect("offset") as usize,
            )
        })
        .collect();
    expected_markers.sort();
    actual.sort();
    assert_eq!(actual, expected_markers);
    (condition_discrepancies, omitted_sources, constant_nops)
}

#[derive(Debug)]
struct Checked {
    hash: String,
    analysis: flow::Analysis,
    instructions: usize,
    constants: usize,
    predicates: usize,
    omitted_sources: usize,
    constant_nops: usize,
}

#[test]
fn every_fragment_matches_independent_disassembly_and_reports_possible_texture_flow() {
    let Some(root) = install(Game::Shred) else { return };
    let Some(references) = oracle(OracleSet::Rpcs3, Game::Shred, "fragment") else {
        return;
    };
    let mut paths = Vec::new();
    archives(&root.join("PS3_GAME/USRDIR"), &mut paths);
    paths.sort();
    expect_found(Query::Install(Game::Shred, None), "PAKs", paths.len());
    let depth = depth();
    let seed = seed();
    if depth == Depth::Smoke {
        let n = paths.len();
        paths.rotate_left(seed as usize % n);
    }
    let mut checked: BTreeMap<Vec<u8>, Checked> = BTreeMap::new();
    let mut family_hashes = BTreeSet::new();
    let mut report = String::from(
        "family\tmicrocode_sha256\tunit\tcoordinate_inputs\tcoordinate_textures\toutput_components\tkill\tpossible_consumers\n",
    );
    let mut constants_report = String::from("family\tmicrocode_sha256\tconstant_byte_offset\tparameter\n");
    let mut totals = [0usize; 5];
    let outputs = [
        Register {
            index: 0,
            half: false,
        },
        Register {
            index: 2,
            half: false,
        },
        Register {
            index: 3,
            half: false,
        },
        Register {
            index: 4,
            half: false,
        },
        Register { index: 0, half: true },
        Register { index: 4, half: true },
        Register { index: 6, half: true },
        Register { index: 8, half: true },
        Register {
            index: 1,
            half: false,
        },
    ];
    for path in &paths {
        if depth == Depth::Smoke && totals[1] >= 8 {
            break;
        }
        totals[0] += 1;
        let pak = Pak::open(path).unwrap();
        let mut found = false;
        for (entry, e) in pak
            .entries()
            .iter()
            .enumerate()
            .filter(|(_, e)| e.name.ends_with(b".psp"))
        {
            found = true;
            totals[2] += 1;
            let data = pak.payload(entry).unwrap();
            let mut at = 0;
            while at < data.len() {
                let container = parse_prefix(&data[at..]).unwrap();
                let Stage::Fragment(stage) = &container.stage else {
                    panic!("fragment profile")
                };
                if !checked.contains_key(container.microcode) {
                    let hash: String = Sha256::digest(container.microcode)
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect();
                    let text = std::fs::read(references.join(format!("{hash}.json")))
                        .unwrap_or_else(|e| panic!("oracle {hash}: {e}"));
                    let reference: Value = serde_json::from_slice(&text).unwrap();
                    let program =
                        fragment::decode(container.microcode).unwrap_or_else(|e| panic!("{hash}: {e}"));
                    let (predicates, omitted_sources, constant_nops) = compare(&program, &reference);
                    let analysis =
                        flow::analyze(&program, &outputs).unwrap_or_else(|e| panic!("flow {hash}: {e}"));
                    let constants = program
                        .instructions
                        .iter()
                        .filter(|i| i.constant.is_some())
                        .count();
                    checked.insert(
                        container.microcode.to_vec(),
                        Checked {
                            hash,
                            analysis,
                            instructions: program.instructions.len(),
                            constants,
                            predicates,
                            omitted_sources,
                            constant_nops,
                        },
                    );
                }
                let p = &checked[container.microcode];
                totals[3] += 1;
                totals[4] += p.instructions;
                let family = String::from_utf8_lossy(&e.name).into_owned();
                if family_hashes.insert((
                    family.clone(),
                    p.hash.clone(),
                    stage.output_from_h0,
                    stage.depth_replace,
                )) {
                    for unit in 0..16 {
                        let samples: Vec<_> = p.analysis.samples.iter().filter(|s| s.unit == unit).collect();
                        if samples.is_empty() {
                            continue;
                        }
                        let inputs = samples.iter().fold(0u16, |bits, s| bits | s.coordinates.inputs);
                        let textures = samples.iter().fold(0u16, |bits, s| bits | s.coordinates.textures);
                        let mode = if stage.output_from_h0 { 4 } else { 0 };
                        let mut destinations = Vec::new();
                        for (mrt, output) in p.analysis.outputs[mode..mode + 4].iter().enumerate() {
                            let channels: String = output
                                .components
                                .iter()
                                .enumerate()
                                .filter(|(_, d)| d.textures & (1 << unit) != 0)
                                .map(|(c, _)| char::from(b"xyzw"[c]))
                                .collect();
                            if !channels.is_empty() {
                                destinations.push(format!("color{mrt}.{channels}"));
                            }
                        }
                        if stage.depth_replace
                            && p.analysis.outputs[8].components[2].textures & (1 << unit) != 0
                        {
                            destinations.push("depth.z".into());
                        }
                        let consumers = p.analysis.consumers[usize::from(unit)]
                            .iter()
                            .map(|op| op.mnemonic())
                            .collect::<Vec<_>>()
                            .join(",");
                        writeln!(
                            report,
                            "{family}\t{}\t{unit}\t{inputs:04x}\t{textures:04x}\t{}\t{}\t{consumers}",
                            p.hash,
                            destinations.join(","),
                            p.analysis.kill.textures & (1 << unit) != 0
                        )
                        .expect("string write");
                    }
                    for param in &container.parameters {
                        for offset in &param.embedded_constants {
                            let name = param
                                .name
                                .map(String::from_utf8_lossy)
                                .map(|s| s.into_owned())
                                .unwrap_or_default();
                            writeln!(constants_report, "{family}\t{}\t{offset}\t{name}", p.hash)
                                .expect("string write");
                        }
                    }
                }
                at = (at + container.bytes.len() + 63) & !63;
            }
            assert_eq!(at, data.len());
        }
        totals[1] += usize::from(found);
    }
    expect_found(
        Query::Oracle(OracleSet::Rpcs3, Game::Shred, "fragment"),
        "independent disassemblies compared",
        checked.len(),
    );
    let unique_instructions: usize = checked.values().map(|p| p.instructions).sum();
    let constants: usize = checked.values().map(|p| p.constants).sum();
    let discrepancies: usize = checked.values().map(|p| p.predicates).sum();
    let omitted: usize = checked.values().map(|p| p.omitted_sources).sum();
    let constant_nops: usize = checked.values().map(|p| p.constant_nops).sum();
    let out = scratch().join("shred-fragment-flow");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("units.tsv"), report).unwrap();
    std::fs::write(out.join("constants.tsv"), constants_report).unwrap();
    eprintln!(
        "SHRED fragment {depth:?} seed={seed}: PAKs={} shader_PAKs={} PSP={} occurrences={} instruction_occurrences={} distinct={} unique_instructions={unique_instructions} inline_constants={constants} known_oracle_condition_spelling_discrepancies={discrepancies} unprinted_sampling_sources={omitted} oracle_constants_printed_as_nop={constant_nops} report={}",
        totals[0],
        totals[1],
        totals[2],
        totals[3],
        totals[4],
        checked.len(),
        out.display()
    );
    if depth == Depth::Full {
        assert_eq!((totals[2], totals[3], checked.len()), (765, 181851, 3267));
    }
}
