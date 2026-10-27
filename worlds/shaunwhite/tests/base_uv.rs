//! Full original VS on Microsoft REF -> independent passthrough PS -> float readback.
//! The derived graph is evaluated separately on the same raw mesh UV/material controls.
#![allow(clippy::unwrap_used)]
mod support;
use anvil_formats::shaunwhite::shaders::Payload;
use iridia_exec::{PixelShader, ProgramShader, Sample};
use iridia_interface::{Attribute, ConstantRole};
use primase_core::{gate::GateReport, gate::Tolerance};
use primase_shaunwhite::shaders::base_uv::derive;
use primase_testdata::{Game, Tool, install_file, own_scratch, tool};
use std::process::Command;

#[test]
fn stored_nonidentity_base_uv_matches_full_native_vs_with_material_controls() {
    let Some(path) = install_file(Game::ShaunWhite, "ekshadersPChigh.bin") else {
        return;
    };
    let Some(host) = tool(Tool::RefSm3) else {
        return;
    };
    let controls = Command::new(&host).arg("--vertex-self-test").output().unwrap();
    assert!(
        controls.status.success(),
        "REF VS controls failed (host needs VS mode): {} {}",
        String::from_utf8_lossy(&controls.stdout),
        String::from_utf8_lossy(&controls.stderr)
    );
    assert!(String::from_utf8_lossy(&controls.stdout).contains("REF_VERTEX_CONTROLS_PASS"));
    let database = anvil_formats::shaunwhite::shaders::read(&std::fs::read(path).unwrap()).unwrap();
    let Some((material, fields)) = support::material("Cell01251_DataBlock", "Gen_Street_Trottoir01_Mat")
    else {
        return;
    };
    assert_eq!(material.id, 0x6e724a05);
    let slot = material.slots["diffuse"];
    assert_eq!((slot.scale, slot.offset, slot.rotation), ([2., 1.], [0., 0.], 0.));
    let (mask, value) = primase_shaunwhite::passes::material_key(&material, &fields).unwrap();
    // Fixture choice: lowest exact compatible Colour record whose generic VS graph is derivable.
    let record = database
        .records
        .iter()
        .enumerate()
        .find_map(|(i, r)| {
            if r.key.permutation & mask != value {
                return None;
            }
            derive(
                &database,
                i as u32,
                &support::binding(&database, i as u32),
                &material,
            )
            .ok()
            .map(|_| i)
        })
        .expect("stored sidewalk has a derivable compatible Colour record");
    let Payload::Effect(effect) = &database.records[record].payload else {
        panic!("effect");
    };
    let raw = effect.vertex.as_ref().unwrap();
    let source = iridia_d3d9::read(raw).unwrap();
    let full = iridia_d3d9::lift(raw).unwrap();
    let table = source.constants.as_ref().unwrap();
    let scratch = own_scratch("base_uv");
    std::fs::create_dir_all(&*scratch).unwrap();
    let shader = scratch.join("stored-vs.bin");
    let states = scratch.join("states.bin");
    let output = scratch.join("uv.bin");
    std::fs::write(&shader, raw).unwrap();
    let mut packets = Vec::new();
    let mut wanted = Vec::new();
    let mut uploaded = Vec::new();
    let mut gate = GateReport::new(
        "base UV slice / full stored VS on Microsoft D3D9 REF",
        Tolerance::Abs(1e-5),
    );
    let uvs = [[-0.25, 0.125], [0.3, 0.7], [1.25, -2.5], [0., 0.], [3., 4.]];
    // Stored controls first, then identity and deliberately changed rotation/scale/offset controls.
    let variants = [
        material.clone(),
        {
            let mut m = material.clone();
            let s = m.slots.get_mut("diffuse").unwrap();
            s.scale = [1., 1.];
            s.offset = [0., 0.];
            s.rotation = 0.;
            m
        },
        {
            let mut m = material.clone();
            let s = m.slots.get_mut("diffuse").unwrap();
            s.scale = [1.3, 0.7];
            s.offset = [0.2, -0.4];
            s.rotation = 26.27;
            m
        },
    ];
    for variant in &variants {
        let binding = support::binding(&database, record as u32);
        let original = binding.clone();
        let slice = derive(&database, record as u32, &binding, variant).unwrap();
        assert_eq!(binding, original);
        assert_eq!(slice.derivation.source_hash, iridia::source_hash(raw));
        assert_eq!(slice.derivation.vertex_key, effect.vertex_key);
        assert_eq!(slice.derivation.words.len(), 6);
        assert!(
            slice
                .constants
                .iter()
                .any(|c| matches!(c.role, ConstantRole::Unknown { .. }))
        );
        iridia::emit::wgsl(&slice.program).unwrap();
        let executable = ProgramShader::new(slice.program.clone()).unwrap();
        for uv in uvs {
            let mut bank = [[0f32; 4]; 256];
            // Neutral frame used only to rasterize the full VS. These are not production UV bindings.
            for c in &table.constants {
                if matches!(c.name.as_str(), "g_World" | "g_WorldViewProjection" | "g_View") {
                    for n in 0..4 {
                        bank[c.register as usize + n][n] = 1.;
                    }
                }
            }
            for (name, value) in [
                ("g_ViewSpecularParams", [1., 1., 0., 0.]),
                ("g_ViewSpecularLookAtVector", [0., 0., 1., 0.]),
                ("g_SunDirection", [0., 0., 1., 0.]),
                ("g_StaticColorScale", [1.; 4]),
            ] {
                if let Some(c) = table.constants.iter().find(|c| c.name == name) {
                    bank[c.register as usize] = value;
                }
            }
            for c in &slice.constants {
                if let ConstantRole::Material(words) = &c.role {
                    for (i, word) in words.iter().enumerate() {
                        let at = c.offset as usize / 4 + i;
                        bank[at / 4][at % 4] = f32::from_bits(*word);
                    }
                }
            }
            let mut attributes = [[0f32; 4]; 16];
            let mut semantics = [[0u32; 2]; 16];
            let mut inputs = [[0u32; 4]; 16];
            for (i, input) in source.inputs.iter().enumerate() {
                semantics[i] = [input.usage.into(), input.index.into()];
                attributes[i] = match (input.usage, input.index) {
                    (0, 0) => [0., 0., 0., 1.],
                    (5, 0) => [uv[0], uv[1], 0.37, 0.83],
                    (3, 0) => [127.5, 127.5, 255., 0.],
                    _ => [1.; 4],
                };
                inputs[input.register as usize] = attributes[i].map(f32::to_bits);
            }
            let bits = bank.map(|r| r.map(f32::to_bits));
            let got = executable
                .execute(&inputs, &[(0, &bits)], &mut |_: Sample| Err("no texture".into()))
                .unwrap()
                .outputs[0]
                .map(f32::from_bits);
            wanted.push(got);
            let mut packet = Vec::new();
            packet.extend(0x31535652u32.to_le_bytes());
            packet.extend((source.inputs.len() as u32).to_le_bytes());
            packet.extend(bank.into_iter().flatten().flat_map(f32::to_le_bytes));
            packet.extend(semantics.into_iter().flatten().flat_map(u32::to_le_bytes));
            packet.extend(attributes.into_iter().flatten().flat_map(f32::to_le_bytes));
            assert_eq!(packet.len(), 4488);
            packets.extend(packet);
            uploaded.push(uv);
        }
    }
    std::fs::write(&states, packets).unwrap();
    let native = Command::new(&host)
        .arg("--vertex")
        .arg(&shader)
        .arg(&states)
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        native.status.success(),
        "full VS on REF: {} {}",
        String::from_utf8_lossy(&native.stdout),
        String::from_utf8_lossy(&native.stderr)
    );
    let bytes = std::fs::read(output).unwrap();
    assert_eq!(bytes.len(), wanted.len() * 16);
    let native = bytes
        .as_chunks::<16>()
        .0
        .iter()
        .map(|b| std::array::from_fn::<_, 4, _>(|n| f32::from_le_bytes(b.as_chunks::<4>().0[n])))
        .collect::<Vec<_>>();
    for (i, (got, want)) in native.iter().zip(&wanted).enumerate() {
        for lane in 0..4 {
            assert!(got[lane].is_finite());
            gate.record(
                format!("case{i} lane{lane}"),
                f64::from((got[lane] - want[lane]).abs()),
            );
        }
    }
    assert!(
        (0..5).any(|i| (native[i][0] - native[i + 5][0]).abs() > 0.1),
        "identity control did not change native base UV"
    );
    assert!(
        (0..5).any(|i| (native[i + 10][1] - native[i + 5][1]).abs() > 0.1),
        "rotation/offset control did not change native base UV"
    );
    for i in 0..5 {
        gate.record(
            format!("stored scale case{i} independent u"),
            f64::from((wanted[i][0] - 2. * uploaded[i][0]).abs()),
        );
        gate.record(
            format!("stored scale case{i} independent v"),
            f64::from((wanted[i][1] - uploaded[i][1]).abs()),
        );
    }
    eprintln!(
        "BASE UV material={} record={} key={:#x} vertex_key={:#x} source={} {:?}",
        material.name,
        record,
        database.records[record].key.permutation,
        effect.vertex_key,
        iridia::source_hash(raw),
        gate
    );
    gate.assert_passed();
    assert_eq!(full.stage, iridia::Stage::Vertex);
}

