//! Shader permutations and program records: hand-laid permutations read to their ids and GUIDs, the refusals, a GUID's
//! bytecode name, program records read from a hand-laid program lookup, a shared data's fixed-function state read in
//! the layouts the game's own state printers use (`Skate.exe`'s rc2 IR filter: the oracle, see
//! `frostbite_formats::permutation::FixedState`); and, on the game's data (the skate. install and its Oodle 9 library),
//! every program of the first program database of `Win32/globals.toc` followed through its permutations to bytecode
//! that reads, a material key's programs read into a world, and every stock raster state decoded.

#![allow(clippy::unwrap_used)]

use frostbite_formats::lookup::{self, RECORD_LEN};
use frostbite_formats::permutation::{self, COMPUTE, RASTER};

/// Metadata: the payload size, the relocation size, the resource's id.
fn meta(size: u32, reloc: u32, id: u64) -> [u8; 16] {
    let mut m = [0u8; 16];
    m[0..4].copy_from_slice(&size.to_le_bytes());
    m[4..8].copy_from_slice(&reloc.to_le_bytes());
    m[8..16].copy_from_slice(&id.to_le_bytes());
    m
}

const G0: [u8; 16] = [
    0xa5, 0xfa, 0x7a, 0x26, 0x0a, 0x1f, 0xf7, 0x3d, 0x33, 0x84, 0xf6, 0x6a, 0x01, 0x00, 0x28, 0x04,
];
const G1: [u8; 16] = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
];

/// A raster permutation with the given GUIDs, laid as the game's are.
fn raster(guids: &[[u8; 16]]) -> (Vec<u8>, [u8; 16]) {
    let mut d = Vec::new();
    d.extend(0x1111_2222_3333_4444u64.to_le_bytes());
    d.extend(0x20u64.to_le_bytes());
    d.extend(0x5555_6666_7777_8888u64.to_le_bytes());
    d.extend(0u64.to_le_bytes());
    for g in guids {
        d.extend(g);
    }
    let size = d.len() as u32;
    d.extend(8u32.to_le_bytes());
    (d, meta(size, 4, 0x99))
}

#[test]
fn a_raster_permutation_reads_to_its_ids_and_programs() {
    let (d, m) = raster(&[G0, G1]);
    let p = permutation::read(RASTER, &d, &m).unwrap();
    assert_eq!(p.kind, permutation::Kind::Raster);
    assert_eq!((p.id, p.shared_data), (0x99, 0x1111_2222_3333_4444));
    assert_eq!(p.input_layout, Some(0x5555_6666_7777_8888));
    assert_eq!(p.bytecode, [G0, G1]);
    // a vertex program alone
    let (d, m) = raster(&[G0]);
    assert_eq!(permutation::read(RASTER, &d, &m).unwrap().bytecode, [G0]);
}

#[test]
fn a_compute_permutation_reads_to_its_program() {
    let mut d = Vec::new();
    d.extend(0xAAAAu64.to_le_bytes());
    d.extend(0x10u64.to_le_bytes());
    d.extend(G1);
    d.extend(8u32.to_le_bytes());
    let p = permutation::read(COMPUTE, &d, &meta(0x20, 4, 7)).unwrap();
    assert_eq!(
        (p.kind, p.id, p.shared_data, p.input_layout),
        (permutation::Kind::Compute, 7, 0xAAAA, None)
    );
    assert_eq!(p.bytecode, [G1]);
}

#[test]
fn a_malformed_permutation_is_refused() {
    let (d, m) = raster(&[G0, G1]);
    let refused = |d: &[u8], m: &[u8; 16], t: u32| permutation::read(t, d, m).unwrap_err().to_string();
    // another type, a metadata that does not match the payload
    assert!(refused(&d, &m, 0x1234).contains("not a permutation"));
    assert!(refused(&d[..d.len() - 1], &m, RASTER).contains("relocations"));
    // a relocation that is not the pointer at 8
    let mut bad = d.clone();
    let n = bad.len();
    bad[n - 4] = 16;
    assert!(refused(&bad, &m, RASTER).contains("pointer at 8"));
    // a pointer that is not where the GUIDs start
    let mut bad = d.clone();
    bad[8] = 0x30;
    assert!(refused(&bad, &m, RASTER).contains("GUIDs"));
    // three programs
    let (d3, m3) = raster(&[G0, G1, G1]);
    assert!(refused(&d3, &m3, RASTER).contains("3 programs"));
    // a compute permutation of the raster layout's size
    assert!(refused(&d, &m, COMPUTE).contains("GUIDs"));
}

