use primase_fromsoft::flver::{FaceSet, Model};
fn put(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn fixture() -> Vec<u8> {
    let mut b = vec![0; 526];
    b[..8].copy_from_slice(b"FLVER\0L\0");
    for (o, v) in [
        (8, 0x2001a),
        (12, 448),
        (16, 78),
        (24, 1),
        (32, 1),
        (36, 1),
        (80, 1),
        (84, 1),
    ] {
        put(&mut b, o, v);
    }
    b[73] = 1;
    put(&mut b, 128, 408);
    put(&mut b, 132, 410);
    put(&mut b, 192, 1);
    put(&mut b, 196, 400);
    put(&mut b, 200, 1);
    put(&mut b, 204, 404);
    b[213] = 1;
    put(&mut b, 216, 3);
    put(&mut b, 224, 6);
    put(&mut b, 232, 16);
    put(&mut b, 248, 24);
    put(&mut b, 252, 3);
    put(&mut b, 264, 72);
    put(&mut b, 268, 6);
    put(&mut b, 272, 4);
    put(&mut b, 284, 320);
    for (i, (offset, kind, semantic)) in [(0, 2, 0), (12, 0x11, 3), (16, 0x13, 5), (20, 0x13, 10)]
        .into_iter()
        .enumerate()
    {
        let o = 320 + i * 20;
        put(&mut b, o + 4, offset);
        put(&mut b, o + 8, kind);
        put(&mut b, o + 12, semantic);
    }
    b[448..454].copy_from_slice(&[0, 0, 1, 0, 2, 0]);
    for (i, p) in [[0f32, 0., 0.], [1., 0., 0.], [0., 1., 0.]]
        .into_iter()
        .enumerate()
    {
        let o = 454 + i * 24;
        for (j, x) in p.into_iter().enumerate() {
            put(&mut b, o + j * 4, x.to_bits());
        }
        b[o + 12..o + 16].copy_from_slice(&[127, 127, 254, 0]);
        b[o + 16..o + 20].copy_from_slice(&[0, 8, 0, 248]);
        b[o + 20..o + 24].copy_from_slice(&[255, 128, 0, 255]);
    }
    b
}
#[test]
fn gx_records_retain_uninterpreted_payload_and_exclude_terminator() {
    let mut bytes = fixture();
    bytes.splice(448..448, [0; 48]);
    put(&mut bytes, 12, 496);
    put(&mut bytes, 148, 416);
    bytes[416..420].copy_from_slice(b"GXMD");
    put(&mut bytes, 420, 123);
    put(&mut bytes, 424, 28);
    for (i, value) in [1, 237, 1, 0.5f32.to_bits()].into_iter().enumerate() {
        put(&mut bytes, 428 + i * 4, value);
    }
    put(&mut bytes, 444, 0x7fff_ffff);
    put(&mut bytes, 448, 100);
    put(&mut bytes, 452, 12);
    let model = Model::read(&bytes).unwrap();
    let gx = &model.materials[0].gx;
    assert_eq!(gx.len(), 1);
    assert_eq!(gx[0].id, *b"GXMD");
    assert_eq!(gx[0].version, 123);
    assert_eq!(gx[0].data, bytes[428..444]);
    assert_eq!(model.meshes[0].vertices[1].position, [1., 0., 0.]);
    put(&mut bytes, 424, 1000);
    assert!(Model::read(&bytes).is_err());
}

#[test]
fn packed_attributes_keep_layer_bytes() {
    let m = Model::read(&fixture()).unwrap();
    let v = &m.meshes[0].vertices[0];
    assert_eq!(v.normal, [0., 0., 1.]);
    assert_eq!(v.uvs, [[1., -1., 0.]]);
    assert_eq!(v.layer_bytes, [(0, [0, 8, 0, 248])]);
    assert_eq!(v.colors, [[1., 128. / 255., 0., 1.]]);
}

#[test]
fn earlier_versions_share_the_geometry_tables() {
    for version in [0x20013, 0x20014, 0x20016, 0x2001a] {
        let mut bytes = fixture();
        put(&mut bytes, 8, version);
        let model = Model::read(&bytes).unwrap();
        assert_eq!(model.version, version);
        assert_eq!(model.meshes[0].vertices[1].position, [1., 0., 0.]);
    }
}
/// The game's G-buffer vertex shaders read NORMAL/TANGENT as uint4 and decode
/// `(v - 127) * f32::from_bits(0x3c010204)`. Checked families: Multiple,
/// MultipleGround, MetallicValue and Character_AMSN. Integer input types come
/// from shader signatures, not a captured input-layout object. These synthetic
/// expected bits were generated independently with exact f64 products of the
/// integer and f32 constant, rounded once to f32; no shader bytes are included.
#[test]
fn packed_normals_follow_consumer_reciprocal_multiply() {
    let expected = include_bytes!("fixtures/flver-normal.bin");
    for value in 0..=255u8 {
        let mut bytes = fixture();
        bytes[466..469].fill(value);
        // Replace the colour member with a uint4 tangent to exercise all lanes.
        put(&mut bytes, 388, 0x11);
        put(&mut bytes, 392, 6);
        bytes[474..478].fill(value);
        let model = Model::read(&bytes).unwrap();
        let bits = u32::from_le_bytes(
            expected[value as usize * 4..value as usize * 4 + 4]
                .try_into()
                .unwrap(),
        );
        let vertex = &model.meshes[0].vertices[0];
        assert_eq!(vertex.normal.map(f32::to_bits), [bits; 3], "byte {value}");
        assert_eq!(vertex.tangents[0].map(f32::to_bits), [bits; 4], "byte {value}");
    }
}
#[test]
fn restarts_and_degenerate_joints_preserve_reference_order() {
    let f = FaceSet {
        flags: 0,
        strip: true,
        cull: true,
        indices: vec![0, 1, 2, 3, 3, 4, 5, 0xffff, 6, 7, 8, 9],
    };
    assert_eq!(f.triangulate(), [0, 1, 2, 3, 2, 1, 3, 4, 5, 6, 7, 8, 9, 8, 7]);
}
#[test]
fn truncated_and_invalid_references_are_rejected() {
    let bytes = fixture();
    for n in 0..bytes.len() {
        assert!(Model::read(&bytes[..n]).is_err(), "prefix {n}");
    }
    for (o, v) in [
        (196, u32::MAX),
        (204, 525),
        (248, 23),
        (244, 1),
        (192, 2),
        (400, 1),
    ] {
        let mut bad = bytes.clone();
        put(&mut bad, o, v);
        assert!(Model::read(&bad).is_err(), "field {o}");
    }
}
