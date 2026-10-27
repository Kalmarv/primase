//! Native REF executes the complete stored PS. The independent source equation checks the raw cut;
//! the original lifted downstream graph re-encodes it for comparison with the native forward output.
#![allow(
    clippy::unwrap_used,
    reason = "explicit fixtures and native oracle assertions"
)]
use anvil_formats::shaunwhite::shaders::{Database, Effect, Payload};
use iridia_exec::{PixelShader, ProgramShader, Sample};
use iridia_interface::Stage;
use primase_core::{World, gate::GateReport, gate::Tolerance};
use primase_shaunwhite::shaders::albedo::{Influence, derive};
use primase_testdata::{Game, Tool, install_file, own_scratch, tool};
use std::{collections::BTreeMap, process::Command};

mod support;
use support::binding;

#[test]
fn hand_worked_frontier_is_a_derivation_and_refuses_a_disconnected_colour() {
    use anvil_formats::shaunwhite::shaders::{Key, Record, SamplerBinding};
    let mut ps = vec![
        0xffff0300u32,
        0x0200001f,
        0x80000005,
        0x90030000,
        0x0200001f,
        0x90000000,
        0xa00f0800,
        0x03000042,
        0x800f0000,
        0x90e40000,
        0xa0e40800,
        0x03000005,
        0x800f0000,
        0x80e40000,
        0xa0e40000,
        0x02000001,
        0x800f0800,
        0x80e40000,
        0xffff,
    ];
    let make = |ps: &[u32]| Database {
        sources: vec![],
        table_padding: 0,
        records: vec![Record {
            key: Key {
                kind: 4,
                permutation: 3 | (2 << 20),
                dependencies: [0; 4],
            },
            offset: 0,
            bytes: 0,
            payload: Payload::Effect(Effect {
                vertex: None,
                pixel: Some(ps.iter().flat_map(|n| n.to_le_bytes()).collect()),
                vertex_key: 0,
                pixel_key: 7,
                states: vec![],
                restore_states: vec![],
                samplers: vec![SamplerBinding {
                    sampler: "Layer1_DiffuseSampler".into(),
                    texture: "g_Layer1_DiffuseTexture".into(),
                    address: 0x800000,
                    states: vec![],
                }],
                additional_samplers: vec![],
                variables: vec![],
            }),
        }],
    };
    let db = make(&ps);
    let b = binding(&db, 0);
    let d = derive(&db, 0, &b, Influence::Unrestored).unwrap();
    assert_eq!(d.derivation.projection.after_word, 7);
    assert_eq!(d.derivation.influence, Influence::Unrestored);
    assert!(d.derivation.cut_inputs.iter().any(|s| s.starts_with("c0 lane")));
    let first_text = iridia::emit::wgsl(&d.program).unwrap();
    let p = ProgramShader::new(d.program).unwrap();
    let result = p
        .execute(
            &[[0; 4]],
            &[(0, &[[2.; 4].map(f32::to_bits)])],
            &mut |_: Sample| Ok([0.25, 0.5, 0.75, 0.8]),
        )
        .unwrap();
    assert_eq!(result.outputs[0].map(f32::from_bits), [0.25, 0.5, 0.75, 1.]);
    ps[17] = 0xa0e40000; // output c0, disconnecting the diffuse path entirely
    assert!(
        derive(&make(&ps), 0, &b, Influence::Restored)
            .unwrap_err()
            .contains("frontier")
    );
    let mut wrong_stage = b.clone();
    wrong_stage.stage = Stage::Vertex;
    assert!(derive(&db, 0, &wrong_stage, Influence::Restored).is_err());
    let mut wrong_key = b.clone();
    wrong_key.variant.as_mut().unwrap().parts[1].value ^= 1;
    assert!(
        derive(&db, 0, &wrong_key, Influence::Restored)
            .unwrap_err()
            .contains("cache key")
    );
    let same = derive(&db, 0, &b, Influence::Unrestored).unwrap();
    assert_eq!(iridia::emit::wgsl(&same.program).unwrap(), first_text);
    let mut bad = b.clone();
    bad.variant.as_mut().unwrap().value = 1;
    assert!(
        derive(&db, 0, &bad, Influence::Restored)
            .unwrap_err()
            .contains("exact SDB2")
    );
}

