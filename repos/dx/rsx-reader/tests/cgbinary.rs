use rsx_reader::cgbinary::{Stage, parse, parse_prefix};

fn word(b: &mut [u8], at: usize, value: u32) {
    b[at..at + 4].copy_from_slice(&value.to_be_bytes());
}

// Hand laid out, not a round trip through a writer: header 0..32,
// parameter 32..80, descriptor 80..104, strings 104..118,
// float2 defaults 120..128, patch table 128..140, microcode 144..176.
fn fixture(fragment: bool) -> Vec<u8> {
    let mut b = vec![0; 176];
    for (i, n) in [if fragment { 7004 } else { 7003 }, 6, 176, 1, 32, 80, 32, 144]
        .into_iter()
        .enumerate()
    {
        word(&mut b, i * 4, n);
    }
    for (i, n) in [1046, 2051, 4102, 3, 104, 120, 128, 111, 4097, u32::MAX, 1, 0]
        .into_iter()
        .enumerate()
    {
        word(&mut b, 32 + i * 4, n);
    }
    word(&mut b, 80, 2);
    if fragment {
        word(&mut b, 84, 0x12345678);
        word(&mut b, 88, 0x80000001);
        b[92..104].copy_from_slice(&[0, 3, 0, 2, 0, 1, 7, 1, 1, 1, 0xaa, 0xbb]);
    } else {
        for (i, n) in [9, 7, 0x12345678, 0x87654321, 15].into_iter().enumerate() {
            word(&mut b, 84 + i * 4, n);
        }
    }
    b[104..118].copy_from_slice(b"albedo\0COLOR0\0");
    word(&mut b, 120, (-0.0f32).to_bits());
    word(&mut b, 124, 0x7fc01234); // keep NaN payload, not a numeric conversion
    word(&mut b, 128, 2);
    word(&mut b, 132, 0);
    word(&mut b, 136, 16);
    for (i, byte) in b[144..].iter_mut().enumerate() {
        *byte = i as u8;
    }
    b
}

#[test]
fn fragment_fields_strings_defaults_and_patch_offsets_are_distinct() {
    let b = fixture(true);
    let p = parse(&b).unwrap();
    let Stage::Fragment(s) = p.stage else {
        panic!("fragment")
    };
    assert_eq!(
        (s.instruction_count, s.attribute_input_mask, s.partial_tex_type),
        (2, 0x12345678, 0x80000001)
    );
    assert_eq!(
        (s.tex_coords_input_mask, s.tex_coords_2d, s.tex_coords_centroid),
        (3, 2, 1)
    );
    assert_eq!(
        (
            s.register_count,
            s.output_from_h0,
            s.depth_replace,
            s.pixel_kill,
            s.padding
        ),
        (7, true, true, true, [0xaa, 0xbb])
    );
    let a = &p.parameters[0];
    assert_eq!(
        (
            a.ty,
            a.resource,
            a.variability,
            a.resource_index,
            a.direction,
            a.parameter_number
        ),
        (1046, 2051, 4102, 3, 4097, -1)
    );
    assert_eq!(
        (a.name, a.semantic),
        (Some(b"albedo".as_slice()), Some(b"COLOR0".as_slice()))
    );
    assert_eq!(a.default_value, Some(vec![0x80000000, 0x7fc01234]));
    assert_eq!(a.embedded_constants, [0, 16]);
    assert_eq!((a.referenced, a.shared, a.texture_unit()), (true, false, Some(3)));
    assert_eq!(p.microcode, (0..32).collect::<Vec<u8>>());
}

