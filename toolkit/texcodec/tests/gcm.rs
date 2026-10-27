//! Oracle: hand-worked libgcm descriptors, Morton grids, pitch rows and BC blocks.
use primase_core::{ColorSpace, TexFormat, TexRole, Texture};
use primase_texcodec::gcm::{self, Error, apply_remap};
use rsx_reader::gcm::CellGcmTexture;

fn descriptor(format: u8, width: u16, height: u16, mips: u8) -> CellGcmTexture {
    CellGcmTexture {
        format,
        mipmap: mips,
        dimension: 2,
        cubemap: 0,
        remap: 0xaae4,
        width,
        height,
        depth: 1,
        location: 0,
        pad: 0,
        pitch: 0,
        offset: 0,
    }
}
fn decode(d: CellGcmTexture, bytes: &[u8]) -> Vec<Texture> {
    gcm::decode(d, bytes, 0, ColorSpace::Linear, TexRole::Color).expect("fixture")
}

#[test]
fn hand_worked_morton_argb_4_and_8() {
    // Stored texels contain row-major indices, ordered by nested 2x2 quadrants.
    let small = [0, 1, 4, 5, 2, 3, 6, 7, 8, 9, 12, 13, 10, 11, 14, 15];
    let large = [
        0, 1, 8, 9, 2, 3, 10, 11, 16, 17, 24, 25, 18, 19, 26, 27, 4, 5, 12, 13, 6, 7, 14, 15, 20, 21, 28, 29,
        22, 23, 30, 31, 32, 33, 40, 41, 34, 35, 42, 43, 48, 49, 56, 57, 50, 51, 58, 59, 36, 37, 44, 45, 38,
        39, 46, 47, 52, 53, 60, 61, 54, 55, 62, 63,
    ];
    for (side, order) in [(4, small.as_slice()), (8, large.as_slice())] {
        let bytes: Vec<u8> = order.iter().flat_map(|&i| [200, i, 100, 50]).collect();
        let image = decode(descriptor(0x85, side, side, 1), &bytes);
        let expected: Vec<u8> = (0..side * side).flat_map(|i| [i as u8, 100, 50, 200]).collect();
        assert_eq!(image[0].levels[0], expected);
    }
}

#[test]
fn rectangular_morton_and_b8_remap() {
    let bytes = [0, 1, 4, 5, 2, 3, 6, 7]; // 4x2: x high bit follows interleaved pair.
    let mut d = descriptor(0x81, 4, 2, 1);
    d.remap = 0xa9ff;
    let image = decode(d, &bytes);
    assert_eq!(
        image[0].levels[0],
        (0..8).flat_map(|i| [i, i, i, 255]).collect::<Vec<_>>()
    );
    d.remap = 0xaae4;
    assert_eq!(decode(d, &bytes), image);
}

#[test]
fn linear_padding_and_constant_pitch_mips() {
    let mut d = descriptor(0xa5, 2, 2, 2);
    d.pitch = 12;
    d.offset = 0xf0000000; // supplied data already addresses the payload.
    let bytes = [
        9, 1, 2, 3, 10, 4, 5, 6, 99, 99, 99, 99, 11, 7, 8, 9, 12, 10, 11, 12, 99, 99, 99, 99, 13, 14, 15, 16,
        99, 99, 99, 99, 99, 99, 99, 99,
    ];
    let image = decode(d, &bytes);
    assert_eq!(
        image[0].levels[0],
        [1, 2, 3, 9, 4, 5, 6, 10, 7, 8, 9, 11, 10, 11, 12, 12]
    );
    assert_eq!(image[0].levels[1], [14, 15, 16, 13]);
    let tail = gcm::decode(d, &bytes[24..], 1, ColorSpace::Linear, TexRole::Color).expect("tail");
    assert_eq!((tail[0].width, tail[0].height), (1, 1));
    assert_eq!(tail[0].levels[0], image[0].levels[1]);
}

#[test]
fn dxt_mips_are_block_linear_and_preserved() {
    let red = [0, 0xf8, 0, 0, 0, 0, 0, 0];
    let green = [0xe0, 7, 0, 0, 0, 0, 0, 0];
    let bytes: Vec<_> = red.into_iter().chain(green).collect();
    let d = descriptor(0x86, 4, 4, 2);
    let image = decode(d, &bytes);
    assert_eq!(image[0].format, TexFormat::Bc1);
    assert_eq!(image[0].levels, [red.to_vec(), green.to_vec()]);
    let mut remapped = d;
    remapped.remap = 0;
    let image = decode(remapped, &bytes);
    assert_eq!(image[0].format, TexFormat::Rgba8Unorm);
    assert_eq!(image[0].levels, [vec![0; 64], vec![0; 16]]);
    let mut linear = descriptor(0xa6, 4, 4, 2);
    linear.pitch = 16;
    let padded: Vec<u8> = red
        .into_iter()
        .chain([99; 8])
        .chain(green)
        .chain([99; 8])
        .collect();
    assert_eq!(decode(linear, &padded)[0].levels, [red.to_vec(), green.to_vec()]);
}

