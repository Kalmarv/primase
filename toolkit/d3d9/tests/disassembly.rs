#![allow(clippy::unwrap_used)]
//! Privately extracted shipped PS and VS programs compared with Microsoft D3DDisassemble (d3dcompiler_47.dll).
//! All executable operands, input/sampler declarations, immediate values and CTAB allocations are compared;
//! validation and WGSL emission check the resulting IR. This is reflection/token evidence, not a rendered-pixel oracle.
use primase_core::gate::{GateReport, Tolerance};
use primase_d3d9::{Destination, Opcode, Register, Source, lift, read};
use primase_testdata::{Game, OracleSet, oracle};

fn register(r: Register) -> String {
    match r {
        Register::Temporary(n) => format!("r{n}"),
        Register::Input(n) => format!("v{n}"),
        Register::Constant(n) => format!("c{n}"),
        Register::Sampler(n) => format!("s{n}"),
        Register::Target(n) => format!("oC{n}"),
        Register::Output(n) => format!("o{n}"),
    }
}
fn mask(mask: u8) -> String {
    if mask == 15 {
        return String::new();
    }
    format!(
        ".{}",
        (0..4)
            .filter(|n| mask & (1 << n) != 0)
            .map(|n| ['x', 'y', 'z', 'w'][n])
            .collect::<String>()
    )
}
fn destination(d: Destination) -> String {
    format!("{}{}", register(d.register), mask(d.mask))
}
fn source(s: Source) -> String {
    let mut text = register(s.register);
    if s.swizzle != [0, 1, 2, 3] {
        text.push('.');
        let lanes = if s.swizzle.iter().all(|&l| l == s.swizzle[0]) {
            &s.swizzle[..1]
        } else {
            &s.swizzle[..]
        };
        text.extend(lanes.iter().map(|n| ['x', 'y', 'z', 'w'][*n as usize]));
    }
    if s.absolute {
        text = format!("abs({text})");
    }
    if s.negate {
        text.insert(0, '-');
    }
    text
}
fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn check(bytes: &[u8], asm: &str) -> primase_d3d9::Shader {
    let shader = read(bytes).unwrap();
    let p = lift(bytes).unwrap();
    primase_shader::emit::wgsl(&p).unwrap();
    let mut gate = GateReport::new("raw SM3 vs Microsoft disassembly", Tolerance::Exact);
    let actual: Vec<_> = shader
        .instructions
        .iter()
        .map(|i| {
            let mut op = match i.opcode {
                Opcode::Tex => match i.control {
                    0 => "texld",
                    1 => "texldp",
                    _ => "texldb",
                }
                .into(),
                Opcode::TexKill => "texkill".into(),
                _ => format!("{:?}", i.opcode).to_ascii_lowercase(),
            };
            let mut args = Vec::new();
            if let Some(d) = i.destination {
                if d.saturate {
                    op.push_str("_sat");
                }
                if d.partial_precision {
                    op.push_str("_pp");
                }
                args.push(destination(d));
            }
            args.extend(i.sources.iter().copied().map(source));
            normalize(&format!("{op} {}", args.join(", ")))
        })
        .collect();
    let expected: Vec<_> = asm
        .lines()
        .map(str::trim)
        .filter(|s| {
            !s.is_empty()
                && !s.starts_with("//")
                && !s.starts_with("dcl_")
                && !s.starts_with("def ")
                && *s != "ps_3_0"
                && *s != "vs_3_0"
        })
        .map(normalize)
        .collect();
    gate.compare_bytes(
        "every executable opcode/operand",
        actual.join("\n").as_bytes(),
        expected.join("\n").as_bytes(),
    );
    const SEMANTICS: [&str; 14] = [
        "position",
        "blendweight",
        "blendindices",
        "normal",
        "psize",
        "texcoord",
        "tangent",
        "binormal",
        "tessfactor",
        "positiont",
        "color",
        "fog",
        "depth",
        "sample",
    ];
    for i in &shader.inputs {
        let decl = format!(
            "dcl_{}{}{} v{}{}",
            SEMANTICS[i.usage as usize],
            if i.index == 0 {
                String::new()
            } else {
                i.index.to_string()
            },
            if i.centroid { "_centroid" } else { "" },
            i.register,
            mask(i.mask)
        );
        assert!(asm.lines().any(|s| s.trim() == decl), "missing {decl}");
        gate.record(decl, 0.);
    }
    for o in &shader.outputs {
        let decl = format!(
            "dcl_{}{} o{}{}",
            SEMANTICS[o.usage as usize],
            if o.index == 0 {
                String::new()
            } else {
                o.index.to_string()
            },
            o.register,
            mask(o.mask)
        );
        assert!(asm.lines().any(|s| s.trim() == decl), "missing {decl}");
        gate.record(decl, 0.);
    }
    for (slot, dim) in &shader.samplers {
        let decl = format!(
            "dcl_{} s{slot}",
            match dim {
                primase_shader::TextureDim::D2 => "2d",
                primase_shader::TextureDim::D3 => "volume",
                _ => "cube",
            }
        );
        assert!(asm.lines().any(|s| s.trim() == decl), "missing {decl}");
        gate.record(decl, 0.);
    }
    let table = shader.constants.as_ref().unwrap();
    for c in &table.constants {
        let row = format!(
            "// {} {}{} {}",
            c.name,
            if c.register_set == 3 { "s" } else { "c" },
            c.register,
            c.count
        );
        assert!(
            asm.lines().any(|s| normalize(s) == row),
            "missing CTAB allocation {row}"
        );
        gate.record(row, 0.);
        let t = &c.ty;
        let ty = match (t.class, t.kind) {
            (0, 3) => "float".to_owned(),
            (1, 3) => format!("float{}", t.columns),
            (2, 3) => format!("row_major float{}x{}", t.rows, t.columns),
            (4, 12) => "sampler2D".to_owned(),
            _ => panic!("unhandled proof parameter type {t:?}"),
        };
        let array = if t.elements > 1 {
            format!("[{}]", t.elements)
        } else {
            String::new()
        };
        let declaration = format!("// {ty} {}{array};", c.name);
        assert!(
            asm.lines().any(|s| normalize(s) == declaration),
            "missing parameter {declaration}"
        );
        gate.record(declaration, 0.);
    }
    for (&n, values) in &shader.definitions {
        let prefix = format!("def c{n}, ");
        let line = asm
            .lines()
            .map(str::trim)
            .find_map(|s| s.strip_prefix(&prefix))
            .unwrap();
        let expected: Vec<_> = line
            .split(',')
            .map(|s| s.trim().parse::<f32>().unwrap().to_bits())
            .collect();
        assert_eq!(values.as_slice(), expected);
        gate.record(format!("def c{n}"), 0.);
    }
    gate.assert_passed();
    eprintln!("{gate}");
    shader
}

