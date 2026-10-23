//! Sekiro's KRAK DCX files (`corpus.sekiro/dcx`) against the game's Oodle 6 library and oozextract, a second, independent Kraken
//! decoder: the whole stream and each independent 256 KiB quantum must decode alike, stored quanta must equal their bytes, and the
//! DLL's own compression must round trip. The envelope is read by the production reader ([`Envelope::parse`]).

#![allow(clippy::unwrap_used)]
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "test I/O: real-data gates"
)]

use primase_compress::oodle::{Oodle, Version};
use primase_compress::reference::{self, Failure};
use primase_compress::{Compressor, Decompressor};
use primase_core::gate::{GateReport, Tolerance};
use primase_fromsoft::dcx::{Compression, Envelope};
use primase_testdata::{Game, OracleSet, Tool, oracle, tool};

type BoxError = Box<dyn std::error::Error>;

/// The quanta oozextract 0.5.6 refuses in `corpus.sekiro/dcx`, by file and quantum: its single-symbol Huffman
/// defect ([`oozextract_056_single_symbol_huffman_chunk_reports_no_bytes_used`]). The gate accepts a refusal there, of the quantum
/// and of its file's whole stream, and nowhere else.
const SINGLE_SYMBOL_HUFFMAN: &[(&str, usize)] = &[("0000000001b70e4e.dcx", 13)];

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
    let mut pinned = Vec::new();
    for file in files {
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
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
                Err(Failure::Refused(_)) if SINGLE_SYMBOL_HUFFMAN.contains(&(&name, i)) => {
                    pinned.push(format!("{name}/quantum/{i}"))
                }
                Err(e) => return Err(format!("{}/quantum/{i}: {e}", file.display()).into()),
            }
        }
        match reference::decode(src, expected) {
            Ok(other) => report.compare_bytes(file.display().to_string(), &got, &other),
            Err(Failure::Refused(_)) if SINGLE_SYMBOL_HUFFMAN.iter().any(|(pin, _)| *pin == name) => {
                pinned.push(name.clone())
            }
            Err(e) => return Err(format!("{}: {e}", file.display()).into()),
        }
        let encoded = dll.compress(&got)?;
        roundtrips.compare_bytes(
            file.display().to_string(),
            &got,
            &dll.decompress(&encoded, expected)?,
        );
    }
    println!(
        "{quantums}\n{stored}\n{roundtrips}\nDLL only, oozextract's pinned single-symbol Huffman defect: {pinned:?}"
    );
    quantums.assert_passed();
    stored.assert_passed();
    roundtrips.assert_passed();
    println!("{report}");
    report.assert_passed();
    Ok(())
}

/// oozextract 0.5.6 refuses a Huffman byte chunk (chunk type 2 or 4) whose code lengths name a single symbol: having
/// filled the output with that symbol, `decode_bytes_type12` returns `src - src_end`, 0 when the code-length
/// header ends the chunk, where its caller requires the chunk's size ("Expected 0 == 3 for chunk type 4"). The C
/// ooz it ports has the same return (kraken.cpp, Kraken_DecodeBytes_Type12). The game's Oodle 6 decodes the
/// chunk; with that return made the chunk's size, oozextract equals the DLL on every file of `corpus.sekiro/dcx`,
/// whole and per quantum. A fix upstream fails the final assertion: then empty
/// [`SINGLE_SYMBOL_HUFFMAN`] and remove this test.
#[test]
fn oozextract_056_single_symbol_huffman_chunk_reports_no_bytes_used()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let Some(root) = oracle(OracleSet::Corpus, Game::Sekiro, "dcx") else {
        return Ok(());
    };
    let Some(path) = tool(Tool::Oodle6) else {
        return Ok(());
    };
    // SAFETY: explicitly configured trusted version-6 game library.
    #[allow(unsafe_code)]
    let dll = unsafe { Oodle::load(path, Version::V6)? };
    for &(name, i) in SINGLE_SYMBOL_HUFFMAN {
        let bytes = std::fs::read(root.join(name))?;
        let (expected, src) = krak_payload(&bytes)?;
        let reference = dll.decompress(src, expected)?;
        let quantum = quanta(src, expected)?
            .into_iter()
            .nth(i)
            .ok_or("no such quantum")?;
        let reference = &reference[i * QUANTUM..i * QUANTUM + quantum.size];
        assert_eq!(
            dll.decompress(quantum.bytes, quantum.size)?,
            reference,
            "{name}/quantum/{i}: the DLL alone"
        );
        match reference::decode(quantum.bytes, quantum.size) {
            Err(Failure::Refused(e)) if e.message.as_deref() == Some("Expected 0 == 3 for chunk type 4") => {}
            Err(other) => return Err(format!("{name}/quantum/{i}: a different failure: {other}").into()),
            Ok(decoded) => {
                assert_eq!(decoded, reference, "{name}/quantum/{i}");
                panic!("upstream defect fixed: empty SINGLE_SYMBOL_HUFFMAN and remove this test");
            }
        }
    }
    Ok(())
}