#[test]
fn bc3_passthrough_and_expansion() {
    let mut d = descriptor(0x88, 4, 4, 1);
    let bytes = [255, 0, 0, 0, 0, 0, 0, 0, 0, 0xf8, 0, 0, 0, 0, 0, 0];
    assert_eq!(decode(d, &bytes)[0].format, TexFormat::Bc3);
    d.remap = 0xaa6c; // Swap red/blue, retain alpha/green.
    assert_eq!(decode(d, &bytes)[0].levels[0], [0, 0, 255, 255].repeat(16));
}

#[test]
fn cube_faces_align_between_chains_only() {
    let mut d = descriptor(0x86, 4, 4, 2);
    d.cubemap = 1;
    let layout = gcm::layout(d, 0).expect("layout");
    assert_eq!(
        layout.iter().map(|s| s.offset).collect::<Vec<_>>(),
        [0, 8, 128, 136, 256, 264, 384, 392, 512, 520, 640, 648]
    );
    let mut bytes = vec![99; 656];
    for s in &layout {
        bytes[s.offset..s.offset + s.size].fill(s.face * 10 + s.mip);
    }
    let images = decode(d, &bytes);
    assert_eq!(images.len(), 6);
    for (i, image) in images.iter().enumerate() {
        assert_eq!(image.levels, [vec![i as u8 * 10; 8], vec![i as u8 * 10 + 1; 8]]);
    }
    d.width = 256;
    d.height = 256;
    d.mipmap = 9;
    let layout = gcm::layout(d, 0).expect("layout");
    assert_eq!(layout[9].offset, 43776);
    assert_eq!(layout.last().map(|s| s.offset + s.size), Some(262584));
    let tail = gcm::layout(d, 1).expect("tail");
    assert_eq!(tail[8].offset, 11008);
    assert_eq!(tail.last().map(|s| s.offset + s.size), Some(65976));
}

#[test]
fn linear_cube_faces_are_contiguous_and_un_does_not_change_pixels() {
    let mut d = descriptor(0xe5, 1, 1, 1);
    d.cubemap = 1;
    d.pitch = 8;
    let layout = gcm::layout(d, 0).expect("linear cube");
    assert_eq!(
        layout.iter().map(|s| s.offset).collect::<Vec<_>>(),
        [0, 8, 16, 24, 32, 40]
    );
    let bytes = [200, 10, 20, 30, 99, 99, 99, 99].repeat(6);
    let with_un = decode(d, &bytes);
    d.format = 0xa5;
    assert_eq!(with_un, decode(d, &bytes));
    assert!(with_un.iter().all(|face| face.levels[0] == [10, 20, 30, 200]));
}

#[test]
fn swizzled_argb_mips_are_contiguous() {
    let d = descriptor(0x85, 2, 2, 2);
    let bytes = [
        1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
    ];
    let image = decode(d, &bytes);
    assert_eq!(
        image[0].levels[0],
        [2, 3, 4, 1, 6, 7, 8, 5, 10, 11, 12, 9, 14, 15, 16, 13]
    );
    assert_eq!(image[0].levels[1], [18, 19, 20, 17]);
}

#[test]
fn channel_selectors_and_constants() {
    assert_eq!(
        apply_remap(0xaae4, [10, 20, 30, 40]).expect("identity"),
        [10, 20, 30, 40]
    );
    assert_eq!(
        apply_remap(0xaa6c, [10, 20, 30, 40]).expect("swap"),
        [30, 20, 10, 40]
    );
    assert_eq!(apply_remap(0x55e4, [10, 20, 30, 40]).expect("one"), [255; 4]);
    assert_eq!(apply_remap(0, [10, 20, 30, 40]).expect("zero"), [0; 4]);
    assert_eq!(apply_remap(0xaa00, [10, 20, 30, 40]).expect("alpha"), [40; 4]);
    assert!(matches!(apply_remap(0xffe4, [0; 4]), Err(Error::Remap(_))));
}

#[test]
fn explicit_refusals() {
    let mut d = descriptor(0x87, 4, 4, 1);
    assert!(matches!(gcm::layout(d, 0), Err(Error::Format(0x87))));
    d.format = 0x85;
    d.dimension = 3;
    d.depth = 4;
    assert!(matches!(gcm::layout(d, 0), Err(Error::Dimension { .. })));
    d.dimension = 2;
    d.depth = 1;
    d.width = 3;
    assert!(matches!(gcm::layout(d, 0), Err(Error::Invalid(_))));
    d.width = 4;
    d.mipmap = 4;
    assert!(matches!(gcm::layout(d, 0), Err(Error::Invalid(_))));
    d.mipmap = 1;
    d.format = 0xa5;
    d.pitch = 15;
    assert!(matches!(gcm::layout(d, 0), Err(Error::Invalid(_))));
    d.pitch = 16;
    assert!(matches!(
        gcm::decode(d, &[0; 63], 0, ColorSpace::Linear, TexRole::Color),
        Err(Error::Length {
            expected: 64,
            actual: 63
        })
    ));
    assert!(matches!(
        gcm::decode(d, &[0; 65], 0, ColorSpace::Linear, TexRole::Color),
        Err(Error::Length {
            expected: 64,
            actual: 65
        })
    ));
    d.width = 0;
    assert!(matches!(gcm::layout(d, 0), Err(Error::Invalid(_))));
}
