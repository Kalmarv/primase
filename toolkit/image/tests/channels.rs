use primase_core::{ColorSpace, TexFormat, TexRole, Texture};
use primase_image::{Error, TextureMetadata, map_rgba8_levels};

fn metadata() -> TextureMetadata {
    TextureMetadata {
        color_space: ColorSpace::Linear,
        role: TexRole::Mask,
        normal: None,
    }
}
fn source() -> Texture {
    Texture {
        width: 3,
        height: 2,
        format: TexFormat::Rgba8Unorm,
        levels: vec![(0..24).collect(), vec![100, 101, 102, 103]],
        color_space: ColorSpace::Srgb,
        role: TexRole::Color,
        normal: None,
    }
}

#[test]
fn mips_coordinates_and_metadata_are_explicit() {
    let source = source();
    let mut calls = Vec::new();
    let output = map_rgba8_levels(&source, 0..2, metadata(), |t| {
        calls.push((t.level, t.x, t.y, t.width, t.height));
        let [r, g, b, _] = t.rgba;
        Ok([b, 255 - g, r, 255])
    })
    .unwrap();
    assert_eq!(
        calls,
        [
            (0, 0, 0, 3, 2),
            (0, 1, 0, 3, 2),
            (0, 2, 0, 3, 2),
            (0, 0, 1, 3, 2),
            (0, 1, 1, 3, 2),
            (0, 2, 1, 3, 2),
            (1, 0, 0, 1, 1)
        ]
    );
    assert_eq!(
        output.levels[0],
        [
            2, 254, 0, 255, 6, 250, 4, 255, 10, 246, 8, 255, 14, 242, 12, 255, 18, 238, 16, 255, 22, 234, 20,
            255
        ]
    );
    assert_eq!(output.levels[1], [102, 154, 100, 255]);
    assert_eq!(output.color_space, ColorSpace::Linear);
    assert_eq!(output.role, TexRole::Mask);
    let base = map_rgba8_levels(&source, 0..1, metadata(), |t| Ok(t.rgba)).unwrap();
    assert_eq!((base.width, base.height, base.levels.len()), (3, 2, 1));
    let lower = map_rgba8_levels(&source, 1..2, metadata(), |t| Ok(t.rgba)).unwrap();
    assert_eq!((lower.width, lower.height), (1, 1));
    assert_eq!(lower.levels, [source.levels[1].clone()]);
}

#[test]
fn every_rgba8_byte_is_preserved_without_implicit_transfer() {
    let mut source = source();
    source.width = 256;
    source.height = 1;
    source.levels = vec![(0..=255).flat_map(|v| [v; 4]).collect()];
    let out = map_rgba8_levels(&source, 0..1, metadata(), |t| Ok(t.rgba)).unwrap();
    assert_eq!(out.levels, source.levels);
}

#[test]
fn compressed_source_uses_raw_decoded_channels() {
    let mut source = source();
    source.width = 4;
    source.height = 4;
    source.format = TexFormat::Bc1;
    source.levels = vec![vec![0, 248, 0, 0, 0, 0, 0, 0]]; // RGB565 red, every selector zero.
    let out = map_rgba8_levels(&source, 0..1, metadata(), |t| {
        assert_eq!(t.rgba, [255, 0, 0, 255]);
        Ok([t.rgba[1], t.rgba[0], t.rgba[2], t.rgba[3]])
    })
    .unwrap();
    assert_eq!(out.levels[0], [0, 255, 0, 255].repeat(16));
}

#[test]
fn malformed_selected_mips_fail_before_any_callback() {
    let mut source = source();
    source.levels[1].push(0);
    assert!(map_rgba8_levels(&source, 0..2, metadata(), |_| panic!("preflight first")).is_err());
    assert!(map_rgba8_levels(&source, 0..3, metadata(), |_| panic!("range first")).is_err());
    // Explicit base-only policy does not decode unrelated stored levels.
    assert!(map_rgba8_levels(&source, 0..1, metadata(), |t| Ok(t.rgba)).is_ok());
    source.width = 0;
    assert!(map_rgba8_levels(&source, 0..1, metadata(), |_| panic!("dimensions first")).is_err());
}

#[test]
fn callback_failure_does_not_change_source() {
    let source = source();
    let before = source.clone();
    let result = map_rgba8_levels(&source, 0..1, metadata(), |_| {
        Err(Error::Invalid("sampling failed".into()))
    });
    assert!(result.is_err());
    assert_eq!(source, before);
}

/// [`map_rgba8_levels_with`] on several threads makes the serial texture texel for texel (a BC1 source of three levels,
/// sizes not whole blocks, a mapping of each texel's own channels and coordinates), and an error is the first failing
/// row's, as the serial one's.
#[test]
fn the_threaded_mapping_is_the_serial_one() {
    use primase_foundations::parallel::Threads;
    use primase_image::map_rgba8_levels_with;
    let mut s = 12345u32;
    let mut noise = |n: usize| -> Vec<u8> {
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s >> 24) as u8
            })
            .collect()
    };
    let (w, h) = (75u32, 51u32);
    let blocks = |w: u32, h: u32| (w.div_ceil(4) * h.div_ceil(4) * 8) as usize;
    let source = Texture {
        width: w,
        height: h,
        format: TexFormat::Bc1,
        levels: vec![noise(blocks(w, h)), noise(blocks(37, 25)), noise(blocks(18, 12))],
        color_space: ColorSpace::Srgb,
        role: TexRole::Color,
        normal: None,
    };
    let map = |t: primase_image::Rgba8Texel| -> Result<[u8; 4], Error> {
        let [r, g, b, a] = t.rgba;
        Ok([
            b ^ (t.x as u8),
            r.wrapping_add(t.y as u8),
            g,
            a.wrapping_sub(t.level as u8),
        ])
    };
    let serial = map_rgba8_levels(&source, 0..3, metadata(), map).unwrap();
    for n in [2, 5, 16] {
        let threads = Threads::new(n).unwrap();
        assert_eq!(
            map_rgba8_levels_with(threads, &source, 0..3, metadata(), map).unwrap(),
            serial,
            "{n} threads"
        );
        assert_eq!(
            map_rgba8_levels_with(threads, &source, 1..3, metadata(), map).unwrap(),
            map_rgba8_levels(&source, 1..3, metadata(), map).unwrap()
        );
    }
    let fail = |t: primase_image::Rgba8Texel| -> Result<[u8; 4], Error> {
        if t.level == 0 && t.y >= 20 && t.x == 3 {
            Err(Error::Invalid(format!("row {}", t.y)))
        } else {
            Ok(t.rgba)
        }
    };
    let serial = map_rgba8_levels(&source, 0..3, metadata(), fail)
        .unwrap_err()
        .to_string();
    let many = map_rgba8_levels_with(Threads::new(8).unwrap(), &source, 0..3, metadata(), fail)
        .unwrap_err()
        .to_string();
    assert_eq!(many, serial);
    assert!(serial.contains("row 20"), "{serial}");
}
