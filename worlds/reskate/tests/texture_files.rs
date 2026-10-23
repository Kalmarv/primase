//! A texture file is read through the toolkit's strict readers: a DDS by `primase_dds`, a PNG by its header and then
//! `primase_image`. A header that lies about its size is a named refusal, never a panic or a wrapped size (regression
//! inputs of the robustness census: a DDS of 0 x 0 panicked in the box filter, one of 2^32 x 2^32 overflowed a level size).

#![allow(clippy::unwrap_used)]

use primase_core::{ColorSpace, TexFormat, TexRole, Texture};
use primase_reskate::texture::TexSource;

fn dds(width: u32, height: u32, format: TexFormat, levels: Vec<Vec<u8>>) -> Vec<u8> {
    primase_dds::write_legacy(&Texture {
        width,
        height,
        format,
        levels,
        color_space: ColorSpace::Linear,
        role: TexRole::Color,
        normal: None,
    })
    .unwrap()
}

fn read(bytes: Vec<u8>) -> Result<TexSource<'static>, String> {
    TexSource::from_file_bytes("t.dds", bytes, None, None).map_err(|e| e.to_string())
}

/// A DXT1 chain 4 x 4, 2 x 2, 1 x 1 (one 8-byte block each) passes through and decodes.
#[test]
fn a_dxt1_file_reads_and_decodes_every_level() {
    let file = dds(4, 4, TexFormat::Bc1, vec![vec![0; 8]; 3]);
    let t = read(file).unwrap();
    assert_eq!((t.width, t.height, t.levels), (4, 4, 3));
    assert_eq!((t.fourcc, t.format_name.as_str()), (Some("DXT1"), "D3DFMT_DXT1"));
    assert_eq!(t.data_levels(), 3);
    assert_eq!(t.data().unwrap().unwrap().len(), 24);
    assert_eq!(t.decode(0, 4, 4).unwrap().unwrap().len(), 4 * 4 * 4);
    assert_eq!(t.decode(2, 1, 1).unwrap().unwrap().len(), 4);
}

/// A DDS of 0 x 0 used to read and then panic at `half`'s `expect`; the toolkit's reader refuses a zero dimension.
#[test]
fn a_dds_of_no_size_is_refused() {
    let mut file = dds(4, 4, TexFormat::Bc1, vec![vec![0; 8]; 3]);
    file[12..16].copy_from_slice(&0u32.to_le_bytes());
    file[16..20].copy_from_slice(&0u32.to_le_bytes());
    let e = read(file).err().unwrap();
    assert!(e.contains("t.dds") && e.contains("zero dimensions"), "{e}");
}

/// 2^32-1 square overflowed a u64 level size (`w * h * 4`): a panic with overflow checks, a wrapped slice without.
#[test]
fn a_dds_of_absurd_size_is_refused() {
    let mut file = dds(4, 4, TexFormat::Rgba8Unorm, vec![vec![0; 64]]);
    file[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    file[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(read(file).is_err());
}

/// The levels are exactly the bytes the header names: a cut file and a file with bytes after are refused.
#[test]
fn a_dds_cut_short_or_followed_by_bytes_is_refused() {
    let file = dds(4, 4, TexFormat::Bc1, vec![vec![0; 8]; 3]);
    let e = read(file[..file.len() - 3].to_vec()).err().unwrap();
    assert!(e.contains("truncated mip"), "{e}");
    let mut long = file;
    long.push(0);
    assert!(read(long).err().unwrap().contains("trailing bytes"));
}

/// Neither a PNG nor a DDS.
#[test]
fn another_file_is_refused_by_name() {
    assert!(
        read(b"RIFF0000WEBPVP8 ".to_vec())
            .err()
            .unwrap()
            .contains("neither a PNG nor a DDS")
    );
}

/// A PNG whose header states no size.
#[test]
fn a_png_of_no_size_is_refused() {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend([0, 0, 0, 13]);
    png.extend(b"IHDR");
    png.extend([0, 0, 0, 0, 0, 0, 0, 0, 8, 6, 0, 0, 0]);
    png.extend([0; 4]);
    let e = read(png).err().unwrap();
    assert!(e.contains("PNG of 0 x 0 pixels"), "{e}");
}

/// A PNG is one level; the chain below it is the box filter's.
#[test]
fn a_png_file_decodes_and_halves() {
    let image = primase_image::Image {
        width: 2,
        height: 2,
        samples: primase_image::Samples::U8(vec![0, 0, 0, 255, 4, 4, 4, 255, 4, 4, 4, 255, 8, 8, 8, 255]),
    };
    let t = read(primase_image::write_png(&image).unwrap()).unwrap();
    assert_eq!(
        (t.width, t.height, t.levels, t.format_name.as_str()),
        (2, 2, 2, "PNG")
    );
    assert_eq!(t.decode(0, 2, 2).unwrap().unwrap().len(), 16);
    assert_eq!(t.decode(1, 1, 1).unwrap().unwrap(), vec![4, 4, 4, 255]);
}