#[test]
fn vertex_fields_and_null_metadata_keep_their_meaning() {
    let mut b = fixture(false);
    for at in [48, 52, 56, 60] {
        word(&mut b, at, 0);
    }
    word(&mut b, 36, 2178); // constant register, not texture unit
    word(&mut b, 44, u32::MAX);
    let p = parse(&b).unwrap();
    let Stage::Vertex(s) = p.stage else {
        panic!("vertex")
    };
    assert_eq!(
        (s.instruction_count, s.instruction_slot, s.register_count),
        (2, 9, 7)
    );
    assert_eq!(
        (s.attribute_input_mask, s.attribute_output_mask, s.user_clip_mask),
        (0x12345678, 0x87654321, 15)
    );
    let a = &p.parameters[0];
    assert_eq!(
        (a.name, a.semantic, a.default_value.as_ref(), a.texture_unit()),
        (None, None, None, None)
    );
    assert!(a.embedded_constants.is_empty());
    assert_eq!(a.resource_index, -1);
}

#[test]
fn caller_owns_concatenation_and_padding() {
    let mut b = fixture(true);
    b.extend([0u8; 16]);
    b.extend(fixture(false));
    assert!(parse(&b).unwrap_err().detail.contains("trailing"));
    assert_eq!(parse_prefix(&b).unwrap().bytes.len(), 176);
    assert_eq!(parse(&b[192..]).unwrap().profile, 7003);
}

#[test]
fn invalid_headers_ranges_booleans_strings_and_constants_refuse() {
    for (at, value, why) in [
        (0, 1, "profile"),
        (4, 7, "revision"),
        (8, 31, "outside"),
        (8, 177, "outside"),
        (12, u32::MAX, "outside"),
        (16, 0, "overlapping"),
        (20, 32, "overlapping"),
        (20, 81, "unaligned"),
        (28, 80, "overlapping"),
        (28, 140, "16-byte"),
        (80, 3, "instruction"),
        (72, 2, "boolean"),
        (48, 32, "metadata"),
        (48, 176, "unterminated"),
        (52, 32, "metadata"),
        (52, 121, "unaligned"),
        (56, 32, "outside"),
        (128, u32::MAX, "outside"),
        (132, 1, "embedded"),
        (136, 32, "embedded"),
        (32, 9999, "unsupported"),
    ] {
        let mut b = fixture(true);
        word(&mut b, at, value);
        let e = parse(&b).expect_err("malformed fixture");
        assert!(
            e.detail.contains(why),
            "at={at} value={value}: {e}, expected {why}"
        );
    }
    let b = fixture(true);
    for len in 0..b.len() {
        assert!(parse(&b[..len]).is_err(), "truncation {len}");
    }
    let mut b = b;
    b[101] = 2;
    assert_eq!(parse(&b).unwrap_err().offset, 101);
}

#[test]
fn shared_strings_and_unknown_enum_codes_are_preserved() {
    let mut b = fixture(true);
    word(&mut b, 48, 112); // suffix of the semantic; valid shared string storage
    word(&mut b, 40, 9999);
    word(&mut b, 64, 8888);
    let p = parse(&b).unwrap();
    assert_eq!(p.parameters[0].name, Some(b"OLOR0".as_slice()));
    assert_eq!(
        (p.parameters[0].variability, p.parameters[0].direction),
        (9999, 8888)
    );
}

#[test]
fn matrix_defaults_and_metadata_at_the_exact_end_are_bounded() {
    let mut b = fixture(false);
    b.resize(240, 0);
    word(&mut b, 8, 240);
    word(&mut b, 28, 208);
    word(&mut b, 32, 1064); // CG_FLOAT4x4, sixteen IEEE float words
    word(&mut b, 48, 0);
    word(&mut b, 52, 112);
    word(&mut b, 56, 0);
    word(&mut b, 60, 207); // empty NUL-terminated string immediately before ucode
    for i in 0..16 {
        word(&mut b, 112 + i * 4, i as u32);
    }
    let p = parse(&b).unwrap();
    assert_eq!(p.parameters[0].default_value, Some((0..16).collect()));
    assert_eq!(p.parameters[0].semantic, Some(b"".as_slice()));
    word(&mut b, 52, 176); // matrix would cross into microcode
    assert!(parse(&b).unwrap_err().detail.contains("metadata"));
}
