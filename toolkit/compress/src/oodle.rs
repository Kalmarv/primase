//! Runtime binding to a caller-selected, trusted Oodle library. No DLL discovery.
use crate::{Compressor, Decompressor, Result, buffer, check_size, invalid};
use std::{ffi::c_void, path::Path};
type Decode = unsafe extern "C" fn(
    *const u8,
    isize,
    *mut u8,
    isize,
    i32,
    i32,
    i32,
    *mut c_void,
    isize,
    *mut c_void,
    *mut c_void,
    *mut c_void,
    isize,
    i32,
) -> isize;
type Encode = unsafe extern "C" fn(
    i32,
    *const u8,
    isize,
    *mut u8,
    i32,
    *const c_void,
    *const c_void,
    *mut c_void,
    *mut c_void,
    isize,
) -> isize;
#[derive(Debug, Clone, Copy)]
pub enum Version {
    V5,
    V6,
    V9,
}
#[derive(Debug)]
enum Capacity {
    Legacy(unsafe extern "C" fn(isize) -> isize),
    Modern(unsafe extern "C" fn(i32, isize) -> isize),
}
/// Oodle 5/6/9 on 64-bit Windows (version-specific capacity-query ABI). Owns the library for
/// the entire lifetime of the copied function pointers. Compression is Kraken,
/// level 4; use [`crate::cas::Cas`] for independent 64 KiB CAS framing.
#[derive(Debug)]
pub struct Oodle {
    _library: libloading::Library,
    decode: Decode,
    encode: Encode,
    capacity: Capacity,
}
impl Oodle {
    /// Load exactly `path` (canonicalized, never searched by basename).
    ///
    /// # Safety
    /// Caller must trust the native library and its dependencies: loading executes
    /// native initialization. Exports must implement the selected Oodle 5/6/9 x64 ABI.
    #[allow(
        clippy::disallowed_methods,
        clippy::disallowed_types,
        reason = "toolkit I/O: loads the codec library the caller names"
    )]
    pub unsafe fn load(path: impl AsRef<Path>, version: Version) -> Result<Self> {
        let path = path.as_ref().canonicalize()?;
        // SAFETY: caller guarantees trusted initialization and matching export ABI;
        // the owned library outlives every function pointer below.
        unsafe {
            let library = libloading::Library::new(path)?;
            let decode = *library.get::<Decode>(b"OodleLZ_Decompress\0")?;
            let encode = *library.get::<Encode>(b"OodleLZ_Compress\0")?;
            let capacity = match version {
                Version::V5 | Version::V6 => {
                    Capacity::Legacy(*library.get(b"OodleLZ_GetCompressedBufferSizeNeeded\0")?)
                }
                Version::V9 => Capacity::Modern(*library.get(b"OodleLZ_GetCompressedBufferSizeNeeded\0")?),
            };
            Ok(Self {
                _library: library,
                decode,
                encode,
                capacity,
            })
        }
    }
}
impl Decompressor for Oodle {
    /// Oodle's one-shot ABI reports decoded size but NOT bytes consumed. The
    /// enclosing container must supply the exact compressed slice; this API cannot
    /// prove absence of trailing compressed bytes. Empty input/output is canonical.
    fn decompress(&self, input: &[u8], expected: usize) -> Result<Vec<u8>> {
        if expected == 0 {
            return if input.is_empty() {
                Ok(Vec::new())
            } else {
                Err(invalid("oodle", 0, "nonempty payload for empty output"))
            };
        }
        let src_len = isize::try_from(input.len()).map_err(|_| crate::Error::Limit)?;
        let dst_len = isize::try_from(expected).map_err(|_| crate::Error::Limit)?;
        let mut out = buffer(expected)?;
        // SAFETY: slices are live, non-overlapping and sized as passed; fuzz-safe
        // decode enabled, no callbacks or external dictionary, synchronous phases.
        let got = unsafe {
            (self.decode)(
                input.as_ptr(),
                src_len,
                out.as_mut_ptr(),
                dst_len,
                1,
                0,
                0,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
                3,
            )
        };
        if got <= 0 {
            return Err(invalid("oodle", 0, format!("decode returned {got}")));
        }
        check_size(got as usize, expected)?;
        Ok(out)
    }
}
impl Compressor for Oodle {
    fn compress(&self, input: &[u8]) -> Result<Vec<u8>> {
        if input.is_empty() {
            return Ok(Vec::new());
        }
        let n = isize::try_from(input.len()).map_err(|_| crate::Error::Limit)?;
        // SAFETY: pure size query with Kraken enum 8 and validated positive length.
        let cap = unsafe {
            match self.capacity {
                Capacity::Legacy(f) => f(n),
                Capacity::Modern(f) => f(8, n),
            }
        };
        if cap < n {
            return Err(invalid("oodle", 0, "invalid compressed capacity"));
        }
        let mut out = buffer(cap as usize)?;
        // SAFETY: output has the DLL-reported capacity; input is live and disjoint;
        // default options, no dictionary, long-range matcher or external scratch.
        let got = unsafe {
            (self.encode)(
                8,
                input.as_ptr(),
                n,
                out.as_mut_ptr(),
                4,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            )
        };
        if got <= 0 || got > cap {
            return Err(invalid(
                "oodle",
                0,
                format!("compress returned {got} for capacity {cap}"),
            ));
        }
        out.truncate(got as usize);
        Ok(out)
    }
}

