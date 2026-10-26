//! Exact material constraints and stored-record variants. Full forward colour is not an albedo derivation.
#![allow(clippy::unwrap_used, reason = "explicit fixture and gate assertions")]
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::shader::{ConstantRole, MaterialPrograms, OutputRole, Shader, ShaderBinding, Stage};
use primase_core::{Mesh, Primitive, Semantic, Stream, Topology, World};
use primase_shaunwhite::components::{MaterialRef, MaterialState, MeshFormat, ShaderLibrary, ShaderPasses};
use primase_shaunwhite::passes::{RECORD_FIELD, material_key};
use primase_shaunwhite::shaders::{Database, Effect, Key, Payload, Record};
use primase_testdata::{Game, install_file};
use std::collections::BTreeMap;
use std::sync::Arc;

fn run(w: &mut World) -> RunReport {
    let mut registry = PassRegistry::new();
    primase_shaunwhite::register(&mut registry);
    let pipeline = registry.parse("resolve-shaunwhite-shaders").unwrap();
    let mut report = RunReport::default();
    PassManager::new(registry)
        .with_pass_checks(true)
        .run(w, &pipeline, &mut report)
        .unwrap();
    report
}
fn body() -> Vec<u8> {
    let mut b = vec![0; 647];
    b[0] = 1;
    b[1..5].copy_from_slice(&1u32.to_le_bytes());
    b[5..9].copy_from_slice(&0xb71f6261u32.to_le_bytes());
    b
}
fn material(b: &[u8]) -> MaterialRef {
    let m = primase_shaunwhite::read::material::Material::parse(b).unwrap();
    MaterialRef {
        id: m.id,
        name: "fixture".into(),
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
        unread: Vec::new(),
    }
}

/// CTAB header 0..28, allocation 28..48, float4 type 48..64, strings at64/68/93.
/// It assigns the material tint to c0; MOV reads that register into oC0.
fn reflected_pixel() -> Vec<u8> {
    let mut table: Vec<u8> = [28u32, 64, 0xffff0300, 1, 28, 0, 93, 68, 2, 1, 48, 0]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    table.extend([1u16, 3, 1, 4, 1, 0].into_iter().flat_map(u16::to_le_bytes));
    table.extend(0u32.to_le_bytes());
    assert_eq!(table.len(), 64);
    table.extend(b"fxc\0g_Layer1_ModulationColor\0ps_3_0\0");
    assert_eq!(table.len(), 100);
    let mut out = Vec::new();
    out.extend(0xffff0300u32.to_le_bytes());
    out.extend((0xfffeu32 | (26 << 16)).to_le_bytes());
    out.extend(b"CTAB");
    out.extend(table);
    out.extend(
        [0x02000001u32, 0x800f0800, 0xa0e40000, 0xffff]
            .into_iter()
            .flat_map(u32::to_le_bytes),
    );
    out
}

#[test]
fn source_controls_make_required_bits_without_choosing_a_frame() {
    let mut b = body();
    b[26] = 1;
    b[27] = 128;
    b[122] = 1;
    b[123] = 1;
    b[553] = 1;
    b[525..529].copy_from_slice(&2u32.to_le_bytes());
    let f = primase_shaunwhite::read::shader_material::read(&b).unwrap();
    assert_eq!(f.flags, (1 << 5) | (128 << 6));
    let (mask, value) = material_key(&material(&b), &f).unwrap();
    // Template3, alpha test19, layers30/31, half-Lambert36, kill41, no-specular50, inverse56.
    assert_eq!(
        value,
        3 | (1 << 19) | (1 << 30) | (1 << 31) | (1 << 36) | (1 << 41) | (1 << 50) | (1 << 56)
    );
    assert_eq!(
        mask & ((1 << 7) | (1 << 17) | (1 << 24) | (1 << 39) | (1 << 55)),
        0
    );
    b[44] = 1;
    let f = primase_shaunwhite::read::shader_material::read(&b).unwrap();
    assert_eq!(f.flags & (1 << 29), 1 << 29);
    assert_eq!(material_key(&material(&b), &f).unwrap().1 & (1 << 36), 0);
    for (at, value) in [(44, 2u32), (525, 3), (496, 3), (28, 4)] {
        let mut bad = b.clone();
        bad[at..at + 4].copy_from_slice(&value.to_le_bytes());
        assert!(
            primase_shaunwhite::read::shader_material::read(&bad).is_err(),
            "{at}"
        );
    }
}