fn fixture(
    usage: u32,
    rows: u32,
    matrix_register: u32,
) -> (
    anvil_formats::shaunwhite::shaders::Database,
    primase_shaunwhite::components::MaterialRef,
) {
    use anvil_formats::shaunwhite::{
        material::LayerState,
        shaders::{Database, Effect, Key, Record},
    };
    use primase_shaunwhite::components::{MaterialRef, MaterialState, SlotRef};
    let mut data: Vec<u8> = [
        28u32,
        64,
        0xfffe_0300,
        1,
        28,
        0,
        82,
        68,
        2 | (4 << 16),
        rows,
        48,
        0,
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect();
    data.extend([1u16, 3, 1, 4, 24, 0].into_iter().flat_map(u16::to_le_bytes));
    data.extend(0u32.to_le_bytes());
    data.extend(b"fxc\0g_VectorArray\0vs_3_0\0");
    while !data.len().is_multiple_of(4) {
        data.push(0);
    }
    let payload = [
        vec![u32::from_le_bytes(*b"CTAB")],
        data.as_chunks::<4>()
            .0
            .iter()
            .copied()
            .map(u32::from_le_bytes)
            .collect(),
    ]
    .concat();
    let op = |code: u32, args: &[u32]| [vec![code | ((args.len() as u32) << 24)], args.to_vec()].concat();
    let words = [
        vec![0xfffe_0300, 65534 | ((payload.len() as u32) << 16)],
        payload,
        op(81, &[0xa00f_0000, 1f32.to_bits(), 0, 0, 0]),
        op(31, &[0x80000000, 0x900f0000]),
        op(31, &[0x80000000 | usage, 0x900f0001]),
        op(31, &[0x80000000, 0xe00f0000]),
        op(31, &[0x80070005, 0xe00f0003]),
        op(1, &[0xe00f0000, 0x90e40000]),
        op(4, &[0x80070000, 0x90c40001, 0xa0500000, 0xa0050000]),
        op(8, &[0xe0010003, 0xa0e40000 | matrix_register, 0x80e40000]),
        op(8, &[0xe0020003, 0xa0e40007, 0x80e40000]),
        op(1, &[0xe00c0003, 0xa0550000]),
        vec![65535],
    ]
    .concat();
    let db = Database {
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
                vertex: Some(words.into_iter().flat_map(u32::to_le_bytes).collect()),
                pixel: Some(vec![0; 8]),
                vertex_key: 13,
                pixel_key: 17,
                states: vec![],
                restore_states: vec![],
                samplers: vec![],
                additional_samplers: vec![],
                variables: vec![],
            }),
        }],
    };
    let material = MaterialRef {
        id: 1,
        name: "hand fixture".into(),
        state: Some(MaterialState {
            blend: 0,
            blended: false,
            alpha_test: false,
            alpha_ref: 128,
            water: false,
            two_sided: false,
            invert_culling: false,
            color: [1.; 4],
            illumination: 0.,
            specular_level: 0.,
            specular_power: 1.,
            normal_scale: 1.,
            layering: LayerState {
                colors: [[1.; 4]; 2],
                use_alpha: [false; 3],
            },
        }),
        slots: std::collections::BTreeMap::from([(
            "diffuse".into(),
            SlotRef {
                texture: primase_core::World::new().spawn(),
                scale: [2., 3.],
                offset: [0.25, -0.5],
                rotation: 0.,
                texcoord: 0,
                address: None,
            },
        )]),
        unread: vec![],
    };
    (db, material)
}

