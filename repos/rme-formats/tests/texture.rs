//! Hand-worked headers, section placement and stored pixel bytes of the texture catalog.
use rme_formats::{
    pak::Pak,
    texture::{Error, Payload, Residency, TextureCatalog, TextureHeader},
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
fn all_copies_across_archives_and_opaque_words() {
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
    assert_eq!(c.resolve("art/paint").count(), 3);
    assert_eq!(c.resolve("ART/paint").count(), 0);
    assert_eq!(c.archive_label(1), Some("b"));
    assert_eq!(c.records()[0].residency, Residency::Streamed);
    assert_eq!(c.records()[1].residency, Residency::Prefetch);
    assert_eq!(c.records()[0].header.raw, h.as_slice());
    assert_eq!(
        c.records()[0].header.opaque_words,
        [(16, 0x12345678), (28, 0x10203040), (32, 0x50607080)]
    );
    for i in 0..3 {
        let (descriptor, first_mip, bytes) = c.pixel_data(i, Payload::Primary).unwrap();
        assert_eq!(
            (
                descriptor.format,
                descriptor.mipmap,
                descriptor.width,
                descriptor.height
            ),
            (0x85, 2, 2, 2)
        );
        assert_eq!(first_mip, 0);
        assert_eq!(&*bytes, pixels.as_slice());
        assert_eq!(&*c.encoded(i, Payload::Primary).unwrap(), pixels.as_slice());
    }
}

#[test]
fn local_split_missing_and_ambiguous_pixels() {
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
    assert!(c.records()[0].pixels.is_some());
    assert!(c.records()[1].pixels.is_none());
    assert!(matches!(
        c.pixel_data(1, Payload::Primary),
        Err(Error::Copy { .. })
    ));
    assert!(matches!(
        c.pixel_data(99, Payload::Primary),
        Err(Error::Copy { .. })
    ));
    let a = pak(&[
        ("MAIN", None),
        ("ui.thp", Some(&h)),
        ("ui.txp", Some(&pixels)),
        ("LOCAL", None),
        ("ui.txp", Some(&pixels)),
    ]);
    assert!(matches!(
        TextureCatalog::new(&[("a", &a)]),
        Err(Error::Copy { .. })
    ));
}

#[test]
fn cube_tail_is_explicit_and_prefetch_cutoff_uses_smaller_axis() {
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
    assert!(c.records()[0].cube_tail.is_some());
    let (descriptor, _, bytes) = c.pixel_data(0, Payload::CubeTail).unwrap();
    assert_eq!(descriptor.cubemap, 1);
    assert_eq!(bytes.len(), 660);
    let mut parsed = TextureHeader::parse(&h).unwrap();
    parsed.gcm.width = 512;
    parsed.gcm.height = 256;
    parsed.gcm.mipmap = 10;
    assert_eq!(parsed.tail_first_mip(), 1);
    parsed.gcm.mipmap = 1;
    assert_eq!(parsed.tail_first_mip(), 0);
}

#[test]
fn header_and_gcm_refusals_are_named() {
    let h = header();
    assert!(matches!(
        TextureHeader::parse(&h[..83]),
        Err(Error::Header { .. })
    ));
    for offset in [0, 4, 8, 12, 20, 52, 56] {
        let mut bad = h.clone();
        bad[offset] ^= 1;
        assert!(
            matches!(TextureHeader::parse(&bad), Err(Error::Header { .. })),
            "{offset}"
        );
    }
    // The catalog keeps a volume's descriptor as stored; refusing its layout is the decoder's.
    let mut h = h;
    h[62] = 3;
    let a = pak(&[("MAIN", None), ("v.thp", Some(&h)), ("v.txp", Some(&[0; 20]))]);
    let c = TextureCatalog::new(&[("a", &a)]).unwrap();
    assert_eq!(c.pixel_data(0, Payload::Primary).unwrap().0.dimension, 3);
}
