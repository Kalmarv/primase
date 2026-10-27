//! Hand-worked XYZ Morton cells and VTC block groups, based on the published
//! NVIDIA VTC layout and the RSX consumer's dimension ordering, not an encoder.
use primase_core::{ColorSpace, TexFormat, TexRole};
use primase_texcodec::gcm::{self, Error};
use rsx_reader::gcm::CellGcmTexture;

fn volume(format: u8, w: u16, h: u16, d: u16, mips: u8) -> CellGcmTexture {
    CellGcmTexture {
        format,
        mipmap: mips,
        dimension: 3,
        cubemap: 0,
        remap: 0xaae4,
        width: w,
        height: h,
        depth: d,
        location: 0,
        pad: 0,
        pitch: 0,
        offset: 0,
    }
}

#[test]
fn hand_worked_rectangular_xyz_argb_and_mips() {
    // 4x2x2: low bits X,Y,Z, followed by the remaining X bit.
    let order = [0, 1, 4, 5, 8, 9, 12, 13, 2, 3, 6, 7, 10, 11, 14, 15];
    let mut bytes: Vec<u8> = order.into_iter().flat_map(|i| [200, i, 30, 40]).collect();
    bytes.extend([201, 100, 31, 41, 202, 101, 32, 42, 203, 102, 33, 43]);
    let d = volume(0x85, 4, 2, 2, 3);
    let layout = gcm::volume_layout(d, 0).expect("layout");
    assert_eq!(
        layout
            .iter()
            .map(|l| (l.offset, l.size, l.depth))
            .collect::<Vec<_>>(),
        [(0, 64, 2), (64, 8, 1), (72, 4, 1)]
    );
    let v = gcm::decode_volume(d, &bytes, 0, ColorSpace::Linear, TexRole::Color).expect("volume");
    assert_eq!(v.format, TexFormat::Rgba8Unorm);
    assert_eq!(
        v.levels[0].data,
        (0..16).flat_map(|i| [i, 30, 40, 200]).collect::<Vec<_>>()
    );
    assert_eq!(v.levels[1].data, [100, 31, 41, 201, 101, 32, 42, 202]);
    assert_eq!(v.levels[2].data, [102, 33, 43, 203]);
    let tail = gcm::decode_volume(d, &bytes[64..], 1, ColorSpace::Linear, TexRole::Color).expect("tail");
    assert_eq!(tail.levels, v.levels[1..]);
    assert!(matches!(
        gcm::decode(d, &bytes, 0, ColorSpace::Linear, TexRole::Color),
        Err(Error::Dimension { .. })
    ));
}

#[test]
fn axes_exhaust_independently_and_b8_remap_applies() {
    // 2x2x4: low XYZ triplet then Z's remaining bit.
    let order = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
    let mut d = volume(0x81, 2, 2, 4, 1);
    d.remap = 0xa9ff;
    let v = gcm::decode_volume(d, &order, 0, ColorSpace::Linear, TexRole::Color).expect("B8");
    assert_eq!(
        v.levels[0].data,
        (0..16).flat_map(|i| [i, i, i, 255]).collect::<Vec<_>>()
    );
    // 2x4x2: low XYZ triplet then Y's remaining bit.
    let order = [0, 1, 2, 3, 8, 9, 10, 11, 4, 5, 6, 7, 12, 13, 14, 15];
    d.width = 2;
    d.height = 4;
    d.depth = 2;
    let v = gcm::decode_volume(d, &order, 0, ColorSpace::Linear, TexRole::Color).expect("rectangular B8");
    assert_eq!(
        v.levels[0].data,
        (0..16).flat_map(|i| [i, i, i, 255]).collect::<Vec<_>>()
    );
}