#[test]
fn hand_worked_uv_graph_uses_vs_rows_and_keeps_cut_bindings_unknown() {
    let (db, material) = fixture(5, 4, 6);
    let colour = support::binding(&db, 0);
    let original = colour.clone();
    let uv = derive(&db, 0, &colour, &material).unwrap();
    assert_eq!(colour, original);
    assert_eq!(uv.derivation.name, "shaunwhite.generic-base-uv.v1");
    assert_eq!(uv.derivation.vertex_key, 13);
    assert_eq!(
        uv.derivation.words.iter().map(|(_, w)| *w).collect::<Vec<_>>(),
        [24, 25, 26, 28, 29, 30]
    );
    assert!(uv.inputs.iter().any(
        |i| matches!(&i.role,iridia_interface::InputRole::Lanes(l) if l[0].semantic==Attribute::TexCoord(0))
    ));
    assert!(
        uv.constants
            .iter()
            .any(|c| matches!(c.role, ConstantRole::Unknown { .. }))
    );
    let mut constants = [[0u32; 4]; 8];
    for c in &uv.constants {
        if let ConstantRole::Material(words) = &c.role {
            for (i, &word) in words.iter().enumerate() {
                let n = c.offset as usize / 4 + i;
                constants[n / 4][n % 4] = word;
            }
        }
    }
    let got = ProgramShader::new(uv.program)
        .unwrap()
        .execute(
            &[
                [f32::NAN.to_bits(); 4],
                [0.5f32, 0.25, 99., 42.].map(f32::to_bits),
            ],
            &[(0, &constants)],
            &mut |_: Sample| Err("no texture".into()),
        )
        .unwrap();
    assert_eq!(got.outputs[0].map(f32::from_bits), [1.25, 0.25, 0., 1.]);
}

#[test]
fn unknown_dependencies_incomplete_matrices_and_wrong_records_are_refused() {
    for (usage, rows, register, reason) in [
        (3, 4, 6, "unresolved/non-material"),
        (5, 3, 6, "allocation incomplete"),
        (5, 4, 8, "unresolved/non-material"),
    ] {
        let (db, m) = fixture(usage, rows, register);
        let error = derive(&db, 0, &support::binding(&db, 0), &m).unwrap_err();
        assert!(error.contains(reason), "{error}");
    }
    let (db, mut m) = fixture(5, 4, 6);
    let mut b = support::binding(&db, 0);
    b.variant.as_mut().unwrap().parts[0].value ^= 1;
    assert!(derive(&db, 0, &b, &m).unwrap_err().contains("exact Colour"));
    m.slots.get_mut("diffuse").unwrap().rotation = f32::NAN;
    assert!(
        derive(&db, 0, &support::binding(&db, 0), &m)
            .unwrap_err()
            .contains("non-finite")
    );
    m.slots.remove("diffuse");
    assert!(
        derive(&db, 0, &support::binding(&db, 0), &m)
            .unwrap_err()
            .contains("slot fields not read")
    );
}
