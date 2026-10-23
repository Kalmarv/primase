#![allow(clippy::unwrap_used)]
//! The decoder and the executor against Microsoft's tools: the instruction streams, declarations and immediate constant
//! buffers of real programs equal the D3D Shader Disassembler's, and the executor's outputs for the programs it takes equal the
//! reference values recorded with each program. Oracles: Microsoft's fxc/D3D disassembly text and recorded reference outputs;
//! nothing here is ours.
use std::collections::BTreeMap;

use primase_dxbc::decode::{Declared, Decoded};
use primase_dxbc::{decode, lift};
use primase_shader_exec::ProgramShader;
use primase_shader_exec::{PixelShader, Sample};

mod common;

/// The decoded program of a container.
fn decoded(bytes: &[u8]) -> Result<Decoded, String> {
    let parts = primase_dxcontainer::chunks(bytes).map_err(|e| e.to_string())?;
    let code = parts
        .iter()
        .find(|c| matches!(&c.kind, b"SHDR" | b"SHEX"))
        .ok_or("no shader code")?;
    let words: Vec<u32> = code
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|w| u32::from_le_bytes(*w))
        .collect();
    decode(&words).map_err(|e| e.to_string())
}

/// The buffers as the executor takes them: each as long as the shader says.
fn refs_of<'a>(shader: &dyn PixelShader, buffers: &'a [(u32, Vec<[u32; 4]>)]) -> Vec<(u32, &'a [[u32; 4]])> {
    shader
        .constant_buffers()
        .iter()
        .filter_map(|&(slot, rows)| {
            let (_, v) = buffers.iter().find(|(s, _)| *s == slot)?;
            Some((slot, &v[..(rows as usize).min(v.len())]))
        })
        .collect()
}

/// The lifter's refusals over `oracles.corpus.general/dxbc` (Sekiro's 390 pixel programs with fxc 10.0.26100's
/// disassembly), by reason with its numbers as N, measured on that corpus: more of a reason, or a new one, fails.
const PINNED_REFUSALS: &[(&str, usize)] = &[];

