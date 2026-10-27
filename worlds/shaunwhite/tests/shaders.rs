#![allow(clippy::unwrap_used)]
//! Hand-worked SDB2 source/table/payload fixture; layout is corroborated by the native version-39 loader.
//! Real-file checks frame every record and metadata tail, not a second home-made decoder's output.
mod inputs;

use anvil_formats::shaunwhite::shaders::{Payload, Stage};
use inputs::{Game, install_file};
use primase_core::World;
use primase_shaunwhite::{
    components::ShaderLibrary,
    read::shaders::{read, read_into},
};
use primase_testdata::gate::{GateReport, Tolerance};

fn words(out: &mut Vec<u8>, data: &[u32]) {
    out.extend(data.iter().flat_map(|n| n.to_le_bytes()));
}
fn program(version: u32) -> Vec<u8> {
    [version, 0xffff].into_iter().flat_map(u32::to_le_bytes).collect()
}
fn pair(out: &mut Vec<u8>, kind: u32, value: u32) {
    words(out, &[kind, value]);
}
fn sampler(out: &mut Vec<u8>, extra: bool) {
    out.extend(if extra { b"U\0V\0" } else { b"S\0T\0" });
    words(
        out,
        &[if extra { 2 } else { 0x0080_0003 }, if extra { 0 } else { 3 }],
    );
    if !extra {
        pair(out, 6, 2);
        pair(out, 1, 3);
        pair(out, 2, 1);
    }
}
fn effect(state_only: bool) -> Vec<u8> {
    let mut b = vec![0xef, 0xbe];
    b.extend(if state_only {
        [1, 1, 0, 0, 0, 0]
    } else {
        [1, 2, 1, 1, 1, 0]
    });
    words(
        &mut b,
        if state_only {
            &[0, 0, 0, 0, 0, 0]
        } else {
            &[8, 8, 1, 0, 2, 0]
        },
    );
    b.extend([0; 16]);
    assert_eq!(b.len(), 48);
    if !state_only {
        b.extend(program(0xfffe_0300));
        b.extend(program(0xffff_0300));
    }
    pair(&mut b, 168, 15);
    pair(&mut b, 168, 0);
    if !state_only {
        pair(&mut b, 22, 0);
        sampler(&mut b, false);
        sampler(&mut b, true);
        b.extend(b"g_Cull\0");
    }
    assert_eq!(b.len(), if state_only { 64 } else { 143 });
    b
}
/// Header 24; source header 88 + `abc` 3 -> table at 115. Six 36-byte rows -> 331;
/// five reserved zero bytes -> payloads at 336. Utilities occupy 24 bytes; effect at 360 is 143;
/// state-only at 503 is 64; missing key at 567 has no payload. Final size 567.
fn fixture() -> Vec<u8> {
    let mut b = b"SDB2".to_vec();
    words(&mut b, &[39, 1, 6, 24, 115]);
    let mut name = [0u8; 64];
    name[..7].copy_from_slice(b"test.fx");
    b.extend(name);
    // IEEE CRC-32 of ASCII abc is 352441C2, not computed by the reader for this expected field.
    words(&mut b, &[0x3524_41c2, 3, 1, 0, 0, 0]);
    b.extend(b"abc");
    assert_eq!(b.len(), 115);
    let data = [
        program(0xfffe_0300),
        program(0xffff_0300),
        program(0xffff_0300),
        effect(false),
        effect(true),
        Vec::new(),
    ];
    let mut at = 336;
    for ((kind, key, dependency), payload) in
        [(5, 0, 0), (6, 0, 0), (7, 0, 0), (4, 3, 1), (8, 9, 0), (4, 67, 1)]
            .into_iter()
            .zip(&data)
    {
        words(
            &mut b,
            &[kind, key, 0, dependency, 0, 0, 0, at, payload.len() as u32],
        );
        at += payload.len() as u32;
    }
    assert_eq!(b.len(), 331);
    b.extend([0; 5]);
    for p in data {
        b.extend(p);
    }
    assert_eq!(b.len(), 567);
    b
}