/// A GUID's bytecode name: its first three fields are little endian as stored, the rest in order; the example is a
/// CompiledBytecode resource of the install and the bytes its permutation holds.
#[test]
fn a_guid_names_its_bytecode_resource() {
    assert_eq!(
        permutation::bytecode_name(&G0),
        "shaders/bytecode/267afaa5-1f0a-3df7-3384-f66a01002804"
    );
    assert_eq!(
        permutation::bytecode_name(&G1),
        "shaders/bytecode/04030201-0605-0807-090a-0b0c0d0e0f10"
    );
}

/// Program records read from a hand-laid program section: the count and the offset of the first at the start, each
/// record's key, and its permutation ids by the count at 0x60 and the pointer at 0x64.
#[test]
fn program_records_name_their_permutations() {
    let (count, first) = (2usize, 0x20usize);
    let ids_at = first + count * RECORD_LEN;
    let mut d = vec![0u8; ids_at + 24];
    d[0..4].copy_from_slice(&(count as u32).to_le_bytes());
    d[4..12].copy_from_slice(&(first as u64).to_le_bytes());
    for (i, (key, ids)) in [(0xAAu64, vec![5u64, 6]), (0xBB, vec![7])]
        .into_iter()
        .enumerate()
    {
        let at = first + i * RECORD_LEN;
        d[at..at + 8].copy_from_slice(&key.to_le_bytes());
        let ptr = ids_at + if i == 0 { 0 } else { 16 };
        d[at + 0x60..at + 0x64].copy_from_slice(&(ids.len() as u32).to_le_bytes());
        d[at + 0x64..at + 0x6c].copy_from_slice(&(ptr as u64).to_le_bytes());
        for (j, id) in ids.iter().enumerate() {
            d[ptr + 8 * j..ptr + 8 * j + 8].copy_from_slice(&id.to_le_bytes());
        }
    }
    let program = d.len();
    // the lookup rows after the program section and an empty relocation list: none (count 0)
    d.extend(0u32.to_le_bytes());
    let mut m = [0u8; 16];
    m[0..4].copy_from_slice(&(program as u32).to_le_bytes());
    m[8..12].copy_from_slice(&4u32.to_le_bytes());
    let records = lookup::read_records(&d, &m).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(
        (records[0].key, &records[0].permutations[..]),
        (0xAA, &[5u64, 6][..])
    );
    assert_eq!(
        (records[1].key, &records[1].permutations[..]),
        (0xBB, &[7u64][..])
    );
    assert_eq!(records[1].raw[0], 0xBB);
    // a permutation array past the section, and records past it
    let mut bad = d.clone();
    bad[first + 0x64] = 0xf0;
    bad[first + 0x65] = 0xff;
    assert!(lookup::read_records(&bad, &m).is_err());
    bad = d.clone();
    bad[0] = 9;
    assert!(lookup::read_records(&bad, &m).is_err());
}

/// A raster shared data laid as the game's: the ids, zeros to 0x78, then `state` (0x78 to 0xb4).
fn shared_with_state(state: &[u8; 0x3c]) -> (Vec<u8>, [u8; 16]) {
    let mut d = vec![0u8; 0xc0];
    d[0x08..0x10].copy_from_slice(&1u64.to_le_bytes());
    d[0x38..0x40].copy_from_slice(&2u64.to_le_bytes());
    d[0x78..0xb4].copy_from_slice(state);
    let m = meta(d.len() as u32, 0, 7);
    (d, m)
}