/// oozextract, the second Kraken decoder the tests hold the DLL to, behind one boundary: its refusals and its panics
/// (its decoder asserts where it should refuse) both come back as a [`second_decoder::Failure`], so a corrupt or
/// unsupported stream is a named test error and never a panic inside a gate.
#[cfg(test)]
mod second_decoder {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    #[derive(Debug, thiserror::Error)]
    pub(super) enum Failure {
        #[error("oozextract refused the stream: {0}")]
        Refused(oozextract::OozError),
        #[error("oozextract panicked instead of refusing the stream (an upstream defect): {0}")]
        Panicked(String),
        #[error("oozextract wrote {written} bytes, expected {expected}")]
        Short { written: usize, expected: usize },
    }
    /// The `expected` bytes oozextract decodes from `input`.
    pub(super) fn decode(input: &[u8], expected: usize) -> Result<Vec<u8>, Failure> {
        let mut out = vec![0; expected];
        let written = catch_unwind(AssertUnwindSafe(|| {
            oozextract::Extractor::new().read_from_slice(input, &mut out)
        }))
        .map_err(|payload| {
            Failure::Panicked(
                payload
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_owned())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "a panic without a message".to_owned()),
            )
        })?
        .map_err(Failure::Refused)?;
        if written != expected {
            return Err(Failure::Short { written, expected });
        }
        Ok(out)
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "test I/O: real-data gates"
)]
mod tests {
    use super::*;
    use primase_core::gate::{GateReport, Tolerance};
    use primase_testdata::{Game, OracleSet, Tool, oracle, tool};
    /// Installed DLL oracle versus independently implemented oozextract; tests
    /// synthetic Kraken compression for all three ABI versions when configured.
    #[test]
    fn dll_cross_checks() -> std::result::Result<(), Box<dyn std::error::Error>> {
        for (oodle, version) in [
            (Tool::Oodle5, Version::V5),
            (Tool::Oodle6, Version::V6),
            (Tool::Oodle9, Version::V9),
        ] {
            let Some(path) = tool(oodle) else {
                continue;
            };
            // SAFETY: gate configuration explicitly identifies the user's trusted game DLL.
            let dll = unsafe { Oodle::load(path, version)? };
            let mut report = GateReport::new(oodle.key(), Tolerance::Exact);
            for n in [0, 1, 65535, 65536, 65537, 131072, 262143, 262144, 262145, 524288] {
                let src: Vec<u8> = (0..n).map(|i| ((i * 17 + i / 256) % 251) as u8).collect();
                let compressed = dll.compress(&src)?;
                report.compare_bytes(format!("dll/{n}"), &dll.decompress(&compressed, n)?, &src);
                if n != 0 {
                    report.compare_bytes(format!("ooz/{n}"), &second_decoder::decode(&compressed, n)?, &src);
                }
                let cas = crate::cas::Cas { oodle: Some(&dll) };
                let framed = cas.compress(&src)?;
                report.compare_bytes(format!("cas/{n}"), &cas.decompress(&framed, n)?, &src);
            }
            if oodle == Tool::Oodle9
                && let Some(folder) = oracle(OracleSet::Corpus, Game::Skate, "cas")
            {
                let mut files = std::fs::read_dir(folder)?.collect::<std::result::Result<Vec<_>, _>>()?;
                files.sort_by_key(|f| f.path());
                for file in files {
                    if file.path().extension().is_none_or(|s| s != "casblock") {
                        continue;
                    }
                    let bytes = std::fs::read(file.path())?;
                    for block in crate::cas::blocks(&bytes)? {
                        let reference = if block.kind == 0 {
                            block.payload.to_vec()
                        } else {
                            second_decoder::decode(block.payload, block.decoded_size)?
                        };
                        let cas = crate::cas::Cas { oodle: Some(&dll) };
                        report.compare_bytes(
                            file.path().display().to_string(),
                            &cas.decompress(&bytes, block.decoded_size)?,
                            &reference,
                        );
                    }
                }
            }
            println!("{report}");
            report.assert_passed();
        }
        Ok(())
    }

