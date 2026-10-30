//! A FLVER face set decoded as a mesh (`geometry::decode`) on the synthetic models of fromsoft-formats' FLVER tests
//! (their builders, copied).
use fromsoft_formats::flver::Model;
use primase_fromsoft::geometry::decode;
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
    // the material's string byte count: two empty UTF-16 strings, terminators only
    put(&mut b, 144, 4);
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
fn speedtree_fixture() -> Vec<u8> {
    let f = fixture();
    let mut b = f[..400].to_vec();
    b.extend([0; 40]);
    b.extend(&f[400..]);
    for (o, v) in [(12, 448), (128, 408), (132, 410), (196, 400), (204, 404)] {
        put(&mut b, o, v + 40);
    }
    put(&mut b, 272, 6);
    b[0x6a..0x6c].copy_from_slice(&[0x00, 0x80]);
    for (i, (word, offset, kind)) in [
        (0x8000_0001u32, 0, 3),
        (0x8000_0001, 16, 3),
        (0, 0, 0x2e),
        (0, 8, 0x2e),
        (0, 16, 0x13),
        (0, 20, 0x13),
    ]
    .into_iter()
    .enumerate()
    {
        let o = 320 + i * 20;
        put(&mut b, o, word);
        put(&mut b, o + 4, offset);
        put(&mut b, o + 8, kind);
        put(&mut b, o + 12, 5);
        put(&mut b, o + 16, i as u32);
    }
    for i in 0..3 {
        let o = 494 + i * 24;
        let one = |on: bool| if on { 0x3c00u16 } else { 0 };
        for (j, h) in [one(i == 1), one(i == 2), 0, 0x3800, 0x3800, 0x3400, 0, 0]
            .into_iter()
            .enumerate()
        {
            b[o + j * 2..o + j * 2 + 2].copy_from_slice(&h.to_le_bytes());
        }
        b[o + 16..o + 24].copy_from_slice(&[255, 0, 0, 0, 0, 0, 255, 0]);
    }
    b
}
/// A SpeedTree model's geometry decodes as an ordinary model's: positions mirrored in z, the normal of unit length.
#[test]
fn a_speedtree_mesh_decodes_as_an_ordinary_one() {
    let m = Model::read(&speedtree_fixture()).unwrap();
    let part = decode(&m, 0, 0).unwrap().unwrap();
    assert_eq!(part.mesh.positions[1], [1., 0., 0.]);
    assert_eq!(part.mesh.uv[1], [0.5, 0.25]);
    let s = 1. / 3f32.sqrt();
    for (got, want) in part.mesh.normals[1].iter().zip([s, -s, s]) {
        assert!((got - want).abs() < 1e-6, "{:?}", part.mesh.normals[1]);
    }
}

/// A SpeedTree billboard card has no position of its own: decoding it as geometry is refused.
#[test]
fn a_speedtree_billboard_card_is_refused_as_geometry() {
    // the stored stream as a billboard's: Float4 UV2 at 0 and Float2 UV3 at 16 (the same 24 bytes)
    let mut b = speedtree_fixture();
    put(&mut b, 272, 4);
    put(&mut b, 320 + 2 * 20 + 8, 3);
    put(&mut b, 320 + 3 * 20 + 4, 16);
    put(&mut b, 320 + 3 * 20 + 8, 1);
    let m = Model::read(&b).unwrap();
    assert!(decode(&m, 0, 0).unwrap_err().to_string().contains("billboard"));
}

/// A colour member is the input of its semantic index, whatever its position: the fixture's one colour is `COLOR0` as
/// stored and `COLOR1` with its index set to 1 (as most of Elden Ring's map meshes store their one colour).
#[test]
fn a_colour_is_placed_by_its_semantic_index() {
    use primase_fromsoft::sekiro::source::LayerLayout;
    let first = LayerLayout::of(&Model::read(&fixture()).unwrap(), 0).unwrap();
    assert_eq!(
        (first.first_colour, first.second_colour, first.colours),
        (Some(0), None, 1)
    );
    let mut b = fixture();
    // the colour member (the fourth, at 320 + 3 * 20): its semantic index
    put(&mut b, 320 + 3 * 20 + 16, 1);
    let m = Model::read(&b).unwrap();
    let second = LayerLayout::of(&m, 0).unwrap();
    assert_eq!(
        (second.first_colour, second.second_colour, second.colours),
        (None, Some(0), 1)
    );
    // the decoded colour is the same value either way
    assert_eq!(m.meshes[0].vertices[0].colors[0], [1.0, 128.0 / 255.0, 0.0, 1.0]);
}
