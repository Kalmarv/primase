//! Synthetic outputs from an independent C# f32 arithmetic harness.
use primase_vertex::*;
#[test]
fn csharp_arithmetic_bits() {
    let bytes = include_bytes!("fixtures/arithmetic.bin");
    let mut cursor = 0;
    for raw in i16::MIN..=i16::MAX {
        assert_eq!(&snorm16(raw).to_le_bytes(), &bytes[cursor..cursor + 4]);
        cursor += 4;
    }
    for _ in 0..4096 {
        let p = u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().unwrap());
        cursor += 4;
        for v in tangent(p).unwrap() {
            assert_eq!(&v.to_le_bytes(), &bytes[cursor..cursor + 4], "packed {p:08x}");
            cursor += 4;
        }
    }
    assert_eq!(cursor, bytes.len());
}
#[test]
fn layout_and_invalid_streams() {
    let elements = vec![
        Element {
            semantic: "POSITION".into(),
            index: 0,
            format: Format::Float3,
            slot: 0,
            offset: Some(16),
        },
        Element {
            semantic: "NORMAL".into(),
            index: 0,
            format: Format::Snorm16x2,
            slot: 0,
            offset: None,
        },
        Element {
            semantic: "TEXCOORD".into(),
            index: 0,
            format: Format::Half2,
            slot: 1,
            offset: None,
        },
    ];
    let layout = resolve_layout(&elements).unwrap();
    assert_eq!(
        layout.iter().map(|e| e.offset).collect::<Vec<_>>(),
        vec![16, 12, 0]
    );
    let bytes = [0, 0x3c, 0, 0xc0];
    assert_eq!(
        decode_stream(
            &layout[2],
            Stream {
                bytes: &bytes,
                stride: 4,
                offset: 0
            },
            0,
            1
        )
        .unwrap(),
        vec![[1., -2., 0., 0.]]
    );
    assert!(
        decode_stream(
            &layout[2],
            Stream {
                bytes: &bytes,
                stride: 4,
                offset: 0
            },
            usize::MAX,
            1
        )
        .is_err()
    );
    assert!(
        decode_stream(
            &layout[0],
            Stream {
                bytes: &bytes,
                stride: 4,
                offset: 0
            },
            0,
            1
        )
        .is_err()
    );
    assert!(Format::Float2.decode(&[0; 7]).is_err());
    assert!(Format::Float2.decode(&[0, 0, 128, 127, 0, 0, 0, 0]).is_err());
}
/// The raw decodes keep a stored NaN or infinity for the reader's own rules; sizes and bounds are checked the same.
#[test]
fn raw_decodes_keep_non_finite_values() {
    let inf_nan = [0, 0, 128, 127, 0, 0, 192, 127];
    let v = Format::Float2.decode_raw(&inf_nan).unwrap();
    assert_eq!(v[0], f32::INFINITY);
    assert!(v[1].is_nan());
    assert!(Format::Float2.decode_raw(&[0; 7]).is_err());
    let half_inf = [0, 0x7c, 0, 0x3c];
    assert!(Format::Half2.decode(&half_inf).is_err());
    assert_eq!(
        Format::Half2.decode_raw(&half_inf).unwrap(),
        [f32::INFINITY, 1., 0., 0.]
    );
    let layout = resolve_layout(&[Element {
        semantic: "TEXCOORD".into(),
        index: 0,
        format: Format::Half2,
        slot: 0,
        offset: None,
    }])
    .unwrap();
    let bytes = [0, 0x3c, 0, 0x3c, 0, 0x7c, 0, 0x3c];
    let stream = Stream {
        bytes: &bytes,
        stride: 4,
        offset: 0,
    };
    assert!(decode_stream(&layout[0], stream, 0, 2).is_err());
    assert_eq!(
        decode_stream_raw(&layout[0], stream, 0, 2).unwrap(),
        vec![[1., 1., 0., 0.], [f32::INFINITY, 1., 0., 0.]]
    );
    assert!(decode_stream_raw(&layout[0], stream, 0, 3).is_err());
    assert!(decode_stream_raw(&layout[0], stream, usize::MAX, 1).is_err());
}
#[test]
fn topology_boundaries_and_joint_parity() {
    assert_eq!(strip_to_list(&[0, 1, 2, 2, 3, 4], true), vec![1, 0, 2, 2, 3, 4]);
    assert_eq!(index_width(&[65535]), IndexWidth::U16);
    assert_eq!(index_width(&[65536]), IndexWidth::U32);
    let mut indices: Vec<_> = (0..65535).collect();
    indices.extend_from_slice(&[0, 1, 2]);
    let chunks = split_triangles(&indices, 65535).unwrap();
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].vertices.len(), 65535);
    assert_eq!(chunks[1].vertices, vec![0, 1, 2]);
    assert!(split_triangles(&[0, 1], 2).is_err());
    assert!(split_triangles(&[0, 1, 2], 2).is_err());
}