/// Optional directory of the game's ps_4_0, ps_4_1 and ps_5_0 binaries and Microsoft's disassembly of each (`.asm`
/// appended): every program has the disassembly's executable instructions, `ret` included, in order. Where the
/// program has recorded state, the executor's outputs equal the recorded ones to 4 f32 epsilon (the reference ran
/// the same sample for every texture sample: a constant). A program the lifter or the executor refuses is counted by
/// reason, not failed.
#[test]
fn independent_disassembly_counts() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = primase_testdata::oracle(
        primase_testdata::OracleSet::Corpus,
        primase_testdata::Game::General,
        "dxbc",
    ) else {
        return Ok(());
    };
    let mut count = 0;
    let (mut exact, mut within, mut nan, mut executed) = (0, 0, 0, 0);
    let mut refused: BTreeMap<String, usize> = BTreeMap::new();
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        if path.extension().is_none_or(|x| x != "ppo") {
            continue;
        }
        let asm_path = path.with_extension("ppo.asm");
        if !asm_path.exists() {
            continue;
        }
        let asm = std::fs::read_to_string(asm_path)?;
        let bytes = std::fs::read(&path)?;
        let program = decoded(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let instructions: Vec<_> = asm
            .lines()
            .skip_while(|l| !matches!(l.trim(), "ps_4_0" | "ps_4_1" | "ps_5_0"))
            .skip(1)
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with("dcl_"))
            .collect();
        assert_eq!(
            program.instructions.len(),
            instructions.len(),
            "{}",
            path.display()
        );
        for (a, b) in program.instructions.iter().zip(instructions) {
            let name = b.split_whitespace().next().unwrap();
            // an indexable resource's form (`sample_d_indexable(texture2d)(float,float,float,float)`) is its opcode's
            let name = name.split_once("_indexable").map_or(name, |(op, _)| op);
            let name = name.strip_suffix("_sat").unwrap_or(name);
            // a conditional's test (`if_nz`, `breakc_z`, `discard_nz`, `retc_z`) is its control, not its name
            let name = match name.rsplit_once('_') {
                Some((op @ ("if" | "breakc" | "continuec" | "discard" | "retc" | "callc"), "nz" | "z")) => op,
                _ => name,
            };
            assert_eq!(a.name(), name, "{} word {}", path.display(), a.word);
        }
        count += 1;
        let state_path = path.with_extension("ppo.state.json");
        if !state_path.exists() {
            continue;
        }
        let shader = match lift(&bytes)
            .map_err(|e| e.group())
            .and_then(|p| ProgramShader::new(p).map_err(|e| common::reason(&e.to_string())))
        {
            Ok(shader) => shader,
            Err(why) => {
                *refused.entry(why).or_default() += 1;
                continue;
            }
        };
        let j: serde_json::Value = serde_json::from_slice(&std::fs::read(state_path)?)?;
        let vectors = |j: &serde_json::Value| -> Vec<[u32; 4]> {
            j.as_array()
                .unwrap()
                .as_chunks::<4>()
                .0
                .iter()
                .map(|v| std::array::from_fn(|i| v[i].as_u64().unwrap() as u32))
                .collect()
        };
        let inputs = vectors(&j["inputs"]);
        let buffers: Vec<_> = j["buffers"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .filter(|(_, j)| !j.is_null())
            .map(|(i, j)| (i as u32, vectors(j)))
            .collect();
        let refs = refs_of(&shader, &buffers);
        // The optional `.ppo.trace.json` oracle diverges on documented cases (README, "Traced value-oracle
        // differences"), so trace equality is not a gate; the instruction stream and final outputs are.
        let result = shader
            .execute(&inputs, &refs, &mut |_: Sample| Ok([0.25, 0.5, 0.75, 1.0]))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        executed += 1;
        let expected = vectors(&j["outputs"]);
        for (i, a) in result.outputs.iter().enumerate() {
            for c in 0..4 {
                if result.output_masks[i] & (1 << c) == 0 {
                    continue;
                }
                let (x, y) = (f32::from_bits(a[c]), f32::from_bits(expected[i][c]));
                if a[c] == expected[i][c] {
                    exact += 1;
                } else if x.is_nan() && y.is_nan() {
                    nan += 1;
                } else {
                    assert!(
                        (x - y).abs() <= 4.0 * f32::EPSILON * y.abs().max(1.0),
                        "{} o{i}.{c}: {x} != {y}",
                        path.display()
                    );
                    within += 1;
                }
            }
        }
    }
    assert!(count > 0, "no paired shaders");
    println!(
        "independent bytecode/disassembly instruction streams matched: {count}; executed against the recorded outputs: {executed}"
    );
    for (why, n) in &refused {
        println!("  not executed ({n}): {why}");
    }
    common::hold_to_pins(&refused, PINNED_REFUSALS);
    println!(
        "constant-sampler value oracle: {exact} exact, {within} within 4 f32 epsilon at max(1, abs(reference)), {nan} NaN pairs"
    );
    Ok(())
}

/// An instruction's name as primase's table spells it, from Microsoft's disassembly: the resource and return-type
/// annotation and `_indexable` dropped, the immediate offset (`_aoffimmi(1,0,0)`, an extended token of the instruction the
/// decoder keeps apart) dropped, saturation dropped, a conditional's test dropped.
fn disassembled_name(token: &str) -> &str {
    let name = token.split('(').next().unwrap_or(token);
    let name = name.strip_suffix("_indexable").unwrap_or(name);
    let name = name.strip_suffix("_aoffimmi").unwrap_or(name);
    let name = name.strip_suffix("_sat").unwrap_or(name);
    match name {
        "if_nz" | "if_z" => "if",
        "breakc_nz" | "breakc_z" => "breakc",
        "continuec_nz" | "continuec_z" => "continuec",
        "discard_nz" | "discard_z" => "discard",
        "retc_nz" | "retc_z" => "retc",
        // spelled without its underscore
        "ldms" => "ld_ms",
        _ => name,
    }
}

/// The texel offset the disassembly prints for an instruction (`sample_aoffimmi_indexable(1,-1,0)(texture2d)(..)`), if it
/// prints one.
fn disassembled_offset(token: &str) -> Option<[i32; 3]> {
    let at = token.find("_aoffimmi")?;
    let group = token[at..].split_once('(')?.1.split_once(')')?.0;
    let v: Vec<i32> = group.split(',').map(|x| x.trim().parse().unwrap()).collect();
    Some([v[0], v[1], v[2]])
}

