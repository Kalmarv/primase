//! Sekiro's KRAK DCX files (`corpus.sekiro/dcx`) against the game's Oodle 6 library and oozextract, a second, independent Kraken
//! decoder: the whole stream and each independent 256 KiB quantum must decode alike, stored quanta must equal their bytes, and the
//! DLL's own compression must round trip. The envelope is read by the production reader ([`Envelope::parse`]).

#![allow(clippy::unwrap_used)]
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "test I/O: real-data gates"
)]

mod inputs;

use fromsoft_formats::dcx::{Compression, Envelope};
use inputs::{Game, OracleSet, Tool, oracle, tool};
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_foundations::compress::reference;
use primase_foundations::compress::{Compressor, Decompressor};
use primase_testdata::gate::{GateReport, Tolerance};

type BoxError = Box<dyn std::error::Error>;

/// Kraken's output quantum: each starts a fresh decoder.
const QUANTUM: usize = 262144;
/// One quantum of a KRAK payload: its bytes (header included), its output size, and whether it is stored.
struct Quantum<'a> {
    bytes: &'a [u8],
    size: usize,
    stored: bool,
}

/// A KRAK envelope's payload and its decoded size, by the production reader.
fn krak_payload(bytes: &[u8]) -> Result<(usize, &[u8]), BoxError> {
    let envelope = Envelope::parse(bytes, usize::MAX)?;
    if envelope.compression != Compression::Kraken {
        return Err("not a KRAK envelope".into());
    }
    Ok((envelope.decoded_size, envelope.payload))
}

/// The payload's quanta, each independent (a decoder restart, no checksum): a Kraken block (`8c 06` and a
/// three-byte compressed-size header) or a stored one (`cc 06` and the quantum's bytes as they are).
fn quanta(src: &[u8], expected: usize) -> std::result::Result<Vec<Quantum<'_>>, BoxError> {
    let mut out = Vec::new();
    let mut pos = 0;
    for i in 0..expected.div_ceil(QUANTUM) {
        let start = pos;
        let size = (expected - i * QUANTUM).min(QUANTUM);
        let stored = match src.get(pos..pos + 2) {
            Some([0x8c, 0x06]) => false,
            Some([0xcc, 0x06]) => true,
            _ => return Err("gate requires independent Kraken or stored quantum, no checksum".into()),
        };
        pos += 2;
        if stored {
            pos += size;
        } else {
            let header = src.get(pos..pos + 3).ok_or("short quantum header")?;
            let v = (usize::from(header[0]) << 16) | (usize::from(header[1]) << 8) | usize::from(header[2]);
            pos += 3;
            pos += if v & 0x3ffff == 0x3ffff {
                if v >> 18 != 1 {
                    return Err("unknown quantum special header".into());
                }
                1
            } else {
                (v & 0x3ffff) + 1
            };
        }
        out.push(Quantum {
            bytes: src.get(start..pos).ok_or("short quantum payload")?,
            size,
            stored,
        });
    }
    if pos != src.len() {
        return Err("trailing quantum bytes".into());
    }
    Ok(out)
}

#[test]
fn loose_krak_dcx() -> std::result::Result<(), BoxError> {
    let Some(root) = oracle(OracleSet::Corpus, Game::Sekiro, "dcx") else {
        return Ok(());
    };
    let Some(path) = tool(Tool::Oodle6) else {
        return Ok(());
    };
    // SAFETY: explicit trusted installed game's version-6 library.
    #[allow(unsafe_code)]
    let dll = unsafe { Oodle::load(path, Version::V6)? };
    let mut pending = vec![root];
    let mut files = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            if ty.is_dir() {
                pending.push(entry.path());
            } else if ty.is_file() && entry.path().extension().is_some_and(|s| s == "dcx") {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    let mut report = GateReport::new("loose KRAK DCX DLL6 vs oozextract", Tolerance::Exact);
    let mut quantums = GateReport::new("independent KRAK quantums vs oozextract", Tolerance::Exact);
    let mut stored = GateReport::new("stored KRAK quantums vs their bytes", Tolerance::Exact);
    let mut roundtrips = GateReport::new("DCX decoded bytes DLL compression roundtrips", Tolerance::Exact);
    for file in files {
        let b = std::fs::read(&file)?;
        let (expected, src) = krak_payload(&b).map_err(|e| format!("{}: {e}", file.display()))?;
        let got = dll.decompress(src, expected)?;
        let quanta = quanta(src, expected).map_err(|e| format!("{}: {e}", file.display()))?;
        for (i, (quantum, reference)) in quanta.iter().zip(got.chunks(QUANTUM)).enumerate() {
            if quantum.stored {
                stored.compare_bytes(
                    format!("{}/quantum/{i}", file.display()),
                    &quantum.bytes[2..],
                    reference,
                );
            }
            match reference::decode(quantum.bytes, quantum.size) {
                Ok(decoded) => {
                    quantums.compare_bytes(format!("{}/quantum/{i}", file.display()), &decoded, reference)
                }
                Err(e) => return Err(format!("{}/quantum/{i}: {e}", file.display()).into()),
            }
        }
        match reference::decode(src, expected) {
            Ok(other) => report.compare_bytes(file.display().to_string(), &got, &other),
            Err(e) => return Err(format!("{}: {e}", file.display()).into()),
        }
        let encoded = dll.compress(&got)?;
        roundtrips.compare_bytes(
            file.display().to_string(),
            &got,
            &dll.decompress(&encoded, expected)?,
        );
    }
    println!("{quantums}\n{stored}\n{roundtrips}");
    quantums.assert_passed();
    stored.assert_passed();
    roundtrips.assert_passed();
    println!("{report}");
    report.assert_passed();
    Ok(())
}