#[test]
fn csharp_quantization_exact() {
    let mut words = include_bytes!("fixtures/quantization.bin")
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes(*b));
    let count = words.next().unwrap();
    for case in 0..count {
        let a = words.next().unwrap();
        let b = words.next().unwrap();
        let x = a as i16;
        let y = (a >> 16) as i16;
        let z = b as i16;
        let normal = split_normal(snorm16(x), [y, z]).unwrap();
        let uv = transform_uv(
            [(a as u16) as f32 / 65535., ((a >> 16) as u16) as f32 / 65535.],
            [0.2, 3.1, -0.4, 2.3],
        )
        .unwrap();
        let pos = scale_translate(
            [snorm16(x), snorm16(y), snorm16(z)],
            [2.1, -3.2, 4.3],
            [-0.7, 0.8, -0.9],
        )
        .unwrap();
        let p32 = packed_position32(a, [2.1, -3.2, 4.3], [-0.7, 0.8, -0.9]).unwrap();
        let p64 =
            packed_position64(((b as u64) << 32) | a as u64, [-1.7, -2.8, -3.9], [2.1, 3.2, 4.3]).unwrap();
        for v in normal.into_iter().chain(uv).chain(pos).chain(p32).chain(p64) {
            assert_eq!(v.to_bits(), words.next().unwrap(), "case {case}");
        }
    }
    for bits in 0..=u16::MAX {
        let expected = f32::from_bits(words.next().unwrap());
        let b = bits.to_le_bytes();
        let decoded = Format::Half2.decode(&[b[0], b[1], 0, 0]);
        if expected.is_finite() {
            assert_eq!(decoded.unwrap()[0].to_bits(), expected.to_bits());
        } else {
            assert!(decoded.is_err());
        }
    }
    assert!(words.next().is_none());
}

#[test]
fn configurable_vertex_budget() {
    let indices: Vec<u32> = (0..60000).chain([0, 1, 2]).collect();
    let chunks = split_by_vertex_budget(&indices, 60000, 60000).unwrap();
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].vertices.len(), 60000);
    assert_eq!(chunks[1].vertices, vec![0, 1, 2]);
    assert!(split_by_vertex_budget(&[], 0, 2).is_err());
    assert!(split_by_vertex_budget(&[], 0, 65536).is_err());
}

#[test]
fn stored_normal_csharp_thresholds() {
    let mut words = include_bytes!("fixtures/stored-normals.bin")
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes(*b));
    let count = words.next().unwrap();
    for i in 0..count {
        let n = std::array::from_fn(|_| f32::from_bits(words.next().unwrap()));
        let expected = std::array::from_fn(|_| words.next().unwrap());
        assert_eq!(
            renormalize_stored_normal(n).unwrap().map(f32::to_bits),
            expected,
            "case {i}"
        );
    }
    assert!(words.next().is_none());
}
