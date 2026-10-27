use morpheme_formats::{bundle_be32, rig_be32};

fn u(b: &mut [u8], at: usize, value: u32) {
    b[at..at + 4].copy_from_slice(&value.to_be_bytes());
}
fn floats(b: &mut [u8], at: usize, values: &[f32]) {
    for (i, value) in values.iter().enumerate() {
        u(b, at + i * 4, value.to_bits());
    }
}

// Literal 32-bit BE fixture. All four pointer bases differ; relocated runtime pointers are deliberately garbage.
fn fixture() -> Vec<u8> {
    let mut b = vec![0x78; 0x200];
    floats(&mut b, 0, &[0., 0., 0., 1., 7., 8., 9., 0.]);
    u(&mut b, 0x20, 0x60);
    u(&mut b, 0x44, 0x80);
    u(&mut b, 0x48, 0xc0);
    u(&mut b, 0x60, 2);
    u(&mut b, 0x64, 0xdeadbeef);
    u(&mut b, 0x68, u32::MAX);
    u(&mut b, 0x6c, 0);
    for (at, value) in [
        (0x80, 2),
        (0x84, 9),
        (0x88, 0x14),
        (0x8c, 0x1c),
        (0x90, 0x24),
        (0x94, 1),
        (0x98, 0),
        (0x9c, 5),
        (0xa0, 0),
        (0xc0, 0xd0000),
        (0xd0, 0x40),
        (0x100, 0x100),
        (0x104, 16),
        (0x108, 2),
        (0x10c, 2),
        (0x110, 0x30),
        (0x114, 0x48),
        (0x118, 0x20),
        (0x130, 6),
        (0x134, 16),
        (0x138, 16),
        (0x13c, 2),
        (0x140, 16),
        (0x144, 16),
        (0x148, 0x80),
        (0x14c, 0x60),
    ] {
        u(&mut b, at, value);
    }
    b[0xa4..0xad].copy_from_slice(b"root\0tip\0");
    floats(&mut b, 0x160, &[3., 4., 5., 0.]);
    u(&mut b, 0x16c, 0x7fc12345);
    floats(&mut b, 0x170, &[2., 0., 0., 0.]);
    u(&mut b, 0x17c, 0xdeadbeef);
    let s = std::f32::consts::FRAC_1_SQRT_2;
    floats(&mut b, 0x180, &[0., 0., s, s, s, 0., 0., s]);
    b
}

#[test]
fn hand_worked_rig_and_all_pointer_bases() {
    let rig = rig_be32(&fixture(), 32).expect("literal rig");
    assert_eq!(
        rig.header_vector_bits,
        [7f32.to_bits(), 8f32.to_bits(), 9f32.to_bits(), 0]
    );
    assert_eq!(rig.joints.len(), 2);
    let root = &rig.joints[0];
    let tip = &rig.joints[1];
    assert_eq!(
        (root.id, root.name.as_slice(), root.parent),
        (0, b"root".as_slice(), None)
    );
    assert_eq!(
        (tip.id, tip.name.as_slice(), tip.parent),
        (1, b"tip".as_slice(), Some(0))
    );
    assert_eq!(root.translation, [3., 4., 5.]);
    assert_eq!(tip.translation, [2., 0., 0.]);
    assert_eq!(root.position_w_bits, 0x7fc12345);
    assert_eq!(tip.position_w_bits, 0xdeadbeef);
    let mut repeated = fixture();
    u(&mut repeated, 0x84, 10);
    repeated[0xa4..0xae].copy_from_slice(b"same\0same\0");
    let repeated = rig_be32(&repeated, 0).expect("repeated names");
    assert_eq!(repeated.joints[0].name, repeated.joints[1].name);
}

#[test]
fn precise_refusals_and_all_truncated_prefixes() {
    let b = fixture();
    // The declared reference-buffer span includes its auxiliary bytes and must be present in full.
    for end in 0..b.len() {
        assert!(rig_be32(&b[..end], 17).is_err(), "prefix {end:x}");
    }
    for (at, value, detail) in [
        (0x20, u32::MAX, "outside"),
        (0x68, 1, "cyclic"),
        (0x6c, 2, "parent outside"),
        (0x6c, 0xfffffffe, "negative parent"),
        (0x94, 0, "name ID"),
        (0x84, 1, "outside"),
        (0x108, 3, "reference joint count"),
        (0x130, 99, "unsupported reference"),
        (0x134, 8, "unsupported reference"),
        (0x13c, 6, "duplicate reference"),
        (0x10c, 1, "missing position"),
        (0x160, 0x7f800000, "nonfinite reference"),
        (0x180, 0x7fc00000, "quaternion"),
        (0x18c, 0, "nonunit"),
    ] {
        let mut bad = b.clone();
        u(&mut bad, at, value);
        let error = rig_be32(&bad, 17).expect_err(detail);
        assert!(error.offset >= 17, "{error}");
        assert!(error.detail.contains(detail), "{detail}: {error}");
    }
    let mut no_nul = b.clone();
    no_nul[0xa4..0xad].fill(0xff);
    let mut parent = b.clone();
    u(&mut parent, 0x6c, 2);
    assert_eq!(
        rig_be32(&parent, 17).expect_err("parent source offset").offset,
        17 + 0x6c
    );
    let mut translation = b.clone();
    u(&mut translation, 0x164, 0x7fc00000);
    assert_eq!(
        rig_be32(&translation, 17)
            .expect_err("translation source offset")
            .offset,
        17 + 0x164
    );
    assert!(
        rig_be32(&no_nul, 0)
            .expect_err("unterminated")
            .detail
            .contains("unterminated")
    );
}

#[test]
fn bounded_sibling_unknown_record_padding_and_eof() {
    let mut b = Vec::new();
    for (typ, payload) in [(1u32, fixture()), (0x12345678, vec![5, 6, 7])] {
        b.extend_from_slice(&typ.to_be_bytes());
        b.extend_from_slice(&91u32.to_be_bytes());
        b.extend_from_slice(&[4; 16]);
        b.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        b.extend_from_slice(&0xabcdef01u32.to_be_bytes());
        b.extend_from_slice(&payload);
        b.resize((b.len() + 15) & !15, 0xef);
    }
    let records = bundle_be32(&b).expect("bundle");
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].offset, 0x220);
    assert_eq!(records[1].payload, [5, 6, 7]);
    assert_eq!(records[1].padding, [0xef; 13]);
    assert_eq!(records[0].guid, [4; 16]);
    assert_eq!(records[0].header_word, 0xabcdef01);
    rig_be32(records[0].payload, 32).expect("rig with unknown sibling");
    for length in [1, 3, 31, 0x21f, b.len() - 1] {
        assert!(bundle_be32(&b[..length]).is_err());
    }
    b.push(0);
    assert!(bundle_be32(&b).is_err());
}
