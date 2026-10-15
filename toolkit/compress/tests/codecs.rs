use primase_compress::{
    cas::{Cas, CasRaw, blocks},
    *,
};
use primase_core::gate::{GateReport, Tolerance};

/// Literal-only LZ4 and independently generated Python zlib fixtures; round trips
/// include zero, one, and both sides of CAS's 64 KiB boundary.
#[test]
fn fixtures_and_roundtrips() -> Result<()> {
    let mut report = GateReport::new("compression synthetic", Tolerance::Exact);
    report.compare_bytes(
        "LZ4 literal",
        &Lz4.decompress(&[0x50, b'h', b'e', b'l', b'l', b'o'], 5)?,
        b"hello",
    );
    report.compare_bytes(
        "zlib Python",
        &Zlib.decompress(&[120, 156, 203, 72, 205, 201, 201, 7, 0, 6, 44, 2, 21], 5)?,
        b"hello",
    );
    report.compare_bytes(
        "deflate Python",
        &Deflate.decompress(&[203, 72, 205, 201, 201, 7, 0], 5)?,
        b"hello",
    );
    for size in [0, 1, 15, 65535, 65536, 65537, 131072] {
        let input: Vec<u8> = (0..size).map(|i| (i * 31 + i / 17) as u8).collect();
        let codecs: [(&str, &dyn Compressor, &dyn Decompressor); 4] = [
            ("lz4", &Lz4, &Lz4),
            ("deflate", &Deflate, &Deflate),
            ("zlib", &Zlib, &Zlib),
            ("cas", &CasRaw, &Cas::default()),
        ];
        for (name, enc, dec) in codecs {
            let encoded = enc.compress(&input)?;
            report.compare_bytes(format!("{name}/{size}"), &dec.decompress(&encoded, size)?, &input);
            assert!(dec.decompress(&encoded, size + 1).is_err());
            if size > 0 {
                assert!(dec.decompress(&encoded, size - 1).is_err());
            }
        }
    }
    println!("{report}");
    report.assert_passed();
    Ok(())
}
#[test]
fn strict_framing() -> Result<()> {
    for codec in [&Deflate as &dyn Decompressor, &Zlib] {
        assert!(codec.decompress(&[], 0).is_err());
    }
    for (enc, dec) in [
        (&Deflate as &dyn Compressor, &Deflate as &dyn Decompressor),
        (&Zlib, &Zlib),
    ] {
        let bytes = enc.compress(b"hello")?;
        for n in 0..bytes.len() {
            assert!(dec.decompress(&bytes[..n], 5).is_err(), "prefix {n}");
        }
        let mut trailing = bytes;
        trailing.push(0);
        assert!(dec.decompress(&trailing, 5).is_err());
    }
    // Python zlib.compressobj + Z_SYNC_FLUSH: no BFINAL, no Adler trailer.
    let sync = [120, 156, 202, 72, 205, 201, 201, 7, 0, 0, 0, 255, 255];
    assert_eq!(ZlibUnterminated.decompress(&sync, 5)?, b"hello");
    assert!(Zlib.decompress(&sync, 5).is_err());
    let z = Zlib.compress(b"hello")?;
    assert_eq!(ZlibUnterminated.decompress(&z[..z.len() - 4], 5)?, b"hello");
    assert!(ZlibUnterminated.decompress(&z[..z.len() - 4], 6).is_err());
    assert!(ZlibUnterminated.decompress(&[], 0).is_err());
    let mut corrupt = z;
    let last = corrupt.len() - 1;
    corrupt[last] ^= 1;
    assert!(ZlibUnterminated.decompress(&corrupt, 5).is_err());
    let raw = CasRaw.compress(b"hello")?;
    for n in 1..raw.len() {
        assert!(Cas::default().decompress(&raw[..n], 5).is_err());
    }
    let mut unknown = raw.clone();
    unknown[4] = 0x7e;
    assert!(Cas::default().decompress(&unknown, 5).is_err());
    let mut dictionary = raw;
    dictionary[0] = 1;
    assert!(blocks(&dictionary).is_err());
    assert!(Lz4.decompress(&[0xf0], 15).is_err());
    assert_eq!(CasRaw.compress(&[])?, [0, 0, 0, 0, 0, 0x70, 0, 0]);
    Ok(())
}
#[test]
fn cas_zlib_fixture() -> Result<()> {
    let z = [120, 156, 203, 72, 205, 201, 201, 7, 0, 6, 44, 2, 21];
    let mut b = vec![0, 0, 0, 5, 2, 0x70, 0, 13];
    b.extend(z);
    assert_eq!(Cas::default().decompress(&b, 5)?, b"hello");
    Ok(())
}
