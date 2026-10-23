#![allow(clippy::unwrap_used)]
//! The program executor on lifted DXIL against Microsoft's WARP rasterizer (Direct3D 12, Shader Model 6), the oracle
//! that is not ours: each case is rendered by WARP on one pixel with the same input registers and constant buffers,
//! and run on [`ProgramShader`]; every lane both write must agree bit for bit (the fixtures use no transcendental).
//!
//! The fixtures are the sampled and integer ones (blend, ops), the control-flow and memory ones (flow, loops, switch,
//! arrays, bits; default and -Od builds), the resource arrays (textures of different sizes in space 1, samplers in space 2) and the
//! pixel operations a texture load at an offset and the half conversions need. Needs `tools.warp` (the oracle program)
//! and `tools.dxc` (for the oracle's vertex shader); it works in `warp` under the scratch folder. The vertex shader is
//! generated from the container's interface ([`primase_dxil::interface`]): every input varying read from
//! `cbuffer Case : register(b0, space1) { uint4 r[32]; }`, row by register.
use primase_shader::{
    D3dClass, Interface, InterpMode, InterpSampling, Program, ResourceId, ScalarType, SystemValue,
};
use primase_shader_exec::{
    Backend, Element, Executable, ImageLoad, ImageQuery, ImageSample, ProgramShader, QueryKind, Resources,
    Runner,
};
use primase_shader_exec::{PixelShader, Sample};
use primase_testdata::{Tool, scratch, tool};
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

