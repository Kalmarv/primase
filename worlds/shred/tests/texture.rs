//! Decoding the stored pixels of hand-worked texture copies into core textures; no reference decoder of ours. The
//! headers and the catalog are tested in `rme-formats`.
use primase_core::{ColorSpace, TexRole};
use primase_shred::read::texture::{self, Error};
use rme_formats::{
    pak::Pak,
    texture::{Error as CatalogError, Payload, TextureCatalog},
};

fn word(b: &mut [u8], offset: usize, value: u32) {
    b[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}
fn header() -> Vec<u8> {
    let mut b = vec![0; 84];
    for (at, value) in [
        (0, 0xafaf0003),
        (4, 84),
        (8, 0x003c0000),
        (12, 0xbeefface),
        (16, 0x12345678),
        (20, 0x00020002),
        (28, 0x10203040),
        (32, 0x50607080),
        (56, 60),
        (60, 0x85020200),
        (64, 0xaae4),
        (68, 0x00020002),
        (72, 0x00010000),
    ] {
        word(&mut b, at, value);
    }
    b
}
// A synthetic stored PAK with named section markers and literal payloads.
fn pak(rows: &[(&str, Option<&[u8]>)]) -> Pak {
    let mut names = Vec::new();
    let mut offsets = Vec::new();
    for (name, _) in rows {
        offsets.push(names.len());
        names.extend(name.as_bytes());
        names.push(0);
    }
    let mut pos = 16 + 32 * rows.len() + names.len();
    let mut out = b"PAK\0\xfe\xeb\0\x04".to_vec();
    out.extend((rows.len() as u32).to_be_bytes());
    out.extend((names.len() as u32).to_be_bytes());
    for ((name, data), name_offset) in rows.iter().zip(offsets) {
        let size = data.map_or(0, |d| d.len());
        for n in [
            primase_foundations::hash::sdbm(name.as_bytes()),
            name_offset as u32,
            u32::from(data.is_none()),
            if data.is_none() { 0 } else { 16 },
            pos as u32,
            size as u32,
            0,
            0,
        ] {
            out.extend(n.to_be_bytes());
        }
        pos += size;
    }
    out.extend(names);
    for (_, data) in rows {
        if let Some(data) = data {
            out.extend_from_slice(data);
        }
    }
    Pak::from_bytes(out).expect("fixture PAK")
}

#[test]
fn every_copy_decodes_to_the_literal_pixels() {
    let h = header();
    let pixels = [255, 1, 2, 3].repeat(5);
    let a = pak(&[
        ("TEXTURES_STREAMED", None),
        ("art\\paint.thp", Some(&h)),
        ("art\\paint.txp", Some(&pixels)),
        ("TEXTURES_PREFETCH", None),
        ("art\\paint.thp", Some(&h)),
        ("art\\paint.txp", Some(&pixels)),
    ]);
    let b = pak(&[
        ("TEXTURES_PREFETCH", None),
        ("art\\paint.thp", Some(&h)),
        ("art\\paint.txp", Some(&pixels)),
    ]);
    let c = TextureCatalog::new(&[("a", &a), ("b", &b)]).unwrap();
    for i in 0..3 {
        let image = texture::decode(&c, i, Payload::Primary, ColorSpace::Linear, TexRole::Color).unwrap();
        assert_eq!(image[0].levels, [[1, 2, 3, 255].repeat(4), vec![1, 2, 3, 255]]);
    }
}

#[test]
fn an_absent_or_unknown_payload_is_a_catalog_error() {
    let h = header();
    let pixels = [0; 20];
    let a = pak(&[
        ("MAIN", None),
        ("ui.thp", Some(&h)),
        ("missing.thp", Some(&h)),
        ("LOCAL", None),
        ("ui.txp", Some(&pixels)),
    ]);
    let c = TextureCatalog::new(&[("a", &a)]).unwrap();
    for texture in [1, 99] {
        assert!(matches!(
            texture::decode(&c, texture, Payload::Primary, ColorSpace::Linear, TexRole::Color),
            Err(Error::Catalog(CatalogError::Copy { .. }))
        ));
    }
}

#[test]
fn a_cube_tail_decodes_to_six_faces() {
    let mut h = header();
    h[63] = 1;
    let pixels = [0; 660]; // each 2x2+1x1 ARGB chain 20 bytes, stride128; last unpadded.
    let a = pak(&[
        ("TEXTURES_STREAMED", None),
        ("cube.thp", Some(&h)),
        ("cube.txp", Some(&pixels)),
        ("cube_m1.txp", Some(&pixels)),
    ]);
    let c = TextureCatalog::new(&[("a", &a)]).unwrap();
    assert_eq!(
        texture::decode(&c, 0, Payload::CubeTail, ColorSpace::Linear, TexRole::Color)
            .unwrap()
            .len(),
        6
    );
}

#[test]
fn the_2d_decoder_refuses_a_volume() {
    let mut h = header();
    h[62] = 3;
    let a = pak(&[("MAIN", None), ("v.thp", Some(&h)), ("v.txp", Some(&[0; 20]))]);
    let c = TextureCatalog::new(&[("a", &a)]).unwrap();
    assert!(matches!(
        texture::decode(&c, 0, Payload::Primary, ColorSpace::Linear, TexRole::Color),
        Err(Error::Gcm(primase_texcodec::gcm::Error::Dimension { .. }))
    ));
}