#[derive(Clone)]
struct State {
    c: [[f32; 4]; 224],
    v: [[f32; 4]; 11],
    t: [[f32; 4]; 16],
    mask: u32,
}
impl State {
    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(0x39534652u32.to_le_bytes());
        out.extend(self.mask.to_le_bytes());
        out.extend(
            self.c
                .iter()
                .chain(&self.v)
                .chain(&self.t)
                .flatten()
                .flat_map(|x| x.to_le_bytes()),
        );
        assert_eq!(out.len(), 4024);
        out
    }
}
#[derive(Clone, Copy)]
struct Case {
    vertex: [f32; 3],
    mask: [f32; 4],
    tex: [[f32; 4]; 3],
    tint: [[f32; 4]; 3],
    alpha: [f32; 4],
    exposure: f32,
}
fn cases() -> Vec<Case> {
    let ordinary = Case {
        vertex: [0.2, 0.6, 0.9],
        mask: [0.8, 0.4, 0.7, 1.],
        tex: [[0.25, 0.5, 0.75, 0.3], [0.9, 0.3, 0.1, 0.4], [0.1, 0.4, 0.8, 0.7]],
        tint: [[0.8, 0.9, 0.7, 0.9], [0.7, 0.6, 0.9, 0.5], [0.9, 0.8, 0.7, 0.6]],
        alpha: [0., 1., 1., 0.],
        exposure: 1.,
    };
    vec![
        ordinary,
        Case {
            vertex: [0., 0., 0.],
            ..ordinary
        },
        Case {
            vertex: [1., 1., 1.],
            alpha: [0.; 4],
            ..ordinary
        },
        Case {
            tex: [[2., 0.15, 0.4, 0.8], [3., 0.2, 0.1, 0.5], [4., 0.25, 0.3, 0.6]],
            tint: [[1.2, 1.1, 0.9, 1.], [1., 1.2, 1.1, 1.], [1.1, 1.2, 1.3, 1.]],
            exposure: 4.,
            ..ordinary
        },
    ]
}
fn source_material(case: Case) -> primase_shaunwhite::components::MaterialRef {
    use anvil_formats::shaunwhite::material::LayerState;
    use primase_shaunwhite::components::{MaterialRef, MaterialState, SlotRef};
    let mut w = World::new();
    MaterialRef {
        id: 1,
        name: "controlled material".into(),
        state: Some(MaterialState {
            blend: 0,
            blended: false,
            alpha_test: false,
            alpha_ref: 128,
            water: false,
            two_sided: false,
            invert_culling: false,
            color: case.tint[0],
            illumination: 0.,
            specular_level: 1.,
            specular_power: 10.,
            normal_scale: 1.,
            layering: LayerState {
                colors: [case.tint[1], case.tint[2]],
                use_alpha: case.alpha[..3]
                    .iter()
                    .map(|v| *v != 0.)
                    .collect::<Vec<_>>()
                    .try_into()
                    .unwrap(),
            },
        }),
        slots: [
            "diffuse",
            "diffuse_b",
            "diffuse_c",
            "mask",
            "normal",
            "specular",
            "specular_b",
        ]
        .into_iter()
        .map(|name| {
            (
                name.into(),
                SlotRef {
                    texture: w.spawn(),
                    scale: [1.; 2],
                    offset: [0.; 2],
                    rotation: 0.,
                    texcoord: 0,
                    address: None,
                },
            )
        })
        .collect(),
        unread: vec![],
    }
}
fn check_material_bindings(
    derived: &primase_shaunwhite::shaders::albedo::DerivedAlbedo,
    case: Case,
    state: &State,
) {
    use iridia_interface::ConstantRole;
    let bound = primase_shaunwhite::shaders::bind_material(derived, &source_material(case)).unwrap();
    let reads = derived.program.constant_reads().unwrap();
    let mut checked = 0;
    for c in &bound.constants {
        if let ConstantRole::Material(words) = &c.role {
            for (n, &word) in words.iter().enumerate() {
                let at = c.offset + n as u32 * 4;
                if reads.reads(c.resource, at, 4) {
                    assert_eq!(
                        word,
                        state.c[at as usize / 16][at as usize % 16 / 4].to_bits(),
                        "material helper vs independently supplied REF fixture {} at byte{at}",
                        c.name.as_deref().unwrap_or("unnamed")
                    );
                    checked += 1;
                }
            }
        }
    }
    let required = reads
        .buffers()
        .map(|resource| reads.words(resource).count())
        .sum::<usize>();
    assert!(required > 0);
    assert_eq!(
        checked, required,
        "every material word read by the proven albedo records must be filled"
    );
    assert!(
        bound
            .constants
            .iter()
            .any(|c| matches!(c.role, ConstantRole::Unknown { .. })),
        "frame words must remain unresolved"
    );
}
fn state(effect: &Effect, case: Case, influence: f32) -> State {
    let source = iridia_d3d9::read(effect.pixel.as_ref().unwrap()).unwrap();
    let mut s = State {
        c: [[0.; 4]; 224],
        v: [[0.; 4]; 11],
        t: [[0.; 4]; 16],
        mask: 0,
    };
    for input in &source.inputs {
        let host = match (input.usage, input.index) {
            (10, 0) => 0,
            (10, 1) => 1,
            (5, n @ 0..=8) => 2 + usize::from(n),
            other => panic!("native fixture varying {other:?}"),
        };
        s.mask |= 1 << host;
        s.v[host] = match host {
            0 => [case.vertex[0], case.vertex[1], case.vertex[2], 1.],
            1 => [1.; 4],
            10 => [1., 0., 0., 0.],
            _ => [0.25, 0.5, 0., 0.],
        };
    }
    let table = source.constants.unwrap();
    for c in &table.constants {
        let row = usize::from(c.register);
        match c.name.as_str() {
            "g_Layer1_ModulationColor" => s.c[row] = case.tint[0],
            "g_Layer2_ModulationColor" => s.c[row] = case.tint[1],
            "g_Layer3_ModulationColor" => s.c[row] = case.tint[2],
            "g_FlagsUseTexAlpha" => s.c[row] = case.alpha,
            "g_HDR_Exposure" => s.c[row] = [case.exposure; 4],
            "g_ColorControlMatrixNotInfluenced" | "g_ColorControlMatrixInfluenced" => {
                for n in 0..3 {
                    s.c[row + n][n] = 1.;
                }
            }
            "g_FogClampDistance" => s.c[row] = [100.; 4],
            "g_VectorArray" => {
                s.c[row + 8] = case.tint[1];
                for offset in [9, 13, 15] {
                    if offset + 1 < usize::from(c.count) {
                        s.c[row + offset] = [1., -0., 0., 0.];
                        s.c[row + offset + 1] = [0., 1., 0., 0.];
                    }
                }
            }
            _ => {}
        }
    }
    for sampler in &effect.samplers {
        let slot = (sampler.address & 65535) as usize;
        s.t[slot] = match sampler.texture.as_str() {
            "g_Layer1_DiffuseTexture" => case.tex[0],
            "g_Layer2_DiffuseTexture" => case.tex[1],
            "g_Layer3_DiffuseTexture" => case.tex[2],
            "g_Layer1_SpecularTexture" | "g_Layer2_SpecularTexture" => [0.; 4],
            "g_MaskTexture" => case.mask,
            "g_InfluenceTexture" => [0., 0., 0., influence],
            other => panic!("native fixture texture {other}"),
        };
    }
    s
}
fn execute(p: &ProgramShader, effect: &Effect, s: &State, cut: Option<[f32; 4]>) -> [f32; 4] {
    execute_sample(p, effect, s, cut, &mut |request: Sample| {
        Ok(s.t[request.texture as usize])
    })
}
fn execute_sample(
    p: &ProgramShader,
    effect: &Effect,
    s: &State,
    cut: Option<[f32; 4]>,
    sample: &mut dyn iridia_exec::Sampler,
) -> [f32; 4] {
    let source = iridia_d3d9::read(effect.pixel.as_ref().unwrap()).unwrap();
    let mut v = vec![[0; 4]; 32];
    for i in &source.inputs {
        let host = match (i.usage, i.index) {
            (10, 0) => 0,
            (10, 1) => 1,
            (5, n) => 2 + usize::from(n),
            _ => unreachable!(),
        };
        v[i.register as usize] = s.v[host].map(f32::to_bits);
    }
    if let Some(rgb) = cut {
        v[31] = rgb.map(f32::to_bits);
    }
    let c = s.c.map(|v| v.map(f32::to_bits));
    p.execute(&v, &[(0, &c)], sample).unwrap().outputs[0].map(f32::from_bits)
}
// Independent equation from shipped e k generic source FctDiffuseColor: interpolate base modulation,
// then layer2 and layer3 by vertex.g/b * mask.g/b * optional texture-alpha modulation.
// This is test evidence, never production derivation or a replacement shader.
fn expected(case: Case, effect: &Effect) -> [f32; 3] {
    let has = |name: &str| effect.samplers.iter().any(|s| s.texture == name);
    let weights = case.vertex;
    let masked = has("g_MaskTexture");
    let weight = |n: usize| weights[n] * if masked { case.mask[n] } else { 1. };
    let mut rgb = [0.; 3];
    for (n, x) in rgb.iter_mut().enumerate() {
        *x = case.tex[0][n] * (1. + weight(0) * (case.tint[0][n] - 1.));
    }
    for layer in 1..3 {
        if !has(if layer == 1 {
            "g_Layer2_DiffuseTexture"
        } else {
            "g_Layer3_DiffuseTexture"
        }) {
            continue;
        }
        let w = weight(layer) * (1. + case.alpha[layer] * (case.tint[layer][3] * case.tex[layer][3] - 1.));
        for (n, x) in rgb.iter_mut().enumerate() {
            *x += w * (case.tex[layer][n] * case.tint[layer][n] - *x);
        }
    }
    rgb
}
fn stored_material(
    name: &str,
) -> Option<(
    primase_shaunwhite::components::MaterialRef,
    primase_shaunwhite::components::MaterialShader,
)> {
    support::material("01_Q01_UnloadMetroStation_DataBlock", name)
}
fn compatible_record(
    db: &Database,
    material: &primase_shaunwhite::components::MaterialRef,
    fields: &primase_shaunwhite::components::MaterialShader,
) -> usize {
    let (mask, value) = primase_shaunwhite::passes::material_key(material, fields).unwrap();
    let selected = db
        .records
        .iter()
        .enumerate()
        .find(|(_, r)| {
            let key = r.key.permutation;
            let Payload::Effect(e) = &r.payload else {
                return false;
            };
            r.key.kind == 4
                && key & mask == value
                && (key >> 20) & 7 == 2
                && [7, 17, 24, 39].iter().all(|bit| key & (1 << bit) != 0)
                && e.samplers.iter().all(|s| {
                    matches!(
                        s.texture.as_str(),
                        "g_Layer1_DiffuseTexture"
                            | "g_Layer2_DiffuseTexture"
                            | "g_Layer3_DiffuseTexture"
                            | "g_MaskTexture"
                            | "g_InfluenceTexture"
                            | "g_Layer1_SpecularTexture"
                            | "g_Layer2_SpecularTexture"
                    )
                })
        })
        .expect("metro mural has a compatible fog/static-lighting Colour variant");
    eprintln!(
        "STORED exact material {}, record {}, key {:#x}, material mask {mask:#x} value {value:#x}",
        material.name, selected.0, selected.1.key.permutation
    );
    selected.0
}
fn metro_record(db: &Database) -> Option<usize> {
    let (material, fields) = stored_material("01_Wall_Metro01Grafitti_Mat")?;
    Some(compatible_record(db, &material, &fields))
}
#[test]
fn original_programs_prove_the_material_frontier_against_microsoft_ref() {
    let Some(path) = install_file(Game::ShaunWhite, "ekshadersPChigh.bin") else {
        return;
    };
    let Some(host) = tool(Tool::RefSm3) else { return };
    let controls = Command::new(&host).arg("--self-test").output().unwrap();
    assert!(
        controls.status.success(),
        "REF controls: {} {}",
        String::from_utf8_lossy(&controls.stdout),
        String::from_utf8_lossy(&controls.stderr)
    );
    assert!(String::from_utf8_lossy(&controls.stdout).contains("REF_CONTROLS_PASS"));
    let db = anvil_formats::shaunwhite::shaders::read(&std::fs::read(path).unwrap()).unwrap();
    let scratch = own_scratch("shaunwhite-ref-albedo");
    let mut gate = GateReport::new(
        "derived albedo / full stored PS on Microsoft D3D9 REF",
        Tolerance::Abs(1e-5),
    );
    let Some(metro) = metro_record(&db) else { return };
    for record in [5, 10, metro] {
        let Payload::Effect(effect) = &db.records[record].payload else {
            panic!("effect")
        };
        let b = binding(&db, record as u32);
        let original = b.clone();
        let derived = derive(&db, record as u32, &b, Influence::Restored).unwrap();
        assert_eq!(b, original);
        eprintln!(
            "DERIVATION record {record}, key {:#x}, {:?}, hash {}, cuts {:?}",
            derived.derivation.key.permutation,
            derived.derivation.projection,
            derived.derivation.source_hash,
            derived.derivation.cuts
        );
        assert!(derived.derivation.cuts.iter().any(|s| s.contains("tone mapping")));
        iridia::emit::wgsl(&derived.program).unwrap();
        let cut = ProgramShader::new(derived.program.clone()).unwrap();
        let tail = ProgramShader::new(derived.reencode.clone()).unwrap();
        let full = ProgramShader::new(iridia_d3d9::lift(effect.pixel.as_ref().unwrap()).unwrap()).unwrap();
        let cases = cases();
        let mut states = Vec::new();
        for &case in &cases {
            for influence in [0., 1.] {
                states.push(state(effect, case, influence));
            }
        }
        let mut light = states[0].clone();
        if record != 5 {
            light.v[1] = [0.25; 4];
        } else {
            light.c[58][0] = 0.5;
        }
        let mut fog = states[0].clone();
        fog.c[32] = [0.8, 0.1, 0.3, 1.];
        fog.c[33] = [0., 0., 0., 0.];
        states.push(light);
        if record != 5 {
            states.push(fog);
        }
        let shader = scratch.join(format!("record-{record}.bin"));
        let input = scratch.join("states.bin");
        let output = scratch.join("rgba.bin");
        std::fs::write(&shader, effect.pixel.as_ref().unwrap()).unwrap();
        std::fs::write(&input, states.iter().flat_map(State::bytes).collect::<Vec<_>>()).unwrap();
        let result = Command::new(&host)
            .arg(shader)
            .arg(input)
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "REF: {} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let bytes = std::fs::read(output).unwrap();
        assert_eq!(bytes.len(), 16 * states.len());
        let native = bytes
            .as_chunks::<16>()
            .0
            .iter()
            .map(|b| std::array::from_fn::<_, 4, _>(|n| f32::from_le_bytes(b.as_chunks::<4>().0[n])))
            .collect::<Vec<_>>();
        for (index, s) in states.iter().enumerate() {
            if index < 8 {
                check_material_bindings(&derived, cases[index / 2], s);
            }
            let raw = execute(&cut, effect, s, None);
            let encoded = execute(&tail, effect, s, Some(raw));
            let lifted = execute(&full, effect, s, None);
            for lane in 0..3 {
                assert!(native[index][lane].is_finite());
                gate.record(
                    format!("record{record} case{index} tail/native lane{lane}"),
                    f64::from((encoded[lane] - native[index][lane]).abs()),
                );
                gate.record(
                    format!("record{record} case{index} full lift/native lane{lane}"),
                    f64::from((lifted[lane] - native[index][lane]).abs()),
                );
                if index < 8 {
                    let wanted = expected(cases[index / 2], effect);
                    gate.record(
                        format!("record{record} case{index} FctDiffuseColor lane{lane}"),
                        f64::from((raw[lane] - wanted[lane]).abs()),
                    );
                }
            }
            eprintln!(
                "REF record{record} case{index}: raw={raw:?}, reencoded={encoded:?}, native={:?}",
                native[index]
            );
        }
        let different = |a: usize, b: usize| (0..3).any(|n| (native[a][n] - native[b][n]).abs() > 0.01);
        assert!(
            different(0, 8),
            "native oracle did not see lighting positive control"
        );
        if record != 5 {
            assert!(different(0, 9), "native oracle did not see fog positive control");
            assert!(
                (0..3).any(|n| (native[6][n] - 1.).abs() < 1e-5),
                "HDR fixture did not reach peak normalization"
            );
        }
    }
    eprintln!("{gate:?}");
    gate.assert_passed();
}