#[test]
fn hand_worked_database_retains_both_programs_and_every_metadata_group() {
    let b = fixture();
    let d = read(&b).unwrap();
    assert_eq!(d.table_padding, 5);
    assert_eq!(d.sources.len(), 1);
    assert_eq!(d.records.len(), 6);
    let s = &d.sources[0];
    assert_eq!(
        (s.name.as_str(), s.crc32, s.dependency),
        ("test.fx", 0x3524_41c2, [1, 0, 0, 0])
    );
    assert_eq!(s.bytes, b"abc");
    let mut gate = GateReport::new("hand-worked SDB2", Tolerance::Exact);
    for (r, (kind, offset, size)) in d.records.iter().zip([
        (5, 336, 8),
        (6, 344, 8),
        (7, 352, 8),
        (4, 360, 143),
        (8, 503, 64),
        (4, 567, 0),
    ]) {
        assert_eq!((r.key.kind, r.offset, r.bytes), (kind, offset, size));
        gate.record(format!("record {kind} at {offset}"), 0.);
    }
    let Payload::Program { stage, bytes } = &d.records[0].payload else {
        panic!()
    };
    assert_eq!(*stage, Stage::Vertex);
    gate.compare_bytes("utility vertex", bytes, &program(0xfffe_0300));
    let Payload::Effect(e) = &d.records[3].payload else {
        panic!()
    };
    assert_eq!((e.vertex_key, e.pixel_key), (1, 2));
    gate.compare_bytes("effect vertex", e.vertex.as_ref().unwrap(), &program(0xfffe_0300));
    gate.compare_bytes("effect pixel", e.pixel.as_ref().unwrap(), &program(0xffff_0300));
    assert_eq!(
        e.states.iter().map(|s| (s.kind, s.value)).collect::<Vec<_>>(),
        [(168, 15)]
    );
    assert_eq!(
        e.restore_states
            .iter()
            .map(|s| (s.kind, s.value))
            .collect::<Vec<_>>(),
        [(168, 0), (22, 0)]
    );
    let s = &e.samplers[0];
    assert_eq!(
        (s.sampler.as_str(), s.texture.as_str(), s.address),
        ("S", "T", 0x0080_0003)
    );
    assert_eq!(
        s.states.iter().map(|s| (s.kind, s.value)).collect::<Vec<_>>(),
        [(6, 2), (1, 3), (2, 1)]
    );
    assert_eq!(
        (
            e.additional_samplers[0].sampler.as_str(),
            e.additional_samplers[0].address
        ),
        ("U", 2)
    );
    assert_eq!(e.variables, ["g_Cull"]);
    let Payload::Effect(s) = &d.records[4].payload else {
        panic!()
    };
    assert!(s.vertex.is_none() && s.pixel.is_none());
    assert_eq!(s.restore_states.len(), 1);
    assert!(matches!(d.records[5].payload, Payload::Missing));
    gate.assert_passed();
    let mut world = World::new();
    read_into(&mut world, &b).unwrap();
    let old = world.resource::<ShaderLibrary>().unwrap().0.clone();
    assert_eq!(*old, d);
    assert!(read_into(&mut world, &b[..566]).is_err());
    assert!(std::sync::Arc::ptr_eq(
        &old,
        &world.resource::<ShaderLibrary>().unwrap().0
    ));
}

#[test]
fn a_stage_program_that_is_not_its_stated_stage_is_refused_where_it_is_stored() {
    let mut b = fixture();
    // the utility vertex record's program (kind 5, at 336) stated as a pixel program
    b[338..340].copy_from_slice(&0xffffu16.to_le_bytes());
    assert!(anvil_formats::shaunwhite::shaders::read(&b).is_ok());
    let error = read(&b).unwrap_err().to_string();
    assert!(error.contains("stage bytecode at 0x150"), "{error}");
}

