//! Native REF executes the complete stored PS. The independent source equation checks the raw cut;
//! the original lifted downstream graph re-encodes it for comparison with the native forward output.
#![allow(
    clippy::unwrap_used,
    reason = "explicit fixtures and native oracle assertions"
)]
use primase_core::{
    World,
    gate::{GateReport, Tolerance},
    shader::{KeyPart, Stage, StageBinding, VariantKey},
};
use primase_shader_exec::{PixelShader, ProgramShader, Sample};
use primase_shaunwhite::shaders::{
    Database, Effect, Payload,
    albedo::{Influence, derive},
};
use primase_testdata::{Game, Tool, install_file, own_scratch, tool};
use std::{collections::BTreeMap, process::Command};

fn binding(db: &Database, record: u32) -> StageBinding {
    let shader = World::new().spawn();
    StageBinding {
        stage: Stage::Pixel,
        variant: Some(VariantKey {
            field: "shaunwhite.shader.record".into(),
            value: record,
            parts: vec![
                KeyPart {
                    name: "SDB2 permutation".into(),
                    value: db.records[record as usize].key.permutation,
                },
                KeyPart {
                    name: "native pixel cache key".into(),
                    value: match &db.records[record as usize].payload {
                        Payload::Effect(e) => e.pixel_key,
                        _ => 0,
                    },
                },
            ],
        }),
        shader,
        inputs: vec![],
        constants: vec![],
        textures: vec![],
        buffers: vec![],
        constant_buffers: vec![],
        outputs: vec![],
    }
}
#[test]
fn hand_worked_frontier_is_a_derivation_and_refuses_a_disconnected_colour() {
    use primase_shaunwhite::shaders::{Key, Record, SamplerBinding};
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
    let first_text = primase_shader::emit::wgsl(&d.program).unwrap();
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
    assert_eq!(primase_shader::emit::wgsl(&same.program).unwrap(), first_text);
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
fn state(effect: &Effect, case: Case, influence: f32) -> State {
    let source = primase_d3d9::read(effect.pixel.as_ref().unwrap()).unwrap();
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
                        s.c[row + offset] = [1., 0., 0., 0.];
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
    let source = primase_d3d9::read(effect.pixel.as_ref().unwrap()).unwrap();
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
    p.execute(&v, &[(0, &c)], &mut |request: Sample| {
        Ok(s.t[request.texture as usize])
    })
    .unwrap()
    .outputs[0]
        .map(f32::from_bits)
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
fn metro_record(db: &Database) -> Option<usize> {
    use primase_shaunwhite::{
        components::{MaterialRef, MaterialState},
        passes::material_key,
    };
    let path = install_file(Game::ShaunWhite, "DataPC_01.forge")?;
    let forge = primase_anvil::read::forge::Forge::open(&path).unwrap();
    let index = forge
        .names
        .iter()
        .position(|n| n.name == "01_Q01_UnloadMetroStation_DataBlock")
        .unwrap();
    let mut file = std::fs::File::open(path).unwrap();
    let entry =
        primase_anvil::read::data::read_entry(&forge.payload(&mut file, index).unwrap(), 1 << 30).unwrap();
    let objects =
        primase_anvil::read::objects::read_objects(&entry.metadata.data, &entry.resource.data).unwrap();
    let object = objects
        .iter()
        .find(|o| o.class == 0xb71f6261 && o.name == "01_Wall_Metro01Grafitti_Mat")
        .unwrap();
    let body = &entry.resource.data[object.body.clone()];
    let m = primase_shaunwhite::read::material::Material::parse(body).unwrap();
    let fields = primase_shaunwhite::read::shader_material::read(body).unwrap();
    let material = MaterialRef {
        id: m.id,
        name: object.name.clone(),
        state: Some(MaterialState {
            blend: m.blend,
            blended: m.blended,
            alpha_test: m.alpha_test,
            alpha_ref: m.alpha_ref,
            water: m.water,
            color: m.color,
            illumination: m.illumination,
            specular_level: m.specular_level,
            specular_power: m.specular_power,
            normal_scale: m.normal_scale,
            layering: m.layering,
        }),
        slots: BTreeMap::new(),
        unread: vec![],
    };
    let (mask, value) = material_key(&material, &fields).unwrap();
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
        "METRO exact material {}, record {}, key {:#x}, material mask {mask:#x} value {value:#x}",
        material.name, selected.0, selected.1.key.permutation
    );
    Some(selected.0)
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
    let db = primase_shaunwhite::read::shaders::read(&std::fs::read(path).unwrap()).unwrap();
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
        primase_shader::emit::wgsl(&derived.program).unwrap();
        let cut = ProgramShader::new(derived.program).unwrap();
        let tail = ProgramShader::new(derived.reencode).unwrap();
        let full = ProgramShader::new(primase_d3d9::lift(effect.pixel.as_ref().unwrap()).unwrap()).unwrap();
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
