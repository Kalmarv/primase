#![allow(clippy::unwrap_used)]
//! The program executor on lifted DXIL against Microsoft's WARP rasterizer (Direct3D 12, Shader Model 6), the oracle
//! that is not ours: each case is rendered by WARP on one pixel with the same input registers and constant buffers,
//! and run on [`ProgramShader`]; every lane both write must agree bit for bit (the fixtures use no transcendental).
//!
//! The fixtures are the control-flow and memory ones the DXIL evaluator cannot run (flow, loops, switch, arrays, bits;
//! default and -Od builds). Needs `PRIMASE_WARP` (the oracle program), `PRIMASE_DXC` (for the oracle's vertex shader)
//! and `PRIMASE_WARP_SCRATCH` (a folder). The vertex shader is generated from the lifted program's own interface:
//! every input varying read from `cbuffer Case : register(b0, space1) { uint4 r[32]; }`, row by register.
use primase_shader::exec::ProgramShader;
use primase_shader::{InterpMode, InterpSampling, PixelShader, Program, Sample, ScalarType, SystemValue};
use primase_testdata::real_data;
use std::collections::BTreeMap;
use std::path::Path;

fn f(x: f32) -> u32 {
    x.to_bits()
}
fn hex(words: &[u32]) -> String {
    words
        .iter()
        .map(|w| format!("{w:08x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// HLSL of a vertex shader that feeds `program`'s pixel inputs: one clockwise triangle covering the pixel at the
/// origin of a one-pixel viewport (SV_Position (0.5, 0.5, 0.5, 1)), every other input read from the case buffer.
/// Elements are declared in signature order with the pixel program's types and interpolation so the compiler packs
/// them alike; system values the rasterizer generates (front face, sample index, ...) are left out.
fn vertex_hlsl(program: &Program) -> String {
    let lanes = ["x", "y", "z", "w"];
    let mut fields = String::new();
    let mut body = String::new();
    for (k, v) in program.interface.inputs.iter().enumerate() {
        let position = v.system == Some(SystemValue::FragCoord);
        if v.system.is_some() && !position {
            continue;
        }
        let d3d = v.provenance.d3d.expect("a register");
        let scalar = match v.value.scalar {
            ScalarType::Uint => "uint",
            ScalarType::Sint => "int",
            _ => "float",
        };
        let width = if v.value.width > 1 {
            v.value.width.to_string()
        } else {
            String::new()
        };
        let interpolation = match (v.value.scalar, v.interp) {
            (ScalarType::Float, Some(i)) => {
                let mut m = String::new();
                if i.mode == InterpMode::Linear {
                    m += "noperspective ";
                }
                match i.sampling {
                    InterpSampling::Centroid => m += "centroid ",
                    InterpSampling::Sample => m += "sample ",
                    InterpSampling::Center => {}
                }
                m
            }
            (ScalarType::Float, None) => String::new(),
            _ => "nointerpolation ".into(),
        };
        let semantic = if position {
            "SV_Position".to_owned()
        } else {
            format!("{}{}", v.label.name, v.label.index)
        };
        fields.push_str(&format!(
            "    {}{scalar}{width} e{k} : {semantic};\n",
            if position { "" } else { &interpolation },
        ));
        if position {
            body.push_str(&format!("    o.e{k} = float4(p, 0.5, 1.0);\n"));
        } else {
            let swizzle: String = (0..4usize)
                .filter(|l| d3d.mask >> l & 1 != 0)
                .map(|l| lanes[l])
                .collect();
            let cast = match v.value.scalar {
                ScalarType::Uint => "",
                ScalarType::Sint => "asint",
                _ => "asfloat",
            };
            body.push_str(&format!("    o.e{k} = {cast}(r[{}].{swizzle});\n", d3d.register));
        }
    }
    format!(
        "cbuffer Case : register(b0, space1) {{ uint4 r[32]; }};\n\
         struct Out\n{{\n{fields}}};\n\
         Out main(uint id : SV_VertexID)\n{{\n    Out o;\n    \
         float2 p = float2(id == 1 ? 3.0 : -1.0, id == 2 ? -3.0 : 1.0);\n{body}    return o;\n}}\n"
    )
}

struct Case {
    name: String,
    inputs: BTreeMap<u32, [u32; 4]>,
    buffers: BTreeMap<u32, Vec<[u32; 4]>>,
}

fn compare(
    container: &Path,
    cases: &[Case],
    dxc: &Path,
    warp: &Path,
    scratch: &Path,
) -> (usize, Vec<String>) {
    let bytes = std::fs::read(container).unwrap();
    let program = primase_dxil::lift(&bytes).unwrap();
    let targets: Vec<(u32, &str)> = program
        .interface
        .outputs
        .iter()
        .map(|v| {
            (
                v.provenance.d3d.unwrap().register,
                match v.value.scalar {
                    ScalarType::Uint => "uint",
                    ScalarType::Sint => "sint",
                    _ => "float",
                },
            )
        })
        .collect();
    let stem = container
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap()
        .replace('.', "_");
    let hlsl = scratch.join(format!("{stem}.lifted.vs.hlsl"));
    let vs = scratch.join(format!("{stem}.lifted.vs.dxil"));
    std::fs::write(&hlsl, vertex_hlsl(&program)).unwrap();
    let out = std::process::Command::new(dxc)
        .args(["-T", "vs_6_0", "-E", "main", "-Fo"])
        .arg(&vs)
        .arg(&hlsl)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "dxc: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let shader = ProgramShader::new(program.clone()).unwrap();
    let position = program
        .interface
        .inputs
        .iter()
        .find(|v| v.system == Some(SystemValue::FragCoord))
        .and_then(|v| v.provenance.d3d)
        .map(|d| d.register);
    let mut text = String::new();
    let mut ours = BTreeMap::new();
    for c in cases {
        text.push_str(&format!(
            "case {}\napi d3d12\nps {}\nvs {}\nfront 1\n",
            c.name,
            container.display(),
            vs.display()
        ));
        for (r, w) in &c.inputs {
            text.push_str(&format!("input {r} {}\n", hex(w)));
        }
        for (slot, rows) in &c.buffers {
            text.push_str(&format!("cb {slot} {}\n", rows.len()));
            for row in rows {
                text.push_str(&format!("row {}\n", hex(row)));
            }
        }
        for (r, kind) in &targets {
            text.push_str(&format!("target {r} {kind}\n"));
        }
        text.push_str("end\n");
        let mut inputs = vec![[0u32; 4]; shader.input_count().max(1)];
        for (r, w) in &c.inputs {
            if let Some(slot) = inputs.get_mut(*r as usize) {
                *slot = *w;
            }
        }
        if let Some(p) = position {
            inputs[p as usize] = [0.5f32, 0.5, 0.5, 1.0].map(f32::to_bits);
        }
        let refs: Vec<(u32, &[[u32; 4]])> = c.buffers.iter().map(|(s, v)| (*s, v.as_slice())).collect();
        let mut sampler = |_: Sample| Err("no textures in these fixtures".to_owned());
        ours.insert(c.name.clone(), shader.execute(&inputs, &refs, &mut sampler));
    }
    let cases_file = scratch.join(format!("{stem}.lifted.cases.txt"));
    let results = scratch.join(format!("{stem}.lifted.warp.jsonl"));
    std::fs::write(&cases_file, text).unwrap();
    let run = std::process::Command::new(warp)
        .arg(&cases_file)
        .arg(&results)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "warp: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let (mut lanes, mut mismatches) = (0, Vec::new());
    for line in std::fs::read_to_string(&results).unwrap().lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        let name = v["case"].as_str().unwrap_or("");
        if let Some(e) = v["error"].as_str() {
            mismatches.push(format!("{name}: WARP error {e}"));
            continue;
        }
        let execution = match &ours[name] {
            Ok(e) => e,
            Err(e) => {
                mismatches.push(format!("{name}: executor error {e}"));
                continue;
            }
        };
        for (k, (reg, _)) in targets.iter().enumerate() {
            for lane in 0..4 {
                if execution.output_masks[*reg as usize] & (1 << lane) == 0 {
                    continue;
                }
                let warp_bits =
                    u32::from_str_radix(v["targets"][k][lane].as_str().unwrap_or(""), 16).unwrap();
                let our_bits = execution.outputs[*reg as usize][lane];
                lanes += 1;
                if f32::from_bits(our_bits).is_nan() && f32::from_bits(warp_bits).is_nan() {
                    continue;
                }
                if warp_bits != our_bits {
                    mismatches.push(format!(
                        "{name} o{reg}.{lane}: executor {our_bits:08x} ({}), WARP {warp_bits:08x} ({})",
                        f32::from_bits(our_bits),
                        f32::from_bits(warp_bits)
                    ));
                }
            }
        }
    }
    (lanes, mismatches)
}

#[test]
fn lifted_fixtures_agree_with_warp() {
    let (Some(dxc), Some(warp), Some(scratch)) = (
        real_data("PRIMASE_DXC"),
        real_data("PRIMASE_WARP"),
        real_data("PRIMASE_WARP_SCRATCH"),
    ) else {
        return;
    };
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    // Values { float4 a; int4 n; }, t = TEXCOORD0 at register 1
    let case = |name: &str, a: [f32; 4], n: [i32; 4], t: [f32; 2]| Case {
        name: name.to_owned(),
        inputs: BTreeMap::from([(1, [f(t[0]), f(t[1]), 0, 0])]),
        buffers: BTreeMap::from([(0, vec![a.map(f32::to_bits), n.map(|x| x as u32)])]),
    };
    let flow = [
        case("flow0", [1.0, 4.0, 2.0, 3.0], [5, 1, 0, 2], [2.0, 0.25]),
        case("flow1", [1.0, 4.0, 2.0, 3.0], [1, 3, 7, -1], [0.5, 5.0]),
        case("flow2", [1.0, 4.0, 2.0, 3.0], [0, 1, 0, 0], [2.0, 0.25]),
        case("flow3", [1.0, 4.0, 2.0, 3.0], [5, 1, 0, 2], [2.0, 0.75]),
        case("flow4", [1.0, 4.0, 2.0, 3.0], [5, 1, 0, 2], [-2.0, 1.0]),
    ];
    let loops = [
        case("loops0", [2.0, 10.0, 0.0, 100.0], [4, 2, 2, 5], [0.5, 1.0]),
        case("loops1", [2.0, 10.0, 0.0, 5.0], [6, 2, 0, 5], [0.5, 20.0]),
        case("loops2", [2.0, 10.0, 0.0, 100.0], [0, 0, 4, 5000], [0.5, 20.0]),
    ];
    let switch = [
        case("switch0", [1.5, 2.5, 3.5, 0.0], [2, 6, 0, 0], [0.25, 2.0]),
        case("switch1", [1.5, 2.5, 3.5, 0.0], [5, 3, 0, 0], [0.25, 2.0]),
        case("switch2", [1.5, 2.5, 3.5, 0.0], [9, 0, 0, 0], [0.25, 2.0]),
        case("switch3", [1.5, 2.5, 3.5, 0.0], [0, 4, 0, 0], [0.25, 2.0]),
    ];
    let mut total = 0;
    let mut all = Vec::new();
    for (base, cases) in [
        ("flow", &flow[..]),
        ("loops", &loops[..]),
        ("switch", &switch[..]),
    ] {
        for variant in ["", ".od"] {
            let container = dir.join(format!("{base}{variant}.dxil"));
            let named: Vec<Case> = cases
                .iter()
                .map(|c| Case {
                    name: format!("{}{}", c.name, variant.replace('.', "_")),
                    inputs: c.inputs.clone(),
                    buffers: c.buffers.clone(),
                })
                .collect();
            let (lanes, mismatches) = compare(&container, &named, &dxc, &warp, &scratch);
            total += lanes;
            all.extend(mismatches);
        }
    }
    println!("{total} output lanes compared with WARP, {} differ", all.len());
    for m in &all {
        println!("MISMATCH {m}");
    }
    assert!(total > 0 && all.is_empty());
}