#[test]
fn corrupt_spans_crcs_tails_and_unknown_versions_fail_by_name() {
    let b = fixture();
    for n in 0..b.len() {
        assert!(read(&b[..n]).is_err(), "accepted truncation {n}");
    }
    for (at, value, name) in [
        (4, 38, "version"),
        (88, 0, "CRC-32"),
        (115, 99, "record kind"),
        (143, 0, "first payload offset"),
        (147, u32::MAX, "record payload"),
        (367, 1, "reserved count"),
        (392, 1, "reserved header"),
        (456, u32::MAX, "sampler state count"),
    ] {
        let mut corrupt = b.clone();
        corrupt[at..at + 4].copy_from_slice(&value.to_le_bytes());
        let error = read(&corrupt).unwrap_err().to_string();
        assert!(error.contains(name), "{at}: {error}");
    }
    let mut trailing = b.clone();
    trailing.push(1);
    assert!(read(&trailing).unwrap_err().to_string().contains("database"));
    let mut tail = b.clone();
    tail[502] = b'!';
    assert!(
        read(&tail)
            .unwrap_err()
            .to_string()
            .contains("runtime state variable")
    );
    for at in 0..b.len() {
        let mut corrupt = b.clone();
        corrupt[at] ^= 128;
        let _ = read(&corrupt);
    }
}

#[test]
fn raw_quality_databases_frame_every_source_record_and_tail() {
    let paths = [
        (
            "high",
            install_file(Game::ShaunWhite, "ekshadersPChigh.bin"),
            6456,
        ),
        (
            "medium",
            install_file(Game::ShaunWhite, "ekshadersPCmed.bin"),
            6456,
        ),
        ("low", install_file(Game::ShaunWhite, "ekshadersPClow.bin"), 3919),
    ];
    for (quality, path, count) in paths {
        let Some(path) = path else { continue };
        let bytes = std::fs::read(path).unwrap();
        let d = read(&bytes).unwrap_or_else(|e| panic!("{quality}: {e}"));
        assert_eq!((d.sources.len(), d.records.len()), (62, count));
        let mut kinds = std::collections::BTreeMap::new();
        let (mut missing, mut vertex, mut pixel, mut samplers, mut states, mut variables) =
            (0, 0, 0, 0, 0, 0);
        for record in &d.records {
            *kinds.entry(record.key.kind).or_insert(0) += 1;
            match &record.payload {
                Payload::Missing => missing += 1,
                Payload::Program { stage, .. } => {
                    if *stage == Stage::Vertex {
                        vertex += 1;
                    } else {
                        pixel += 1;
                    }
                }
                Payload::Effect(e) => {
                    vertex += usize::from(e.vertex.is_some());
                    pixel += usize::from(e.pixel.is_some());
                    samplers += e.samplers.len() + e.additional_samplers.len();
                    states += e.states.len() + e.restore_states.len();
                    variables += e.variables.len();
                }
            }
        }
        if quality == "high" {
            assert_eq!((missing, vertex, pixel), (54, 6393, 6394));
            let Payload::Effect(e) = &d.records[5].payload else {
                panic!()
            };
            assert_eq!(
                e.variables,
                [
                    "g_AlphaBlendEnabled",
                    "g_BlendOp",
                    "g_SrcBlend",
                    "g_DestBlend",
                    "g_ZFunc",
                    "g_Cullmode",
                    "g_ZWriteEnable"
                ]
            );
            assert_eq!(
                (
                    e.samplers[0].sampler.as_str(),
                    e.samplers[0].texture.as_str(),
                    e.samplers[0].address
                ),
                ("InfluenceSampler", "g_InfluenceTexture", 0x0080_000f)
            );
            let proof = iridia_d3d9::lift(e.pixel.as_ref().unwrap()).unwrap();
            assert_eq!(
                proof.provenance.source_hash.as_deref(),
                Some("sha256:82ffb69b8dced07b67e74ed481349f1c68ede096add5379c7b5749ea9b809c0e")
            );
        }
        eprintln!(
            "SDB2 {quality}: {} sources, {} records {kinds:?}, {missing} missing, {vertex} VS, {pixel} PS, {samplers} samplers, {states} state pairs, {variables} runtime state names; exact end {}",
            d.sources.len(),
            d.records.len(),
            bytes.len()
        );
    }
}