// Original setter outputs, generated by executing its original image instructions, not a second UV implementation.
fn native_uv_rows() -> Option<BTreeMap<String, [f32; 11]>> {
    let path = primase_testdata::oracle(
        primase_testdata::OracleSet::GameCode,
        Game::ShaunWhite,
        "uv-rotation.tsv",
    )?;
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains("c7eb65c89ab016694a2f60a5f586c451a5a02bf05f226bffc943d256128d35c4"));
    Some(
        text.lines()
            .filter(|line| !line.starts_with('#'))
            .map(|line| {
                let mut cells = line.split('\t');
                let name = cells.next().unwrap().to_owned();
                let words = cells
                    .map(|w| f32::from_bits(u32::from_str_radix(w, 16).unwrap()))
                    .collect::<Vec<_>>();
                (name, words.try_into().unwrap())
            })
            .collect(),
    )
}
#[test]
fn stored_rotated_mask_matches_native_setter_and_full_ps_on_ref() {
    use iridia_interface::ConstantRole;
    let Some(path) = install_file(Game::ShaunWhite, "ekshadersPChigh.bin") else {
        return;
    };
    let Some(host) = tool(Tool::RefSm3) else { return };
    let Some(oracle) = native_uv_rows() else { return };
    let controls = Command::new(&host).arg("--self-test").output().unwrap();
    assert!(
        controls.status.success(),
        "{}",
        String::from_utf8_lossy(&controls.stderr)
    );
    assert!(String::from_utf8_lossy(&controls.stdout).contains("REF_PATTERN_CONTROLS_PASS"));
    let db = anvil_formats::shaunwhite::shaders::read(&std::fs::read(path).unwrap()).unwrap();
    let Some((material, fields)) = stored_material("Gen_Metal_Surface_BlueRusty01_Mat") else {
        return;
    };
    assert_eq!(material.id, 0x6e48322c);
    let record = compatible_record(&db, &material, &fields);
    let Payload::Effect(effect) = &db.records[record].payload else {
        panic!("effect")
    };
    let derived = derive(
        &db,
        record as u32,
        &binding(&db, record as u32),
        Influence::Restored,
    )
    .unwrap();
    let helper = primase_shaunwhite::shaders::bind_material(&derived, &material).unwrap();
    let full = ProgramShader::new(iridia_d3d9::lift(effect.pixel.as_ref().unwrap()).unwrap()).unwrap();
    let parsed = iridia_d3d9::read(effect.pixel.as_ref().unwrap()).unwrap();
    let bank = parsed
        .constants
        .unwrap()
        .constants
        .into_iter()
        .find(|c| c.name == "g_VectorArray")
        .unwrap()
        .register as usize;
    let source = material.state.unwrap();
    let case = Case {
        vertex: [0.3, 0.8, 0.5],
        tint: [source.color, source.layering.colors[0], source.layering.colors[1]],
        alpha: [
            f32::from(source.layering.use_alpha[0] && (source.blended || source.alpha_test)),
            f32::from(source.layering.use_alpha[1]),
            f32::from(source.layering.use_alpha[2]),
            0.,
        ],
        ..cases()[0]
    };
    let mut base = state(effect, case, 1.);
    // Neutral frame as in the established full-PS proof. UV words below come ONLY from native execution.
    for (row, name) in [
        (2, "diffuse"),
        (4, "normal"),
        (6, "specular"),
        (9, "diffuse_b"),
        (11, "specular_b"),
        (13, "diffuse_c"),
        (15, "mask"),
    ] {
        let Some(slot) = material.slots.get(name) else {
            continue;
        };
        let native = &oracle[name];
        assert_eq!(
            [
                slot.scale[0],
                slot.scale[1],
                slot.offset[0],
                slot.offset[1],
                slot.rotation
            ],
            native[..5],
            "oracle inputs must match stored {name}"
        );
        base.c[bank + row][..3].copy_from_slice(&native[5..8]);
        base.c[bank + row + 1][..3].copy_from_slice(&native[8..11]);
    }
    base.c[bank + 17] = [source.normal_scale, 1., 1., 1.];
    base.c[bank + 19][0] = source.illumination;
    base.c[bank + 20] = [
        0.,
        material
            .slots
            .get("diffuse_b")
            .map_or(0., |s| f32::from(s.texcoord)),
        0.,
        f32::from(material.slots["mask"].texcoord),
    ];
    // 16x16 point/clamp images; only the mask varies spatially. Every sampler retains its original register.
    let width = 16usize;
    let mut images = Vec::new();
    let mask_slot = (effect
        .samplers
        .iter()
        .find(|s| s.texture == "g_MaskTexture")
        .unwrap()
        .address
        & 65535) as usize;
    for (slot, constant) in base.t.iter().enumerate() {
        for y in 0..width {
            for x in 0..width {
                images.push(if slot == mask_slot {
                    [1., ((x + 3 * y) % 16) as f32 / 15., 0., 1.]
                } else {
                    *constant
                });
            }
        }
    }
    let mut states = Vec::new();
    for y in 0..5 {
        for x in 0..7 {
            let mut s = base.clone();
            let uv = [x as f32 * 0.61 - 0.37, y as f32 * 0.73 - 0.21];
            for v in &mut s.v[2..10] {
                *v = [uv[0], uv[1], uv[0] + 0.13, uv[1] + 0.19];
            }
            states.push(s);
        }
    }
    let scratch = own_scratch("shaunwhite-ref-uv-rotation");
    let shader = scratch.join("original.bin");
    let input = scratch.join("states.bin");
    let output = scratch.join("rgba.bin");
    let textures = scratch.join("images.bin");
    std::fs::write(&shader, effect.pixel.as_ref().unwrap()).unwrap();
    std::fs::write(&input, states.iter().flat_map(State::bytes).collect::<Vec<_>>()).unwrap();
    let mut image_bytes = [0x32474d49u32, 16, 16, 16]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    image_bytes.extend(images.iter().flatten().flat_map(|v| v.to_le_bytes()));
    std::fs::write(&textures, image_bytes).unwrap();
    let result = Command::new(&host)
        .arg(shader)
        .arg(input)
        .arg(&output)
        .arg(textures)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let bytes = std::fs::read(output).unwrap();
    assert_eq!(bytes.len(), states.len() * 16);
    let native = bytes
        .as_chunks::<16>()
        .0
        .iter()
        .map(|b| std::array::from_fn::<_, 4, _>(|n| f32::from_le_bytes(b.as_chunks::<4>().0[n])))
        .collect::<Vec<_>>();
    let mut gate = GateReport::new(
        "stored rotated UV / full PS on Microsoft D3D9 REF",
        Tolerance::Abs(1e-5),
    );
    let mut identity_differs = 0;
    let mut degrees_as_radians_differs = 0;
    for (index, s) in states.iter().enumerate() {
        let mut filled = s.clone();
        for c in &helper.constants {
            if let ConstantRole::Material(words) = &c.role {
                for (i, &word) in words.iter().enumerate() {
                    let at = c.offset as usize / 4 + i;
                    filled.c[at / 4][at % 4] = f32::from_bits(word);
                }
            }
        }
        let mut sample = |request: Sample| {
            let x = (request.uv[0] * width as f32)
                .floor()
                .clamp(0., (width - 1) as f32) as usize;
            let y = (request.uv[1] * width as f32)
                .floor()
                .clamp(0., (width - 1) as f32) as usize;
            Ok(images[(request.texture as usize * width + y) * width + x])
        };
        let actual = execute_sample(&full, effect, &filled, None, &mut sample);
        for lane in 0..3 {
            assert!(native[index][lane].is_finite());
            gate.record(
                format!("UV case{index} lane{lane}"),
                f64::from((actual[lane] - native[index][lane]).abs()),
            );
        }
        let mut identity = filled.clone();
        let slot = material.slots["mask"];
        identity.c[bank + 15] = [slot.scale[0], 0., slot.offset[0], 0.];
        identity.c[bank + 16] = [0., slot.scale[1], slot.offset[1], 0.];
        let wrong = execute_sample(&full, effect, &identity, None, &mut sample);
        if (0..3).any(|n| (wrong[n] - native[index][n]).abs() > 0.01) {
            identity_differs += 1;
        }
        let (sin, cos) = slot.rotation.sin_cos();
        identity.c[bank + 15] = [slot.scale[0] * cos, -slot.scale[0] * sin, slot.offset[0], 0.];
        identity.c[bank + 16] = [slot.scale[1] * sin, slot.scale[1] * cos, slot.offset[1], 0.];
        let wrong = execute_sample(&full, effect, &identity, None, &mut sample);
        if (0..3).any(|n| (wrong[n] - native[index][n]).abs() > 0.01) {
            degrees_as_radians_differs += 1;
        }
    }
    assert!(identity_differs > 0, "pattern must detect omitted rotation");
    assert!(
        degrees_as_radians_differs > 0,
        "pattern must discriminate degrees from radians"
    );
    eprintln!(
        "UV ROTATION material={} record={record} key={:#x} source={} mask-degrees={} identity differs={identity_differs}/{} radians differs={degrees_as_radians_differs}/{} {gate:?}",
        material.name,
        derived.derivation.key.permutation,
        derived.derivation.source_hash,
        material.slots["mask"].rotation,
        states.len(),
        states.len()
    );
    gate.assert_passed();
}