/// HLSL of a vertex shader that feeds the pixel inputs of `interface` (the container's whole signature, unused elements
/// too: the pipeline links it to the vertex shader's): one clockwise triangle covering the pixel at the
/// origin of a one-pixel viewport (SV_Position (0.5, 0.5, 0.5, 1)), every other input read from the case buffer.
/// Elements are declared in signature order with the pixel program's types and interpolation so the compiler packs
/// them alike; system values the rasterizer generates (front face, sample index, ...) are left out.
fn vertex_hlsl(interface: &Interface) -> String {
    let lanes = ["x", "y", "z", "w"];
    let mut fields = String::new();
    let mut body = String::new();
    for (k, v) in interface.inputs.iter().enumerate() {
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
    // a vertex shader must write SV_Position, whether or not the pixel program declares it
    if !interface
        .inputs
        .iter()
        .any(|v| v.system == Some(SystemValue::FragCoord))
    {
        fields.insert_str(0, "    float4 position : SV_Position;\n");
        body.insert_str(0, "    o.position = float4(p, 0.5, 1.0);\n");
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
    /// Whether the oracle renders a front face (and the executor is told it does).
    front: bool,
    inputs: BTreeMap<u32, [u32; 4]>,
    buffers: BTreeMap<u32, Vec<[u32; 4]>>,
    /// The oracle's `texture` lines, as written after the keyword.
    textures: Vec<String>,
}

/// What the executor makes of a case: per output member, its four lanes and a mask of those written.
type Ours = Result<Vec<([u32; 4], u8)>, String>;

/// The executor's run of a program with constant buffers, and textures that are one colour each (the case's `texture`
/// lines of the form `<slot> <four hex words>`).
fn plain(program: &Program, c: &Case) -> Ours {
    let shader = ProgramShader::new(program.clone()).unwrap();
    let position = program
        .interface
        .inputs
        .iter()
        .find(|v| v.system == Some(SystemValue::FragCoord))
        .and_then(|v| v.provenance.d3d)
        .map(|d| d.register);
    let mut inputs = vec![[0u32; 4]; shader.input_count().max(1)];
    for (r, w) in &c.inputs {
        if let Some(slot) = inputs.get_mut(*r as usize) {
            *slot = *w;
        }
    }
    if let Some(p) = position {
        inputs[p as usize] = [0.5f32, 0.5, 0.5, 1.0].map(f32::to_bits);
    }
    let front = program
        .interface
        .inputs
        .iter()
        .find(|v| v.system == Some(SystemValue::FrontFacing))
        .and_then(|v| v.provenance.d3d);
    if let Some(d) = front {
        inputs[d.register as usize][d.mask.trailing_zeros() as usize] = if c.front { u32::MAX } else { 0 };
    }
    let refs: Vec<(u32, &[[u32; 4]])> = c.buffers.iter().map(|(s, v)| (*s, v.as_slice())).collect();
    let colours: BTreeMap<u32, [f32; 4]> = c
        .textures
        .iter()
        .map(|t| {
            let mut words = t.split_whitespace();
            let slot = words.next().unwrap().parse().unwrap();
            let mut colour = [0f32; 4];
            for lane in &mut colour {
                *lane = f32::from_bits(u32::from_str_radix(words.next().unwrap(), 16).unwrap());
            }
            (slot, colour)
        })
        .collect();
    let mut sampler = |q: Sample| {
        colours
            .get(&q.texture)
            .copied()
            .ok_or_else(|| format!("no texture at t{}", q.texture))
    };
    let e = shader
        .execute(&inputs, &refs, &mut sampler)
        .map_err(|e| e.to_string())?;
    Ok(program
        .interface
        .outputs
        .iter()
        .map(|v| {
            let r = v.provenance.d3d.unwrap().register as usize;
            (e.outputs[r], e.output_masks[r])
        })
        .collect())
}

/// Render `cases` of the container on WARP and compare every lane the executor writes, as `ours` makes it of the lifted
/// program (its output members in the order of the interface's outputs). A float lane agrees when it is bit for bit the
/// same, or within `tolerance` relative (0 for the fixtures that use no `exp`, `log`, `sin`, `cos` or `rsqrt`, which
/// Direct3D leaves to the implementation within a few units in the last place).
fn compare(
    container: &Path,
    cases: &[Case],
    dxc: &Path,
    warp: &Path,
    scratch: &Path,
    ours: &dyn Fn(&Program, &Case) -> Ours,
    tolerance: f64,
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
    std::fs::write(&hlsl, vertex_hlsl(&primase_dxil::interface(&bytes).unwrap())).unwrap();
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
    let mut text = String::new();
    let mut ran = BTreeMap::new();
    for c in cases {
        text.push_str(&format!(
            "case {}\napi d3d12\nps {}\nvs {}\nfront {}\n",
            c.name,
            container.display(),
            vs.display(),
            u8::from(c.front)
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
        for t in &c.textures {
            text.push_str(&format!("texture {t}\n"));
        }
        for (r, kind) in &targets {
            text.push_str(&format!("target {r} {kind}\n"));
        }
        text.push_str("end\n");
        ran.insert(c.name.clone(), ours(&program, c));
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
        let execution = match &ran[name] {
            Ok(e) => e,
            Err(e) => {
                mismatches.push(format!("{name}: executor error {e}"));
                continue;
            }
        };
        for (k, (reg, kind)) in targets.iter().enumerate() {
            for lane in 0..4 {
                let (values, mask) = execution[k];
                if mask & (1 << lane) == 0 {
                    continue;
                }
                let warp_bits =
                    u32::from_str_radix(v["targets"][k][lane].as_str().unwrap_or(""), 16).unwrap();
                let our_bits = values[lane];
                lanes += 1;
                if f32::from_bits(our_bits).is_nan() && f32::from_bits(warp_bits).is_nan() {
                    continue;
                }
                if *kind == "float" && warp_bits != our_bits {
                    let (a, b) = (
                        f64::from(f32::from_bits(our_bits)),
                        f64::from(f32::from_bits(warp_bits)),
                    );
                    if (a - b).abs() <= tolerance * a.abs().max(b.abs()) {
                        continue;
                    }
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

/// `tools.dxc` and `tools.warp`, and the WARP comparison's folder under the scratch folder; `None` unless both tools
/// are configured.
fn tools() -> Option<(std::path::PathBuf, std::path::PathBuf, std::path::PathBuf)> {
    let (dxc, warp) = (tool(Tool::Dxc)?, tool(Tool::Warp)?);
    let scratch = scratch().join("warp");
    std::fs::create_dir_all(&scratch).unwrap();
    Some((dxc, warp, scratch))
}

#[test]
fn lifted_fixtures_agree_with_warp() {
    let Some((dxc, warp, scratch)) = tools() else {
        return;
    };
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    // Values { float4 a; int4 n; }, t = TEXCOORD0 at register 1
    let case = |name: &str, a: [f32; 4], n: [i32; 4], t: [f32; 2]| Case {
        name: name.to_owned(),
        front: true,
        inputs: BTreeMap::from([(1, [f(t[0]), f(t[1]), 0, 0])]),
        buffers: BTreeMap::from([(0, vec![a.map(f32::to_bits), n.map(|x| x as u32)])]),
        textures: Vec::new(),
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
    // Values { float4 a; int4 n; float4 table[6]; }, t = TEXCOORD0 at register 1
    let array_case = |name: &str, a: [f32; 4], n: [i32; 4]| {
        let table = (0..6).map(|r| [f(10.0 + r as f32), f(20.0 + r as f32), f(0.5), f(0.25)]);
        let mut rows = vec![a.map(f32::to_bits), n.map(|x| x as u32)];
        rows.extend(table);
        Case {
            name: name.to_owned(),
            front: true,
            inputs: BTreeMap::from([(1, [0, 0, 0, 0])]),
            buffers: BTreeMap::from([(0, rows)]),
            textures: Vec::new(),
        }
    };
    let arrays = [
        array_case("arrays0", [1.0, 2.0, 3.0, 4.0], [1, 2, 3, 4]),
        array_case("arrays1", [0.5, -2.0, 8.0, 0.125], [0, 3, 5, 0]),
        array_case("arrays2", [7.0, 1.0, -1.0, 2.0], [6, 5, 1, 5]),
        array_case("arrays3", [1.0, 2.0, 3.0, 4.0], [-1, 1, 0, 2]),
    ];
    // Values { float4 f; uint4 u; int4 i; }; no divide by zero
    let bits_case = |name: &str, f4: [f32; 4], u4: [u32; 4], i4: [i32; 4]| Case {
        name: name.to_owned(),
        front: true,
        inputs: BTreeMap::new(),
        buffers: BTreeMap::from([(0, vec![f4.map(f32::to_bits), u4, i4.map(|x| x as u32)])]),
        textures: Vec::new(),
    };
    let bits = [
        bits_case(
            "bits0",
            [3.0, 4.0, 1.5, 0.0],
            [176, 8, 0x1_0000, 1],
            [-7, 2, -16, 2],
        ),
        bits_case("bits1", [-2.5, 0.5, -0.25, 9.0], [0, 1, 0, 31], [7, -3, 1000, 35]),
        bits_case(
            "bits2",
            [100.0, 3.0, 0.75, 4e9],
            [u32::MAX, 3, 0x8000_0001, 0xdead_beef],
            [i32::MIN, 7, i32::MAX, 31],
        ),
        bits_case(
            "bits3",
            [1e-3, -1e3, 2.0, 255.5],
            [12345, 678, 0x0f0f_0f0f, 5],
            [-100, -9, -1, 1],
        ),
    ];
    let mut total = 0;
    let mut all = Vec::new();
    for (base, cases) in [
        ("flow", &flow[..]),
        ("loops", &loops[..]),
        ("switch", &switch[..]),
        ("arrays", &arrays[..]),
        ("bits", &bits[..]),
    ] {
        for variant in ["", ".od"] {
            let container = dir.join(format!("{base}{variant}.dxil"));
            let named: Vec<Case> = cases
                .iter()
                .map(|c| Case {
                    name: format!("{}{}", c.name, variant.replace('.', "_")),
                    front: c.front,
                    inputs: c.inputs.clone(),
                    buffers: c.buffers.clone(),
                    textures: Vec::new(),
                })
                .collect();
            let (lanes, mismatches) = compare(&container, &named, &dxc, &warp, &scratch, &plain, 0.0);
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

/// `blend.dxil` and `ops.dxil`, the sampled and the integer-and-rounding fixtures: the executor's values for them are
/// worked out by hand in `fixtures.rs`; here WARP's. `ops` raises `exp2`, `log2` and `rsqrt`, which Direct3D leaves to the
/// implementation within a few units in the last place: its float lanes agree within 1e-5 relative.
#[test]
fn lifted_blend_and_ops_agree_with_warp() {
    let Some((dxc, warp, scratch)) = tools() else {
        return;
    };
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    // blend: uv, weight and id inputs (TEXCOORD0 to 2), constants scale, bias and flags (b3), one texture (t2)
    let mut blend = Vec::new();
    for (k, (uv, w, id)) in [
        ([0.25f32, 0.75], 0.4f32, 7u32),
        ([0.6, 0.1], 0.9, 3),
        ([0.0, 1.0], -2.0, 0),
    ]
    .into_iter()
    .enumerate()
    {
        for front in [false, true] {
            let colour = [0.3, 0.7, 0.2 + k as f32 * 0.1, 1.0].map(f);
            blend.push(Case {
                name: format!("blend{k}{}", if front { "f" } else { "b" }),
                front,
                inputs: BTreeMap::from([(1, [f(uv[0]), f(uv[1]), f(w), 0]), (2, [id, 0, 0, 0])]),
                buffers: BTreeMap::from([(
                    3,
                    vec![
                        [f(2.), f(4.), f(1.5), 0],
                        [f(0.5), f(0.25), f(0.1), 0],
                        [5, 0, 0, 0],
                    ],
                )]),
                textures: vec![format!("2 {}", hex(&colour))],
            });
        }
    }
    // ops: constants a and n (b0), t (TEXCOORD0)
    let ops: Vec<Case> = [
        ([2.5f32, 3., 8., 4.], [-7i32, 5], [0.75f32, 0.25]),
        ([-1.25, 0.5, 0.125, 9.], [100, 3], [0.1, 0.9]),
        ([3.5, -2., 1024., 0.0625], [-1, 0], [0.5, 0.5]),
    ]
    .into_iter()
    .enumerate()
    .map(|(k, (a, n, t))| Case {
        name: format!("ops{k}"),
        front: true,
        inputs: BTreeMap::from([(1, [f(t[0]), f(t[1]), 0, 0])]),
        buffers: BTreeMap::from([(0, vec![a.map(f32::to_bits), [n[0] as u32, n[1] as u32, 0, 0]])]),
        textures: Vec::new(),
    })
    .collect();
    let (mut total, mut all) = (0, Vec::new());
    for (fixture, cases, tolerance) in [("blend.dxil", &blend, 0.0), ("ops.dxil", &ops, 1e-5)] {
        let (lanes, mismatches) = compare(
            &dir.join(fixture),
            cases,
            &dxc,
            &warp,
            &scratch,
            &plain,
            tolerance,
        );
        total += lanes;
        all.extend(mismatches);
    }
    println!("{total} output lanes compared with WARP, {} differ", all.len());
    for m in &all {
        println!("MISMATCH {m}");
    }
    assert!(total > 0 && all.is_empty());
}

/// The resource of a class at a register, by its D3D binding.
fn resource_at(program: &Program, register: u32, class: D3dClass) -> ResourceId {
    program
        .interface
        .resources
        .iter()
        .find(|r| {
            r.provenance
                .d3d
                .is_some_and(|d| d.register == register && d.class == class)
        })
        .unwrap_or_else(|| panic!("no {class:?} at {register}"))
        .id
}

/// Run a lifted pixel program once on the executor with `constants` in its constant buffer 0, the entry arguments
/// `args` and `backend` for its images; every output member with the lanes of its width marked written.
fn run_with_backend(
    program: &Program,
    constants: Vec<[u32; 4]>,
    args: &[[u32; 4]],
    backend: &mut dyn Backend,
) -> Ours {
    let exe = Executable::new(program).map_err(|e| e.to_string())?;
    let mut resources = Resources::default();
    resources
        .constants
        .insert(resource_at(program, 0, D3dClass::ConstantBuffer), constants);
    let bound = exe.bind_resources(&resources)?;
    let mut runner = Runner::<1>::new(&exe, &bound, backend, None);
    let [out] = runner.run([args])?;
    Ok(program
        .interface
        .outputs
        .iter()
        .zip(out.values)
        .map(|(v, values)| (values, (1u8 << v.value.width) - 1))
        .collect())
}

/// The textures of `resource_arrays.hlsl` as the oracle is given them: texture `i` of the array (t0 to t3 in space 1)
/// is 2 << i by 1 << i texels of the value i + 1 in every lane, the single texture (t8) is 32 by 64.
struct Sized {
    array: ResourceId,
}
impl Backend for Sized {
    fn sample(&mut self, r: ImageSample<Element>) -> Result<[f32; 4], String> {
        assert_eq!(r.texture.id, self.array);
        Ok([(r.texture.index + 1) as f32; 4])
    }
    fn load(&mut self, r: ImageLoad<Element>) -> Result<[u32; 4], String> {
        assert_eq!(r.texture.id, self.array);
        // a load outside the texture reads zeros
        let (w, h) = (2 << r.texture.index, 1 << r.texture.index);
        Ok(if r.texel[0] < w && r.texel[1] < h {
            [f((r.texture.index + 1) as f32); 4]
        } else {
            [0; 4]
        })
    }
    fn query(&mut self, r: ImageQuery<Element>) -> Result<[u32; 4], String> {
        assert_eq!(r.kind, QueryKind::Size);
        Ok(if r.texture.id == self.array {
            [2 << r.texture.index, 1 << r.texture.index, 1, 0]
        } else {
            [32, 64, 1, 0]
        })
    }
}

/// `pixel_ops.hlsl`'s `img` is a ramp: texel (x, y) is (x, y, 0, 1).
struct Ramp;
impl Backend for Ramp {
    fn sample(&mut self, _: ImageSample<Element>) -> Result<[f32; 4], String> {
        Err("no sampling in this fixture".into())
    }
    fn load(&mut self, r: ImageLoad<Element>) -> Result<[u32; 4], String> {
        Ok([f(r.texel[0] as f32), f(r.texel[1] as f32), 0, f(1.0)])
    }
    // the sample count of the multisampled texture, which only the sample position (not compared) depends on
    fn query(&mut self, _: ImageQuery<Element>) -> Result<[u32; 4], String> {
        Ok([4, 0, 0, 0])
    }
}

#[test]
fn lifted_resource_arrays_and_pixel_operations_agree_with_warp() {
    let Some((dxc, warp, scratch)) = tools() else {
        return;
    };
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let hexf = |x: f32| format!("{:08x}", x.to_bits());
    let uniform = |x: f32| format!("{0} {0} {0} {0}", hexf(x));
    // Select { uint2 which; float2 uv; }; texture i of the array in space 1 at t(i), the single one at t8
    let arrays = |name: &str, which: [u32; 2]| Case {
        name: name.to_owned(),
        front: true,
        inputs: BTreeMap::new(),
        buffers: BTreeMap::from([(0, vec![[which[0], which[1], f(0.25), f(0.75)]])]),
        textures: (0..4u32)
            .map(|i| format!("{i}:1 {} {}x{}", uniform((i + 1) as f32), 2 << i, 1 << i))
            .chain([format!("8 {} 32x64", uniform(1.0))])
            .collect(),
    };
    // Values { float h; uint packed; }; img (t0) is a 16 by 16 ramp
    let ops = |name: &str, h: f32, packed: u32| Case {
        name: name.to_owned(),
        front: true,
        inputs: BTreeMap::new(),
        buffers: BTreeMap::from([(0, vec![[f(h), packed, 0, 0]])]),
        textures: vec![format!("0 {} 16x16 ramp", uniform(0.0))],
    };
    let (mut total, mut all) = (0, Vec::new());
    for variant in ["", ".od", ".66"] {
        let tag = variant.replace('.', "_");
        let named = |cases: Vec<Case>| -> Vec<Case> {
            cases
                .into_iter()
                .map(|c| Case {
                    name: format!("{}{tag}", c.name),
                    ..c
                })
                .collect()
        };
        // tex[which.x] with smp[which.y]; the load of tex[which.x + 1] at (1, 2) is out of range for textures 0 and 1
        let cases = named(vec![
            arrays("arrays_1_3", [1, 3]),
            arrays("arrays_2_0", [2, 0]),
            arrays("arrays_0_2", [0, 2]),
            arrays("arrays_2_3", [2, 3]),
        ]);
        let container = dir.join(format!("resource_arrays{variant}.dxil"));
        let (lanes, mismatches) = compare(
            &container,
            &cases,
            &dxc,
            &warp,
            &scratch,
            &|program, c| {
                let mut backend = Sized {
                    array: resource_at(program, 0, D3dClass::ShaderResource),
                };
                run_with_backend(
                    program,
                    c.buffers[&0].clone(),
                    &[[f(0.5), f(0.5), f(0.5), f(1.0)]],
                    &mut backend,
                )
            },
            0.0,
        );
        total += lanes;
        all.extend(mismatches);
        // the offset load and the half conversions; the sample index and its position need a multisampled target
        let cases = named(vec![
            ops("ops0", 1.5, 0x4000),
            ops("ops1", -0.375, 0x3c00),
            ops("ops2", 65504.0, 0xc400),
        ]);
        let container = dir.join(format!("pixel_ops{variant}.dxil"));
        let (lanes, mismatches) = compare(
            &container,
            &cases,
            &dxc,
            &warp,
            &scratch,
            &|program, c| {
                // main(float4 pos : SV_Position, uint sample : SV_SampleIndex): the fragment's position and the sample's
                // index, neither of which a compared output depends on
                let mut out = run_with_backend(
                    program,
                    c.buffers[&0].clone(),
                    &[[0, 0, 0, 0], [0, 0, 0, 0]],
                    &mut Ramp,
                )?;
                // Target1's x and y are the sample's position: not compared
                out[1].1 &= 0b1100;
                Ok(out)
            },
            0.0,
        );
        total += lanes;
        all.extend(mismatches);
    }
    println!("{total} output lanes compared with WARP, {} differ", all.len());
    for m in &all {
        println!("MISMATCH {m}");
    }
    assert!(total > 0 && all.is_empty());
}
