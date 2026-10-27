use fromsoft_formats::msb::{MapOffset, Part, PartKind};
fn part() -> Part {
    Part {
        index: 0,
        name: "synthetic".into(),
        kind: PartKind::MapPiece,
        model: None,
        hit_filter: None,
        npc_param: None,
        position: [0.; 3],
        rotation_degrees: [0.; 3],
        scale: [1.; 3],
        entity_id: -1,
        entity_group_ids: [-1; 8],
        shadows: Default::default(),
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
    // X->Z->Y rotates +X to +Y, +Y to +X and +Z to -Z, in the map's own frame; the translation is the stored one.
    close(
        p.matrix().unwrap(),
        [[0., 2., 0.], [3., 0., 0.], [0., 0., -4.], [10., 20., 30.]],
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
    assert_eq!(p.matrix().unwrap()[3], [1., 2., 3.]);
    // a quarter turn about y: +X to -Z, +Z to +X
    close(
        o.matrix().unwrap(),
        [[0., 0., -1.], [0., 1., 0.], [1., 0., 0.], [100., 0., 200.]],
    );
}
#[test]
fn invalid_transform_values_are_errors() {
    let p = Part {
        rotation_degrees: [f32::NAN, 0., 0.],
        ..part()
    };
    assert!(p.matrix().is_err());
}

fn put64(data: &mut [u8], offset: usize, value: usize) {
    data[offset..offset + 8].copy_from_slice(&(value as u64).to_le_bytes());
}
fn empty_map() -> Vec<u8> {
    map_with_parts(&[])
}
/// A map whose parts section holds these part records (every other section empty).
fn map_with_parts(parts: &[Vec<u8>]) -> Vec<u8> {
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
        let records: &[Vec<u8>] = if i == 5 { parts } else { &[] };
        let start = data.len();
        let table = 16 + 8 * (records.len() + 1);
        data.resize(start + table, 0);
        data[start..start + 4].copy_from_slice(&35u32.to_le_bytes());
        data[start + 4..start + 8].copy_from_slice(&((records.len() + 1) as u32).to_le_bytes());
        put64(&mut data, start + 8, start + table);
        data.extend(name.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
        data.resize(data.len().next_multiple_of(8), 0);
        for (k, r) in records.iter().enumerate() {
            let at = data.len();
            put64(&mut data, start + 16 + 8 * k, at);
            data.extend_from_slice(r);
        }
        let next = if i == 7 { 0 } else { data.len() };
        put64(&mut data, start + table - 8, next);
    }
    data
}
/// One map-piece part record (no model): header, name, sib path, the entity block, one byte of type data.
fn part_record(entity_id: i32, groups: [i32; 8], tail: [u8; 16]) -> Vec<u8> {
    let mut r = vec![0u8; 160];
    let text = |r: &mut Vec<u8>, field: usize, s: &str| {
        let at = r.len();
        put64(r, field, at);
        r.extend(s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
        r.resize(r.len().next_multiple_of(8), 0);
    };
    text(&mut r, 0, "m000000_0000");
    text(&mut r, 24, "sib");
    r[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    r[56..68].copy_from_slice(&[1f32.to_le_bytes(); 3].concat());
    r[68..76].fill(255);
    let entity = r.len();
    put64(&mut r, 96, entity);
    r.resize(entity + 0x54, 0);
    r[entity..entity + 4].copy_from_slice(&entity_id.to_le_bytes());
    for (k, g) in groups.iter().enumerate() {
        r[entity + 0x1c + 4 * k..entity + 0x20 + 4 * k].copy_from_slice(&g.to_le_bytes());
    }
    r[entity + 0x44..entity + 0x54].copy_from_slice(&tail);
    let type_data = r.len();
    put64(&mut r, 104, type_data);
    r.resize(r.len() + 8, 0);
    r
}
#[test]
fn shadow_flags_retain_false_renderer_bytes_and_contradictions() {
    use fromsoft_formats::msb::{MapLayout, ShadowFlags};
    let mut record = part_record(5, [-1; 8], [0; 16]);
    let entity = u64::from_le_bytes(record[96..104].try_into().unwrap()) as usize;
    for (offset, value) in [(0x0a, 1), (0x0c, 0), (0x0d, 2), (0x0e, 255), (0x11, 1), (0x12, 1)] {
        record[entity + offset] = value;
    }
    let map = MapLayout::read(&map_with_parts(&[record.clone()])).unwrap();
    let expected = ShadowFlags {
        cast: false,
        receive: true,
        only: true,
        point_light_cast: true,
        static_cast: 2,
        cascade3_cast: 255,
    };
    assert_eq!(map.parts[0].shadows, expected);
    // Contradiction is preserved, not silently normalized into an enum.
    assert!(map.parts[0].shadows.only && !map.parts[0].shadows.cast);
    let json = serde_json::to_value(expected).unwrap();
    assert_eq!(json["cast"], false);
    assert_eq!(json["static_cast"], 2);
    assert_eq!(json["cascade3_cast"], 255);
    for offset in [0x0a, 0x0c, 0x11, 0x12] {
        let mut bad = record.clone();
        bad[entity + offset] = 2;
        assert!(MapLayout::read(&map_with_parts(&[bad])).is_err());
    }
}

#[test]
fn entity_ids_and_groups_as_stored() {
    let groups = [1117201, -1, 0, 1116201, -1, -1, -1, -1];
    let map =
        fromsoft_formats::msb::MapLayout::read(&map_with_parts(&[part_record(-1, groups, [0; 16])])).unwrap();
    let p = &map.parts[0];
    assert_eq!(
        (p.entity_id, p.entity_group_ids),
        (-1, groups),
        "sentinels kept as stored"
    );
    assert_eq!(
        p.entity_ids().collect::<Vec<_>>(),
        [1117201, 1116201],
        "only positive IDs address the part"
    );
    let map =
        fromsoft_formats::msb::MapLayout::read(&map_with_parts(&[part_record(1110432, [-1; 8], [0; 16])]))
            .unwrap();
    assert_eq!(map.parts[0].entity_ids().collect::<Vec<_>>(), [1110432]);
}
#[test]
fn part_graphics_config_and_scene_ids_as_stored() {
    use fromsoft_formats::msb::{MapLayout, PartGparam};
    let plain = part_record(1, [-1; 8], [0; 16]);
    let mut with = part_record(2, [-1; 8], [0; 16]);
    // the config at +112: four value IDs and sixteen zero bytes; the scene config at +120: sixty zero bytes, the four
    // signed bytes, and the rest of its eighty
    let config = with.len();
    put64(&mut with, 112, config);
    for v in [7i32, -1, 12, 3] {
        with.extend(v.to_le_bytes());
    }
    with.resize(with.len() + 16, 0);
    let scene = with.len();
    put64(&mut with, 120, scene);
    with.resize(scene + 80, 0);
    with[scene + 60..scene + 64].copy_from_slice(&[2, 0xff, 4, 0]);
    let map = MapLayout::read(&map_with_parts(&[plain, with.clone()])).unwrap();
    assert_eq!(
        map.gparam_configs().unwrap(),
        [PartGparam {
            part: 1,
            light_set: 7,
            fog: -1,
            light_scattering: 12,
            env_map: 3,
            scene: Some([2, -1, 4, 0]),
        }],
        "a part without the slot stores none; the other as stored, -1 kept"
    );
    // a part without a scene config has none; one whose scene block runs past its record is an error
    let mut no_scene = with.clone();
    put64(&mut no_scene, 120, 0);
    let map = MapLayout::read(&map_with_parts(&[no_scene])).unwrap();
    assert_eq!(map.gparam_configs().unwrap()[0].scene, None);
    let mut short = with.clone();
    let near_end = short.len() - 20;
    put64(&mut short, 120, near_end);
    let map = MapLayout::read(&map_with_parts(&[short])).unwrap();
    assert!(map.gparam_configs().is_err());
}
#[test]
fn only_elden_rings_layout_has_a_grass_slot() {
    // Sekiro's header has another record at +128: not a grass config, so none are counted whatever it holds
    let mut record = part_record(1, [-1; 8], [0; 16]);
    let extra = record.len();
    put64(&mut record, 128, extra);
    record.resize(extra + 8, 0);
    let map = fromsoft_formats::msb::MapLayout::read(&map_with_parts(&[record])).unwrap();
    assert_eq!(map.grass_parts().unwrap(), 0);
}
#[test]
fn entity_block_tail_must_be_zero() {
    let mut tail = [0u8; 16];
    tail[15] = 1;
    assert!(
        fromsoft_formats::msb::MapLayout::read(&map_with_parts(&[part_record(-1, [-1; 8], tail)])).is_err()
    );
}
#[test]
fn truncated_entity_block_is_an_error() {
    let mut r = part_record(5, [-1; 8], [0; 16]);
    // point the entity block 8 bytes before the record's end
    let end = r.len() - 8;
    put64(&mut r, 96, end);
    assert!(fromsoft_formats::msb::MapLayout::read(&map_with_parts(&[r])).is_err());
}
#[test]
fn empty_sections_and_truncated_envelopes() {
    let data = empty_map();
    let map = fromsoft_formats::msb::MapLayout::read(&data).unwrap();
    assert_eq!(map.sections.len(), 8);
    assert!(map.models.is_empty() && map.parts.is_empty());
    for end in 0..data.len() - 8 {
        assert!(
            fromsoft_formats::msb::MapLayout::read(&data[..end]).is_err(),
            "{end}"
        );
    }
}
#[test]
fn rejects_cycles_and_names_pointing_into_headers() {
    let mut data = empty_map();
    put64(&mut data, 32, 16);
    assert!(fromsoft_formats::msb::MapLayout::read(&data).is_err());
    let mut data = empty_map();
    put64(&mut data, 24, 16);
    assert!(fromsoft_formats::msb::MapLayout::read(&data).is_err());
}