#[test]
fn vtc_four_plane_groups_two_plane_tail_and_mip_offsets() {
    // 8x4x8, two BC1 blocks per plane. Each integer labels a whole BC block.
    let labels = [0, 2, 4, 6, 1, 3, 5, 7, 8, 10, 12, 14, 9, 11, 13, 15];
    let bytes: Vec<u8> = labels.into_iter().flat_map(|i| [i; 8]).collect();
    let d = volume(0x86, 8, 4, 8, 1);
    let v = gcm::decode_volume(d, &bytes, 0, ColorSpace::Linear, TexRole::Color).expect("VTC");
    assert_eq!(v.format, TexFormat::Bc1);
    assert_eq!(v.levels[0].data, (0..16).flat_map(|i| [i; 8]).collect::<Vec<_>>());
    let bytes: Vec<u8> = [0, 2, 1, 3].into_iter().flat_map(|i| [i; 8]).collect();
    let two = gcm::decode_volume(
        volume(0x86, 8, 4, 2, 1),
        &bytes,
        0,
        ColorSpace::Linear,
        TexRole::Color,
    )
    .expect("two-plane VTC");
    assert_eq!(
        two.levels[0].data,
        (0..4).flat_map(|i| [i; 8]).collect::<Vec<_>>()
    );
    // 8x4x4 -> 4x2x2 -> 2x1x1 -> 1x1x1:8,2,1,1 BC blocks.
    let mut bytes: Vec<u8> = [0, 2, 4, 6, 1, 3, 5, 7]
        .into_iter()
        .flat_map(|i| [i; 8])
        .collect();
    bytes.extend([8; 8]);
    bytes.extend([9; 8]);
    bytes.extend([10; 8]);
    bytes.extend([11; 8]);
    let d = volume(0x86, 8, 4, 4, 4);
    let v = gcm::decode_volume(d, &bytes, 0, ColorSpace::Linear, TexRole::Color).expect("VTC mips");
    assert_eq!(
        v.levels
            .iter()
            .map(|l| (l.width, l.height, l.depth, l.data.len()))
            .collect::<Vec<_>>(),
        [(8, 4, 4, 64), (4, 2, 2, 16), (2, 1, 1, 8), (1, 1, 1, 8)]
    );
    assert_eq!(v.levels[0].data, (0..8).flat_map(|i| [i; 8]).collect::<Vec<_>>());
    assert_eq!(v.levels[1].data, [[8; 8], [9; 8]].concat());
    assert_eq!(
        gcm::decode_volume(d, &bytes[64..], 1, ColorSpace::Linear, TexRole::Color)
            .expect("tail")
            .levels,
        v.levels[1..]
    );
}

#[test]
fn bc3_remapping_and_linear_volume_pitch() {
    let mut d = volume(0x88, 4, 4, 2, 1);
    d.remap = 0xaa6c;
    let red = [255, 0, 0, 0, 0, 0, 0, 0, 0, 0xf8, 0, 0, 0, 0, 0, 0];
    let bytes = red.repeat(2);
    let v = gcm::decode_volume(d, &bytes, 0, ColorSpace::Linear, TexRole::Color).expect("BC3");
    assert_eq!(v.format, TexFormat::Rgba8Unorm);
    assert_eq!(v.levels[0].data, [0, 0, 255, 255].repeat(32));
    let mut d = volume(0xa5, 2, 2, 3, 2);
    d.pitch = 12;
    let mut bytes = Vec::new();
    for i in 0..6 {
        bytes.extend([200, i, 30, 40, 201, i + 10, 31, 41, 99, 99, 99, 99]);
    }
    bytes.extend([202, 77, 32, 42, 99, 99, 99, 99, 99, 99, 99, 99]);
    let v = gcm::decode_volume(d, &bytes, 0, ColorSpace::Linear, TexRole::Color).expect("pitched");
    let expected: Vec<_> = (0..6)
        .flat_map(|i| [i, 30, 40, 200, i + 10, 31, 41, 201])
        .collect();
    assert_eq!(v.levels[0].data, expected);
    assert_eq!(v.levels[1].data, [77, 32, 42, 202]);
}

#[test]
fn volume_refusals_are_explicit() {
    let mut d = volume(0x85, 2, 2, 2, 1);
    assert!(matches!(
        gcm::decode_volume(d, &[0; 31], 0, ColorSpace::Linear, TexRole::Color),
        Err(Error::Length {
            expected: 32,
            actual: 31
        })
    ));
    assert!(matches!(
        gcm::decode_volume(d, &[0; 33], 0, ColorSpace::Linear, TexRole::Color),
        Err(Error::Length {
            expected: 32,
            actual: 33
        })
    ));
    d.dimension = 2;
    assert!(matches!(gcm::volume_layout(d, 0), Err(Error::Dimension { .. })));
    d.dimension = 3;
    d.depth = 0;
    assert!(matches!(gcm::volume_layout(d, 0), Err(Error::Dimension { .. })));
    d.depth = 3;
    assert!(matches!(gcm::volume_layout(d, 0), Err(Error::Invalid(_))));
    d.depth = 2;
    d.cubemap = 1;
    assert!(matches!(gcm::volume_layout(d, 0), Err(Error::Invalid(_))));
    d.cubemap = 0;
    d.format = 0xa6;
    d.pitch = 8;
    assert!(matches!(gcm::volume_layout(d, 0), Err(Error::Invalid(_))));
    d.format = 0x87;
    assert!(matches!(gcm::volume_layout(d, 0), Err(Error::Format(0x87))));
}
