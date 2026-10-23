//! A GrassPatternLibrary body built by hand in the layout of Riders Republic's (docs/grass.md §2.2): one model, a group
//! with one rule, an empty group and the 443-byte tail.
use primase_anvil::read::grass::StoredGrassLibrary;

fn object(id: u64, kind: u32) -> Vec<u8> {
    let mut b = id.to_le_bytes().to_vec();
    b.extend(kind.to_le_bytes());
    b
}

fn library() -> Vec<u8> {
    let mut b = vec![1];
    b.extend(1u32.to_le_bytes());
    b.push(0);
    // a model: the settings object (six f32, the bytes and the enum, three imposter levels, the lookat ratio, 23
    // bytes with the LOD table and the four distances) and the bounding volume object
    b.extend(object(1, 0xDDB8_20C0));
    for v in [0.1f32, 0.9, 1.0, 0.0, 0.5, 10.0] {
        b.extend(v.to_le_bytes());
    }
    b.extend([4, 1]);
    b.extend(1u32.to_le_bytes());
    b.extend([2, 3, 4]);
    b.extend(1.0f32.to_le_bytes());
    b.extend([0, 1, 2, 3, 3]);
    for v in [10.0f32, 20.0, 40.0, 0.0] {
        b.extend(v.to_le_bytes());
    }
    b.extend([0, 0]);
    b.extend(object(2, 0x4AEC_3476));
    for v in [-1.0f32, -1.0, 0.0, 1.0, 1.0, 2.0] {
        b.extend(v.to_le_bytes());
    }
    b.extend([0u8; 24]);
    b.extend(0u32.to_le_bytes());
    // a group with one rule
    b.extend(object(3, 0xBDD9_6F63));
    b.extend(1u32.to_le_bytes());
    b.push(0);
    b.extend(object(4, 0xA6FC_073D));
    b.extend(5u16.to_le_bytes());
    b.push(1);
    b.extend(0.3f32.to_le_bytes());
    b.extend(0.0f32.to_le_bytes());
    b.push(1);
    b.extend(1.2f32.to_le_bytes());
    b.extend(0.8f32.to_le_bytes());
    b.push(1);
    b.extend(1.2f32.to_le_bytes());
    b.extend(0.8f32.to_le_bytes());
    b.extend(5.0f32.to_le_bytes());
    b.extend([0u8; 49]);
    // an empty group
    b.extend(object(5, 0xBDD9_6F63));
    b.extend(0u32.to_le_bytes());
    b.push(0);
    // the tail: the table (none but entry 255) and 187 bytes
    b.extend([0xFF; 255]);
    b.push(184);
    b.extend([0u8; 187]);
    b
}

#[test]
fn a_library_reads_to_its_last_byte() {
    let body = library();
    let l = StoredGrassLibrary::parse(&body).expect("library");
    assert_eq!(l.models.len(), 1);
    assert_eq!(l.models[0].id, 1);
    assert_eq!(l.models[0].settings.softness, 0.5);
    assert_eq!(l.models[0].settings.visible_virtual_lod_count, 4);
    assert_eq!(l.models[0].settings.raycast_translucency, 1);
    assert_eq!(
        l.models[0].settings.imposter_shader_start_model_lod_index,
        [2, 3, 4]
    );
    assert_eq!(l.models[0].settings.switch_distances, [10.0, 20.0, 40.0, 0.0]);
    assert_eq!(l.models[0].bounds, [-1.0, -1.0, 0.0, 1.0, 1.0, 2.0]);
    assert_eq!(l.groups.len(), 2);
    assert_eq!(l.groups[0].rules.len(), 1);
    let r = &l.groups[0].rules[0];
    assert_eq!((r.model, r.tiling_log_count), (5, 1));
    assert_eq!((r.density, r.scale_a, r.scale_b), (0.3, 1.2, 0.8));
    assert!(r.scaling_enabled && r.height_scaling_enabled);
    assert!(l.groups[1].rules.is_empty());
    assert_eq!(l.table[254], 0xFF);
    assert_eq!(l.table[255], 184);
    assert_eq!(l.rest.len(), 187);
}

#[test]
fn a_short_body_is_refused() {
    let body = library();
    assert!(StoredGrassLibrary::parse(&body[..body.len() - 300]).is_err());
}
