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
use primase_foundations::compress::oodle_framing::{self, BLOCK_SIZE, DecoderType};
use primase_foundations::compress::reference;
use primase_foundations::compress::{Compressor, Decompressor};
use primase_testdata::gate::{GateReport, Tolerance};

type BoxError = Box<dyn std::error::Error>;

/// One quantum of a KRAK payload (a Kraken block holds one): its bytes (block header included), its output size, and
/// whether it is stored.
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

/// The payload's quanta, each independent: its blocks as [`oodle_framing::walk`] frames them (which refuses the checksum
/// bit and bytes left after the last block), every one a Kraken block that restarts the decoder, compressed or stored.
fn quanta(src: &[u8], expected: usize) -> std::result::Result<Vec<Quantum<'_>>, BoxError> {
    let framing = oodle_framing::walk(src, expected)?;
    let ends = framing.blocks.iter().skip(1).map(|b| b.offset).chain([src.len()]);
    framing
        .blocks
        .iter()
        .zip(ends)
        .map(|(block, end)| {
            if !block.restart || block.decoder != DecoderType::Kraken {
                return Err("gate requires independent Kraken or stored quantum".into());
            }
            Ok(Quantum {
                bytes: src.get(block.offset..end).ok_or("block outside the payload")?,
                size: block.decoded,
                stored: block.stored,
            })
        })
        .collect()
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
        for (i, (quantum, reference)) in quanta.iter().zip(got.chunks(BLOCK_SIZE)).enumerate() {
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