/// The stock decal row's state (hand-worked from the game's printers' layouts, permutation.rs's table): bias 200,
/// slope 0.1, cull back, front counter-clockwise, depth clip; depth test GreaterEqual, no write, stencil test reading
/// 0x08 and writing nothing, both faces keep / keep / replace / GreaterEqual; logic op 4 off, independent blend; three
/// premultiplied targets (One, InvSourceAlpha; masks rgb, rgb, gb) and five unblended ones.
fn decal_state() -> [u8; 0x3c] {
    let mut s = [0u8; 0x3c];
    s[0..4].copy_from_slice(&0.1f32.to_le_bytes());
    s[4..6].copy_from_slice(&200i16.to_le_bytes());
    s[7] = 0x1c;
    s[8..16].copy_from_slice(&[0x35, 0x08, 0x00, 0x00, 0x32, 0x00, 0x32, 0x44]);
    s[16] = 0xff;
    let records: [[u8; 5]; 8] = [
        [0x03, 0x05, 0x01, 0x05, 0x07],
        [0x03, 0x05, 0x01, 0x05, 0x07],
        [0x03, 0x05, 0x01, 0x05, 0x06],
        [0x02, 0x00, 0x01, 0x00, 0x0f],
        [0x02, 0x00, 0x01, 0x00, 0x0f],
        [0x02, 0x00, 0x01, 0x00, 0x0f],
        [0x02, 0x00, 0x01, 0x00, 0x0f],
        [0x02, 0x00, 0x01, 0x00, 0x0f],
    ];
    for (i, r) in records.iter().enumerate() {
        s[0x14 + 5 * i..0x19 + 5 * i].copy_from_slice(r);
    }
    s
}

#[test]
fn a_shared_datas_fixed_state_reads_in_the_games_layout() {
    use frostbite_formats::permutation::{BlendOp, Compare, Cull, Fill, StencilFace, StencilOp};
    let (d, m) = shared_with_state(&decal_state());
    let s = permutation::read_shared(permutation::SHARED_RASTER, &d, &m)
        .unwrap()
        .fixed_state()
        .unwrap();
    assert_eq!((s.depth_bias, s.slope_scaled_depth_bias), (200, 0.1));
    assert_eq!((s.fill, s.cull), (Fill::Solid, Cull::Back));
    assert!(s.front_counter_clockwise && s.depth_clip && !s.multisample && !s.conservative);
    assert!(s.depth_test && !s.depth_write && s.stencil_test);
    assert_eq!(s.depth_compare, Compare::GreaterEqual);
    assert_eq!((s.stencil_read_mask, s.stencil_write_mask), (0x08, 0x00));
    let face = StencilFace {
        fail: StencilOp::Keep,
        depth_fail: StencilOp::Keep,
        pass: StencilOp::Replace,
        compare: Compare::GreaterEqual,
    };
    assert_eq!((s.front, s.back), (face, face));
    assert_eq!(s.logic_op, 4);
    assert!(!s.logic_op_enable && !s.alpha_to_coverage && s.independent_blend);
    let t = s.targets[0];
    assert!(t.blend);
    assert_eq!(
        (
            t.source,
            t.destination,
            t.source_alpha,
            t.destination_alpha,
            t.alpha_op,
            t.write_mask
        ),
        (1, 5, 1, 5, BlendOp::Add, 0x7)
    );
    assert_eq!(s.targets[2].write_mask, 0x6);
    assert!(!s.targets[3].blend);
    assert_eq!((s.targets[3].source, s.targets[3].write_mask), (1, 0xf));
}

#[test]
fn a_fixed_state_out_of_the_games_ranges_is_refused() {
    let refused = |state: [u8; 0x3c]| {
        let (d, m) = shared_with_state(&state);
        permutation::read_shared(permutation::SHARED_RASTER, &d, &m)
            .unwrap()
            .fixed_state()
            .unwrap_err()
            .to_string()
    };
    let mut cull = decal_state();
    cull[7] = 0x06;
    assert!(refused(cull).contains("cull mode 3"));
    let mut op = decal_state();
    op[0x15] = 0x25;
    assert!(refused(op).contains("colour blend operation"));
    let mut mode = decal_state();
    mode[0x16] = 0x11;
    assert!(refused(mode).contains("past the game's 17"));
    // a payload that ends before the state
    let (mut d, _) = shared_with_state(&decal_state());
    d.truncate(0x90);
    let m = meta(d.len() as u32, 0, 7);
    let short = permutation::read_shared(permutation::SHARED_RASTER, &d, &m).unwrap();
    assert!(
        short
            .fixed_state()
            .unwrap_err()
            .to_string()
            .contains("hold no state")
    );
}