/// A declared resource (`t`) or view (`u`): its slot, its shape as the disassembler names it, and its return type
/// (`float`, `uint`, `sint`, or `mixed` for a structured one with its stride).
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Resource {
    slot: u32,
    shape: String,
    returns: String,
}

/// A declared resource (`t`) or view (`u`) from a `dcl_resource_*` or `dcl_uav_*` line.
fn disassembled_declaration(line: &str) -> Option<(char, Resource)> {
    let (kind, rest) = if let Some(r) = line.strip_prefix("dcl_resource_") {
        ('t', r)
    } else if let Some(r) = line.strip_prefix("dcl_uav_typed_") {
        ('u', r)
    } else {
        ('u', line.strip_prefix("dcl_uav_")?)
    };
    let register = |s: &str| s.trim().trim_start_matches(kind).parse::<u32>().unwrap();
    if let Some(r) = rest.strip_prefix("structured") {
        let (slot, stride) = r.trim_start_matches("_glc").trim().split_once(',').unwrap();
        return Some((
            kind,
            Resource {
                slot: register(slot),
                shape: format!("structured {}", stride.trim()),
                returns: "mixed".into(),
            },
        ));
    }
    let (shape, rest) = rest.split_once(' ').unwrap();
    let (types, slot) = rest.rsplit_once(' ').unwrap();
    let returns = types.trim_matches(['(', ')']);
    Some((
        kind,
        Resource {
            slot: register(slot),
            shape: shape.trim_end_matches("_glc").to_owned(),
            returns: returns.to_owned(),
        },
    ))
}

/// The resources and views a decoded program declares, in the disassembler's vocabulary.
fn declared(program: &Decoded) -> (Vec<Resource>, Vec<Resource>) {
    // D3D_RETURN_TYPE, one nibble per lane, as the disassembler names them
    let returns = |token: u32| {
        (0..4)
            .map(|lane| {
                [
                    "?",
                    "unorm",
                    "snorm",
                    "sint",
                    "uint",
                    "float",
                    "mixed",
                    "double",
                    "continued",
                    "unused",
                ][((token >> (4 * lane)) & 15) as usize]
            })
            .collect::<Vec<_>>()
            .join(",")
    };
    // D3D_SRV_DIMENSION as the disassembler names it; a multisampled one states its sample count
    let shape = |dimension: u32, samples: u32| match dimension {
        1 => "buffer".to_owned(),
        2 => "texture1d".to_owned(),
        3 => "texture2d".to_owned(),
        4 => format!("texture2dms({samples})"),
        5 => "texture3d".to_owned(),
        6 => "texturecube".to_owned(),
        7 => "texture1darray".to_owned(),
        8 => "texture2darray".to_owned(),
        9 => format!("texture2dmsarray({samples})"),
        10 => "texturecubearray".to_owned(),
        other => panic!("a shape the disassembly test doesn't name: {other}"),
    };
    let (mut resources, mut views) = (Vec::new(), Vec::new());
    for d in &program.declarations {
        match &d.kind {
            Declared::Resource {
                slot,
                dimension,
                samples,
                token,
                ..
            } => resources.push(Resource {
                slot: *slot,
                shape: shape(*dimension, *samples),
                returns: returns(*token),
            }),
            Declared::ResourceStructured { slot, stride } => resources.push(Resource {
                slot: *slot,
                shape: format!("structured {stride}"),
                returns: "mixed".into(),
            }),
            Declared::UavTyped {
                slot,
                dimension,
                token,
                ..
            } => views.push(Resource {
                slot: *slot,
                shape: shape(*dimension, 0),
                returns: returns(*token),
            }),
            Declared::UavStructured { slot, stride, .. } => views.push(Resource {
                slot: *slot,
                shape: format!("structured {stride}"),
                returns: "mixed".into(),
            }),
            _ => {}
        }
    }
    (resources, views)
}