#[test]
fn exact_record_variants_keep_unknown_constants_and_forward_output() {
    let mut w = World::new();
    let b = body();
    let m = material(&b);
    let fields = primase_shaunwhite::read::shader_material::read(&b).unwrap();
    let key = material_key(&m, &fields).unwrap().1 | (2 << 20);
    let host = w.spawn();
    w.insert(host, m);
    w.insert(host, fields);
    let proto = w.spawn();
    w.insert(
        proto,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(
                Semantic::Position,
                Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]]),
            )]),
            indices: Some(vec![0, 1, 2]),
            material: Some(host),
        }]),
    );
    w.insert(proto, MeshFormat { stride: 12, flags: 0 });
    // ps_3_0; mov oC0,c0; END. An unreflected external float register is not silently zero.
    let bytes: Vec<_> = [0xffff0300u32, 0x02000001, 0x800f0800, 0xa0e40000, 0xffff]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let mut records = Vec::new();
    for (i, key) in [key, key | (1 << 7), key | (1 << 30)].into_iter().enumerate() {
        records.push(Record {
            key: Key {
                kind: 4,
                permutation: key,
                dependencies: [0; 4],
            },
            offset: 0,
            bytes: 0,
            payload: Payload::Effect(Effect {
                vertex: None,
                pixel: Some(if i == 1 { reflected_pixel() } else { bytes.clone() }),
                vertex_key: 0,
                pixel_key: 123,
                states: Vec::new(),
                restore_states: Vec::new(),
                samplers: Vec::new(),
                additional_samplers: Vec::new(),
                variables: Vec::new(),
            }),
        });
        assert_eq!(records.len(), i + 1);
    }
    w.insert_resource(ShaderLibrary(Arc::new(Database {
        sources: Vec::new(),
        records,
        table_padding: 0,
    })));
    let r = run(&mut w);
    assert_eq!(w.get::<ShaderPasses>(host).unwrap().0, [0, 1]);
    let binding = w.get::<ShaderBinding>(host).unwrap();
    assert_eq!(binding.stages.len(), 2);
    assert_eq!(w.iter::<Shader>().count(), 2);
    for (i, stage) in binding.stages.iter().enumerate() {
        let variant = stage.variant.as_ref().unwrap();
        assert_eq!((variant.field.as_str(), variant.value), (RECORD_FIELD, i as u32));
        if i == 0 {
            assert!(matches!(stage.constants[0].role, ConstantRole::Unknown { .. }));
        } else {
            assert_eq!(stage.constants[0].role, ConstantRole::Material(vec![0; 4]));
        }
        assert!(stage.outputs.iter().any(|o| o.role == OutputRole::Colour));
        assert!(!stage.outputs.iter().any(|o| o.role == OutputRole::Albedo));
    }
    assert_eq!(w.get::<MaterialPrograms>(host).unwrap().0.len(), 2);
    assert_eq!(r.steps[0].log.counts["stored colour record variants"], 2);
}

#[test]
fn six_levels_resolve_and_count_every_supported_or_unresolved_material() {
    let files = [
        install_file(Game::ShaunWhite, "DataPC_01.forge"),
        install_file(Game::ShaunWhite, "DataPC_02.forge"),
        install_file(Game::ShaunWhite, "DataPC_03.forge"),
        install_file(Game::ShaunWhite, "DataPC_04.forge"),
        install_file(Game::ShaunWhite, "DataPC_05.forge"),
        install_file(Game::ShaunWhite, "DataPC_06.forge"),
    ];
    let Some(shader_file) = install_file(Game::ShaunWhite, "ekshadersPChigh.bin") else {
        return;
    };
    let Some(files) = files.into_iter().collect::<Option<Vec<_>>>() else {
        return;
    };
    let db = Arc::new(primase_shaunwhite::read::shaders::read(&std::fs::read(shader_file).unwrap()).unwrap());
    for (level, path) in files.iter().enumerate() {
        let shared: Vec<_> = files.iter().filter(|p| *p != path).cloned().collect();
        let (mut w, read_report) = primase_shaunwhite::read::level::read_level(path, &shared).unwrap();
        let field_failures: Vec<_> = read_report
            .iter()
            .filter(|(_, t)| t.starts_with("material shader fields not read"))
            .collect();
        eprintln!("KEYFIELDS L{:02} {field_failures:?}", level + 1);
        let materials = w.iter::<MaterialRef>().count();
        w.insert_resource(ShaderLibrary(db.clone()));
        let report = run(&mut w);
        let log = &report.steps[0].log;
        let resolved = log
            .counts
            .get("materials resolved to compatible colour variants")
            .copied()
            .unwrap_or(0);
        let unresolved: u64 = log
            .omitted
            .iter()
            .filter(|(n, _)| n.starts_with("materials unresolved:"))
            .map(|(_, n)| n)
            .sum();
        assert_eq!(resolved + unresolved, materials as u64);
        assert!(resolved > 0, "no resolved materials in level {}", level + 1);
        assert!(
            w.iter::<ShaderBinding>()
                .all(|(_, b)| b.stages.iter().all(|s| s.stage == Stage::Pixel))
        );
        eprintln!(
            "COVERAGE L{:02}: resolved={resolved}; variants={}; unresolved={unresolved}; reasons={:?}",
            level + 1,
            log.counts["stored colour record variants"],
            log.omitted
        );
        let unknown: u64 = log
            .counts
            .iter()
            .filter(|(n, _)| n.starts_with("unresolved binding:"))
            .map(|(_, n)| n)
            .sum();
        let textures = w
            .iter::<ShaderBinding>()
            .flat_map(|(_, b)| &b.stages)
            .flat_map(|s| &s.textures)
            .filter(|t| matches!(t.source, primase_core::shader::TextureSource::Texture { .. }))
            .count();
        let tints = w
            .iter::<ShaderBinding>()
            .flat_map(|(_, b)| &b.stages)
            .flat_map(|s| &s.constants)
            .filter(|c| matches!(c.role, ConstantRole::Material(_)))
            .count();
        assert!(
            textures > 0 && tints > 0,
            "no source texture/tint bindings in level {}",
            level + 1
        );
        eprintln!("BOUND L{:02}: textures={textures}; tints={tints}", level + 1);
        eprintln!(
            "BINDINGS L{:02}: unresolved occurrences={unknown}; programs={}",
            level + 1,
            log.counts["unique pixel programs"]
        );
    }
}
