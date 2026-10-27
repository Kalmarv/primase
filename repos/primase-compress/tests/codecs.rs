use primase_compress::*;
use primase_core::gate::{GateReport, Tolerance};

/// Literal-only LZ4 and independently generated Python zlib fixtures; round trips
/// include zero, one, and both sides of a 64 KiB boundary.
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
        let codecs: [(&str, &dyn Compressor, &dyn Decompressor); 3] = [
            ("lz4", &Lz4, &Lz4),
            ("deflate", &Deflate, &Deflate),
            ("zlib", &Zlib, &Zlib),
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
    assert!(Lz4.decompress(&[0xf0], 15).is_err());
    Ok(())
}
/// One zstd frame laid out by hand (RFC 8878): single segment with a one-byte content size, then `blocks`, each
/// (last, kind, body): kind 0 raw (body is the bytes), 1 RLE (body is one byte repeated `size` times).
fn zstd_frame(content: u8, dict: bool, blocks: &[(bool, u8, &[u8], u32)]) -> Vec<u8> {
    let mut f = vec![0x28, 0xB5, 0x2F, 0xFD];
    // descriptor: single segment (0x20), dictionary id flag 1 (one byte) when asked
    f.push(if dict { 0x21 } else { 0x20 });
    if dict {
        f.push(7);
    }
    f.push(content);
    for &(last, kind, body, size) in blocks {
        let header = u32::from(last) | (u32::from(kind) << 1) | (size << 3);
        f.extend_from_slice(&header.to_le_bytes()[..3]);
        f.extend_from_slice(body);
    }
    f
}

/// zstd: raw and RLE blocks decode exactly; a wrong expected size, a stated content size that differs, trailing
/// input, a missing last block, a dictionary and an over-long output are refused.
#[test]
fn zstd_one_exact_frame() -> Result<()> {
    let raw = zstd_frame(5, false, &[(true, 0, b"hello", 5)]);
    assert_eq!(Zstd.decompress(&raw, 5)?, b"hello");
    let two = zstd_frame(8, false, &[(false, 0, b"ab", 2), (true, 1, b"z", 6)]);
    assert_eq!(Zstd.decompress(&two, 8)?, b"abzzzzzz");
    // the frame states 5 bytes
    assert!(Zstd.decompress(&raw, 4).is_err());
    assert!(Zstd.decompress(&raw, 6).is_err());
    let mut trailing = raw.clone();
    trailing.push(0);
    assert!(Zstd.decompress(&trailing, 5).is_err());
    let unfinished = zstd_frame(5, false, &[(false, 0, b"hello", 5)]);
    assert!(Zstd.decompress(&unfinished, 5).is_err());
    let dict = zstd_frame(5, true, &[(true, 0, b"hello", 5)]);
    assert!(Zstd.decompress(&dict, 5).is_err());
    // states 5 but holds 6
    let long = zstd_frame(5, false, &[(true, 0, b"hello!", 6)]);
    assert!(Zstd.decompress(&long, 5).is_err());
    assert!(Zstd.decompress(&raw[..raw.len() - 1], 5).is_err());
    Ok(())
}

/// A stream of unstated size inflates to what it holds, memory following the output and not the bound; more than the
/// bound, a missing end, trailing input and a damaged stream are refused.
#[test]
fn deflate_of_an_unstated_size_is_bounded_by_its_output() -> Result<()> {
    use primase_compress::DEFLATE_MAX_RATIO;
    let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    let packed = Deflate.compress(&data)?;
    assert_eq!(Deflate.decompress_up_to(&packed, data.len())?, data);
    assert_eq!(Deflate.decompress_up_to(&packed, usize::MAX)?, data);
    assert!(matches!(
        Deflate.decompress_up_to(&packed, data.len() - 1),
        Err(Error::Size { expected, actual }) if expected == data.len() - 1 && actual > expected
    ));
    // a run: the stream is tiny and the output the most it can be
    let zeros = Deflate.compress(&vec![0u8; 1 << 20])?;
    assert!(
        zeros.len() * DEFLATE_MAX_RATIO >= 1 << 20,
        "{} bytes",
        zeros.len()
    );
    assert_eq!(Deflate.decompress_up_to(&zeros, 1 << 20)?.len(), 1 << 20);
    assert!(
        Deflate
            .decompress_up_to(&packed[..packed.len() - 1], usize::MAX)
            .is_err()
    );
    let mut trailing = packed.clone();
    trailing.push(0);
    assert!(Deflate.decompress_up_to(&trailing, usize::MAX).is_err());
    assert!(Deflate.decompress_up_to(&[], usize::MAX).is_err());
    assert!(Deflate.decompress_up_to(&[0xFF; 16], usize::MAX).is_err());
    Ok(())
}