/// The immediate constant buffer's values as the disassembler prints them, row by row.
fn disassembled_immediates(text: &str) -> Vec<[String; 4]> {
    let Some(at) = text.find("dcl_immediateConstantBuffer") else {
        return Vec::new();
    };
    // the outer braces' contents: up to the brace closing the first one
    let block = &text[at..];
    let mut depth = 0;
    let end = block
        .char_indices()
        .find(|&(_, c)| {
            depth += match c {
                '{' => 1,
                '}' => -1,
                _ => 0,
            };
            c == '}' && depth == 0
        })
        .unwrap()
        .0;
    block[block.find('{').unwrap() + 1..end]
        .split('{')
        .filter_map(|row| {
            let row = row.split('}').next()?.trim();
            (!row.is_empty()).then(|| {
                let v: Vec<String> = row.split(',').map(|x| x.trim().to_owned()).collect();
                std::array::from_fn(|i| v[i].clone())
            })
        })
        .collect()
}

/// Every pixel container of `oracles.d3d-disassembler.riders/shader-pack` (`NNNN.dxbc`, each with Microsoft's D3D
/// Shader Disassembler text `NNNN.txt`): its checksum verified, then its instruction stream, its declared resources and
/// views, and its immediate constant buffer equal to the disassembly's (each value printed as the disassembler prints it).
#[test]
fn pack_against_microsoft_disassembly() {
    let Some(root) = primase_testdata::oracle(
        primase_testdata::OracleSet::D3dDisassembler,
        primase_testdata::Game::Riders,
        "shader-pack",
    ) else {
        return;
    };
    let mut read = 0;
    let mut paths: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "dxbc"))
        .collect();
    paths.sort();
    for path in paths {
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            primase_dxcontainer::checksum(&bytes).unwrap()[..],
            bytes[4..20],
            "{}",
            path.display()
        );
        let text = String::from_utf8(std::fs::read(path.with_extension("txt")).unwrap())
            .unwrap()
            .replace('\0', "");
        let lines: Vec<&str> = text.lines().map(str::trim).collect();
        let Some(start) = lines.iter().position(|l| *l == "ps_5_0") else {
            continue;
        };
        let program = decoded(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let body = &lines[start + 1..];
        let names: Vec<&str> = body
            .iter()
            .filter(|l| {
                !l.is_empty() && !l.starts_with("//") && !l.starts_with("dcl_") && !l.starts_with('{')
            })
            .map(|l| disassembled_name(l.split_whitespace().next().unwrap()))
            .collect();
        let ours: Vec<&str> = program.instructions.iter().map(|i| i.name()).collect();
        assert_eq!(ours, names, "{}", path.display());
        // and the offsets the instructions carry are the ones the disassembly prints
        let offsets: Vec<Option<[i32; 3]>> = body
            .iter()
            .filter(|l| {
                !l.is_empty() && !l.starts_with("//") && !l.starts_with("dcl_") && !l.starts_with('{')
            })
            .map(|l| disassembled_offset(l.split_whitespace().next().unwrap()))
            .collect();
        let ours: Vec<Option<[i32; 3]>> = program.instructions.iter().map(|i| i.sample_offset()).collect();
        assert_eq!(ours, offsets, "{}", path.display());
        let (mut resources, mut views) = (Vec::new(), Vec::new());
        for (kind, d) in body.iter().filter_map(|l| disassembled_declaration(l)) {
            if kind == 't' {
                resources.push(d);
            } else {
                views.push(d);
            }
        }
        let (mut ours_resources, mut ours_views) = declared(&program);
        for v in [&mut resources, &mut views, &mut ours_resources, &mut ours_views] {
            v.sort();
        }
        assert_eq!(ours_resources, resources, "{}", path.display());
        assert_eq!(ours_views, views, "{}", path.display());
        let immediates = program
            .declarations
            .iter()
            .find_map(|d| match &d.kind {
                Declared::ImmediateBuffer(rows) => Some(rows.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let printed: Vec<[String; 4]> = immediates
            .iter()
            .map(|row| {
                row.map(|bits| {
                    let f = f32::from_bits(bits);
                    // a lane whose bits are a normal float is printed as one, the others as the integer
                    if f.is_normal() {
                        format!("{f:.6}")
                    } else {
                        (bits as i32).to_string()
                    }
                })
            })
            .collect();
        assert_eq!(printed, disassembled_immediates(&text), "{}", path.display());
        read += 1;
    }
    println!("pixel containers read, matching the disassembly: {read}");
    assert!(read > 0);
}
