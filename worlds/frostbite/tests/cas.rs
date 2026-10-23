//! CAS block streams: fixtures, strict framing, round trips, and (with the game's Oodle libraries configured) the DLL held to
//! a second Kraken decoder.

#![allow(clippy::unwrap_used)]
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "test I/O: real-data gates"
)]

use primase_compress::oodle::{Oodle, Version};
use primase_compress::{Compressor, Decompressor, reference};
use primase_core::gate::{GateReport, Tolerance};
use primase_frostbite::cas::{Cas, CasRaw, blocks};
use primase_testdata::{Game, OracleSet, Tool, oracle, tool};

type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Raw blocks round trip at zero, one, and both sides of the 64 KiB block boundary, and a decode of any other size is refused.
#[test]
fn raw_round_trips() -> R {
    let mut report = GateReport::new("cas raw synthetic", Tolerance::Exact);
    for size in [0, 1, 15, 65535, 65536, 65537, 131072] {
        let input: Vec<u8> = (0..size).map(|i| (i * 31 + i / 17) as u8).collect();
        let encoded = CasRaw.compress(&input)?;
        let cas = Cas::default();
        report.compare_bytes(format!("cas/{size}"), &cas.decompress(&encoded, size)?, &input);
        assert!(cas.decompress(&encoded, size + 1).is_err());
        if size > 0 {
            assert!(cas.decompress(&encoded, size - 1).is_err());
        }
    }
    println!("{report}");
    report.assert_passed();
    Ok(())
}

#[test]
fn strict_framing() -> R {
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
    assert_eq!(CasRaw.compress(&[])?, [0, 0, 0, 0, 0, 0x70, 0, 0]);
    Ok(())
}

#[test]
fn zlib_block_fixture() -> R {
    let z = [120, 156, 203, 72, 205, 201, 201, 7, 0, 6, 44, 2, 21];
    let mut b = vec![0, 0, 0, 5, 2, 0x70, 0, 13];
    b.extend(z);
    assert_eq!(Cas::default().decompress(&b, 5)?, b"hello");
    Ok(())
}

/// Installed DLL oracle versus independently implemented oozextract: CAS-framed Kraken for all three ABI versions when
/// configured, and the game's own Oodle blocks (`corpus.skate/cas`) against the second decoder.
#[test]
fn dll_cross_checks() -> R {
    for (oodle, version) in [
        (Tool::Oodle5, Version::V5),
        (Tool::Oodle6, Version::V6),
        (Tool::Oodle9, Version::V9),
    ] {
        let Some(path) = tool(oodle) else {
            continue;
        };
        // SAFETY: gate configuration explicitly identifies the user's trusted game DLL.
        #[allow(unsafe_code)]
        let dll = unsafe { Oodle::load(path, version)? };
        let cas = Cas { oodle: Some(&dll) };
        let mut report = GateReport::new(oodle.key(), Tolerance::Exact);
        for n in [0, 1, 65535, 65536, 65537, 131072, 262143, 262144, 262145, 524288] {
            let src: Vec<u8> = (0..n).map(|i| ((i * 17 + i / 256) % 251) as u8).collect();
            let framed = cas.compress(&src)?;
            report.compare_bytes(format!("cas/{n}"), &cas.decompress(&framed, n)?, &src);
        }
        if oodle == Tool::Oodle9
            && let Some(folder) = oracle(OracleSet::Corpus, Game::Skate, "cas")
        {
            let mut files = std::fs::read_dir(folder)?.collect::<Result<Vec<_>, _>>()?;
            files.sort_by_key(|f| f.path());
            for file in files {
                if file.path().extension().is_none_or(|s| s != "casblock") {
                    continue;
                }
                let bytes = std::fs::read(file.path())?;
                for block in blocks(&bytes)? {
                    let expected = if block.kind == 0 {
                        block.payload.to_vec()
                    } else {
                        reference::decode(block.payload, block.decoded_size)?
                    };
                    report.compare_bytes(
                        file.path().display().to_string(),
                        &cas.decompress(&bytes, block.decoded_size)?,
                        &expected,
                    );
                }
            }
        }
        println!("{report}");
        report.assert_passed();
    }
    Ok(())
}
