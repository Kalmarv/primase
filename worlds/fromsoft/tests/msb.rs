use primase_fromsoft::msb::{MapOffset, Part, PartKind};
fn part() -> Part {
    Part {
        index: 0,
        name: "synthetic".into(),
        kind: PartKind::MapPiece,
        model: None,
        hit_filter: None,
        position: [0.; 3],
        rotation_degrees: [0.; 3],
        scale: [1.; 3],
    }
}
fn close(a: [[f32; 3]; 4], b: [[f32; 3]; 4]) {
    for (a, b) in a.iter().flatten().zip(b.iter().flatten()) {
        assert!((a - b).abs() < 1e-6, "{a} != {b}");
    }
}
#[test]
fn euler_order_is_x_then_z_then_y() {
    let mut p = part();
    p.rotation_degrees = [90., 90., 90.];
    p.scale = [2., 3., 4.];
    p.position = [10., 20., 30.];
    // X->Z->Y rotates +X to +Y, +Y to +X and +Z to -Z.
    // Conjugating the Z reflection keeps this particular basis unchanged.
    close(
        p.transform().unwrap().cols,
        [[0., 2., 0.], [3., 0., 0.], [0., 0., -4.], [10., 20., -30.]],
    );
}
#[test]
fn offset_is_separate_from_part_world_position() {
    let p = Part {
        position: [1., 2., 3.],
        ..part()
    };
    let o = MapOffset {
        name: "offset".into(),
        position: [100., 0., 200.],
        degrees: 90.,
    };
    assert_eq!(p.transform().unwrap().cols[3], [1., 2., -3.]);
    close(
        o.transform().unwrap().cols,
        [[0., 0., 1.], [0., 1., 0.], [-1., 0., 0.], [100., 0., -200.]],
    );
}
#[test]
fn invalid_transform_values_are_errors() {
    let p = Part {
        rotation_degrees: [f32::NAN, 0., 0.],
        ..part()
    };
    assert!(p.transform().is_err());
}

fn put64(data: &mut [u8], offset: usize, value: usize) {
    data[offset..offset + 8].copy_from_slice(&(value as u64).to_le_bytes());
}
fn empty_map() -> Vec<u8> {
    let mut data = b"MSB \x01\0\0\0\x10\0\0\0\0\0\x01\xff".to_vec();
    let names = [
        "MODEL_PARAM_ST",
        "EVENT_PARAM_ST",
        "POINT_PARAM_ST",
        "ROUTE_PARAM_ST",
        "LAYER_PARAM_ST",
        "PARTS_PARAM_ST",
        "MAPSTUDIO_PARTS_POSE_ST",
        "MAPSTUDIO_BONE_NAME_STRING",
    ];
    for (i, name) in names.into_iter().enumerate() {
        let start = data.len();
        data.resize(start + 24, 0);
        data[start..start + 4].copy_from_slice(&35u32.to_le_bytes());
        data[start + 4..start + 8].copy_from_slice(&1u32.to_le_bytes());
        put64(&mut data, start + 8, start + 24);
        data.extend(name.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
        data.resize(data.len().next_multiple_of(8), 0);
        let next = if i == 7 { 0 } else { data.len() };
        put64(&mut data, start + 16, next);
    }
    data
}
#[test]
fn empty_sections_and_truncated_envelopes() {
    let data = empty_map();
    let map = primase_fromsoft::msb::MapLayout::read(&data).unwrap();
    assert_eq!(map.sections.len(), 8);
    assert!(map.models.is_empty() && map.parts.is_empty());
    for end in 0..data.len() - 8 {
        assert!(
            primase_fromsoft::msb::MapLayout::read(&data[..end]).is_err(),
            "{end}"
        );
    }
}
#[test]
fn rejects_cycles_and_names_pointing_into_headers() {
    let mut data = empty_map();
    put64(&mut data, 32, 16);
    assert!(primase_fromsoft::msb::MapLayout::read(&data).is_err());
    let mut data = empty_map();
    put64(&mut data, 24, 16);
    assert!(primase_fromsoft::msb::MapLayout::read(&data).is_err());
}