#[test]
fn shipped_pixel_shader_matches_microsofts_disassembly() {
    let Some(folder) = oracle(OracleSet::D3dDisassembler, Game::General, "sm3") else {
        return;
    };
    let bytes = std::fs::read(folder.join("pixel.bin")).unwrap();
    let asm = std::fs::read_to_string(folder.join("pixel.asm")).unwrap();
    assert_eq!(
        primase_shader::source_hash(&bytes),
        "sha256:82ffb69b8dced07b67e74ed481349f1c68ede096add5379c7b5749ea9b809c0e"
    );
    let shader = check(&bytes, &asm);
    assert_eq!(shader.instructions.len(), 30);
    assert_eq!(
        (
            shader.inputs.len(),
            shader.samplers.len(),
            shader.constants.unwrap().constants.len()
        ),
        (5, 2, 8)
    );
}

#[test]
fn shipped_vertex_shaders_match_microsofts_disassembly() {
    let Some(folder) = oracle(OracleSet::D3dDisassembler, Game::General, "sm3") else {
        return;
    };
    for (name, inputs, outputs, instructions) in [("vertex", 3, 6, 48), ("vertex-layered", 5, 8, 71)] {
        let bytes = std::fs::read(folder.join(format!("{name}.bin"))).unwrap();
        let asm = std::fs::read_to_string(folder.join(format!("{name}.asm"))).unwrap();
        let shader = check(&bytes, &asm);
        assert_eq!(shader.stage, primase_shader::Stage::Vertex);
        assert_eq!(
            (
                shader.inputs.len(),
                shader.outputs.len(),
                shader.instructions.len()
            ),
            (inputs, outputs, instructions)
        );
    }
}