    /// Two independent 256 KiB Kraken quanta, concatenated: oozextract up to 0.5.5 kept the first quantum's output
    /// base after the second's decoder restart and refused (or panicked on) the whole stream; 0.5.6 decodes it, and
    /// both decoders must agree.
    #[test]
    fn concatenated_quanta_restart_their_decoder() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let Some(path) = tool(Tool::Oodle6) else {
            return Ok(());
        };
        // SAFETY: explicitly configured trusted version-6 game library.
        let dll = unsafe { Oodle::load(path, Version::V6)? };
        let mut raw = Vec::new();
        let mut encoded = Vec::new();
        for j in 0..2 {
            let part: Vec<_> = (0..262144)
                .map(|i| ((i * 17 + i / 256 + j) % 251) as u8)
                .collect();
            let compressed = dll.compress(&part)?;
            assert_eq!(second_decoder::decode(&compressed, part.len())?, part);
            raw.extend(part);
            encoded.extend(compressed);
        }
        assert_eq!(dll.decompress(&encoded, raw.len())?, raw);
        assert_eq!(second_decoder::decode(&encoded, raw.len())?, raw);
        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "test I/O: real-data gates"
)]
mod dcx_gate {
    use super::*;
    use primase_core::gate::{GateReport, Tolerance};
    use primase_testdata::{Game, OracleSet, Tool, oracle, tool};
    type BoxError = Box<dyn std::error::Error>;
    /// The quanta oozextract 0.5.6 refuses in `corpus.sekiro/dcx`, by file and quantum: its single-symbol Huffman
    /// defect ([`super::upstream_defect`]). The gate accepts a refusal there, of the quantum and of its file's whole
    /// stream, and nowhere else.
    pub(super) const SINGLE_SYMBOL_HUFFMAN: &[(&str, usize)] = &[("0000000001b70e4e.dcx", 13)];
    /// Kraken's output quantum: each starts a fresh decoder.
    pub(super) const QUANTUM: usize = 262144;
    /// One quantum of a KRAK payload: its bytes (header included), its output size, and whether it is stored.
    pub(super) struct Quantum<'a> {
        pub bytes: &'a [u8],
        pub size: usize,
        pub stored: bool,
    }
    /// Test-only container adapter following SoulsFormats DCX.DecompressDCXKRAK: the payload and its decoded size.
    /// Production container parsing belongs in the FromSoft world crate.
    pub(super) fn krak_payload(b: &[u8]) -> std::result::Result<(usize, &[u8]), BoxError> {
        if b.get(40..44) != Some(b"KRAK") {
            return Err("unsupported DCX".into());
        }
        let u32be = |o: usize| -> std::result::Result<u32, BoxError> {
            Ok(u32::from_be_bytes(
                b.get(o..o + 4).ok_or("short DCX")?.try_into()?,
            ))
        };
        for (o, v) in [
            (0, 0x44435800),
            (4, 0x11000),
            (8, 0x18),
            (12, 0x24),
            (16, 0x44),
            (20, 0x4c),
            (24, 0x44435300),
            (36, 0x44435000),
            (44, 0x20),
            (52, 0),
            (56, 0),
            (60, 0),
            (64, 0x10100),
            (68, 0x44434100),
            (72, 8),
        ] {
            if u32be(o)? != v {
                return Err(format!("DCX header at {o:#x}").into());
            }
        }
        if ![0x06000000, 0x09000000].contains(&u32be(48)?) {
            return Err("DCX level".into());
        }
        let expected = u32be(28)? as usize;
        let size = u32be(32)? as usize;
        let src = b.get(76..76 + size).ok_or("short DCX payload")?;
        if b[76 + size..].iter().any(|&v| v != 0) || b.len() - 76 - size >= 16 {
            return Err("DCX padding".into());
        }
        Ok((expected, src))
    }
    /// The payload's quanta, each independent (a decoder restart, no checksum): a Kraken block (`8c 06` and a
    /// three-byte compressed-size header) or a stored one (`cc 06` and the quantum's bytes as they are).
    pub(super) fn quanta(src: &[u8], expected: usize) -> std::result::Result<Vec<Quantum<'_>>, BoxError> {
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
                let v =
                    (usize::from(header[0]) << 16) | (usize::from(header[1]) << 8) | usize::from(header[2]);
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
        let mut roundtrips =
            GateReport::new("DCX decoded bytes DLL compression roundtrips", Tolerance::Exact);
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
                match second_decoder::decode(quantum.bytes, quantum.size) {
                    Ok(decoded) => {
                        quantums.compare_bytes(format!("{}/quantum/{i}", file.display()), &decoded, reference)
                    }
                    Err(second_decoder::Failure::Refused(_))
                        if SINGLE_SYMBOL_HUFFMAN.contains(&(&name, i)) =>
                    {
                        pinned.push(format!("{name}/quantum/{i}"))
                    }
                    Err(e) => return Err(format!("{}/quantum/{i}: {e}", file.display()).into()),
                }
            }
            match second_decoder::decode(src, expected) {
                Ok(other) => report.compare_bytes(file.display().to_string(), &got, &other),
                Err(second_decoder::Failure::Refused(_))
                    if SINGLE_SYMBOL_HUFFMAN.iter().any(|(pin, _)| *pin == name) =>
                {
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
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "test I/O: real-data gates"
)]
mod upstream_defect {
    use super::dcx_gate::{QUANTUM, SINGLE_SYMBOL_HUFFMAN, krak_payload, quanta};
    use super::second_decoder::{Failure, decode};
    use super::*;
    use primase_testdata::{Game, OracleSet, Tool, oracle, tool};
    /// oozextract 0.5.6 refuses a Huffman byte chunk (chunk type 2 or 4) whose code lengths name a single symbol: having
    /// filled the output with that symbol, `decode_bytes_type12` returns `src - src_end`, 0 when the code-length
    /// header ends the chunk, where its caller requires the chunk's size ("Expected 0 == 3 for chunk type 4"). The C
    /// ooz it ports has the same return (kraken.cpp, Kraken_DecodeBytes_Type12). The game's Oodle 6 decodes the
    /// chunk; with that return made the chunk's size, oozextract equals the DLL on every file of `corpus.sekiro/dcx`,
    /// whole and per quantum. A fix upstream fails the final assertion: then empty
    /// [`super::dcx_gate::SINGLE_SYMBOL_HUFFMAN`] and remove this test.
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
            match decode(quantum.bytes, quantum.size) {
                Err(Failure::Refused(e))
                    if e.message.as_deref() == Some("Expected 0 == 3 for chunk type 4") => {}
                Err(other) => return Err(format!("{name}/quantum/{i}: a different failure: {other}").into()),
                Ok(decoded) => {
                    assert_eq!(decoded, reference, "{name}/quantum/{i}");
                    panic!("upstream defect fixed: empty SINGLE_SYMBOL_HUFFMAN and remove this test");
                }
            }
        }
        Ok(())
    }
}
