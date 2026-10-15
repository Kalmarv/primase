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

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::gate::{GateReport, Tolerance, real_data};
    /// Installed DLL oracle versus independently implemented oozextract; tests
    /// synthetic Kraken compression for all three ABI versions when configured.
    #[test]
    fn dll_cross_checks() -> std::result::Result<(), Box<dyn std::error::Error>> {
        for (var, version) in [
            ("PRIMASE_OODLE5", Version::V5),
            ("PRIMASE_OODLE6", Version::V6),
            ("PRIMASE_OODLE9", Version::V9),
        ] {
            let Some(path) = real_data(var) else {
                continue;
            };
            // SAFETY: gate configuration explicitly identifies the user's trusted game DLL.
            let dll = unsafe { Oodle::load(path, version)? };
            let mut report = GateReport::new(var, Tolerance::Exact);
            for n in [0, 1, 65535, 65536, 65537, 131072, 262143, 262144, 262145, 524288] {
                let src: Vec<u8> = (0..n).map(|i| ((i * 17 + i / 256) % 251) as u8).collect();
                let compressed = dll.compress(&src)?;
                report.compare_bytes(format!("dll/{n}"), &dll.decompress(&compressed, n)?, &src);
                if n != 0 {
                    let mut other = vec![0; n];
                    let got = oozextract::Extractor::new().read_from_slice(&compressed, &mut other)?;
                    assert_eq!(got, n);
                    report.compare_bytes(format!("ooz/{n}"), &other, &src);
                }
                let cas = crate::cas::Cas { oodle: Some(&dll) };
                let framed = cas.compress(&src)?;
                report.compare_bytes(format!("cas/{n}"), &cas.decompress(&framed, n)?, &src);
            }
            if var == "PRIMASE_OODLE9"
                && let Some(folder) = real_data("PRIMASE_CAS_SAMPLES")
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
                            let mut out = vec![0; block.decoded_size];
                            oozextract::Extractor::new().read_from_slice(block.payload, &mut out)?;
                            out
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
}

#[cfg(test)]
mod dcx_gate {
    use super::*;
    use primase_core::gate::{GateReport, Tolerance, real_data};
    /// Test-only container adapter following SoulsFormats DCX.DecompressDCXKRAK.
    /// Production container parsing belongs in the future FromSoft world crate.
    #[test]
    fn loose_krak_dcx() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let Some(root) = real_data("PRIMASE_DCX") else {
            return Ok(());
        };
        let Some(path) = real_data("PRIMASE_OODLE6") else {
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
        let mut blocked = 0;
        let mut quantums = GateReport::new("independent KRAK quantums vs oozextract", Tolerance::Exact);
        let mut roundtrips =
            GateReport::new("DCX decoded bytes DLL compression roundtrips", Tolerance::Exact);
        for file in files {
            let b = std::fs::read(&file)?;
            if b.get(40..44) != Some(b"KRAK") {
                return Err(format!("unsupported DCX {}", file.display()).into());
            }
            let u32be = |o: usize| -> std::result::Result<u32, Box<dyn std::error::Error>> {
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
            let got = dll.decompress(src, expected)?;
            let mut other = vec![0; expected];
            match oozextract::Extractor::new().read_from_slice(src, &mut other) {
                Ok(_) => report.compare_bytes(file.display().to_string(), &got, &other),
                Err(e) => {
                    if expected <= 262144 {
                        return Err(e.into());
                    }
                    blocked += 1;
                    eprintln!(
                        "DLL only, whole-stream second decoder blocked by upstream restart defect: {}",
                        file.display()
                    );
                }
            }
            // Independent restarts permit an independent oracle for each quantum,
            // even while upstream mishandles the complete concatenated stream.
            let mut pos = 0;
            for (i, reference) in got.chunks(262144).enumerate() {
                let start = pos;
                if src.get(pos..pos + 2) != Some(&[0x8c, 0x06]) {
                    return Err("gate requires independent Kraken quantum, no checksum".into());
                }
                pos += 2;
                let header = src.get(pos..pos + 3).ok_or("short quantum header")?;
                let v =
                    (usize::from(header[0]) << 16) | (usize::from(header[1]) << 8) | usize::from(header[2]);
                pos += 3;
                let length = if v & 0x3ffff == 0x3ffff {
                    if v >> 18 != 1 {
                        return Err("unknown quantum special header".into());
                    }
                    1
                } else {
                    (v & 0x3ffff) + 1
                };
                pos += length;
                let quantum = src.get(start..pos).ok_or("short quantum payload")?;
                let mut decoded = vec![0; reference.len()];
                let written = oozextract::Extractor::new().read_from_slice(quantum, &mut decoded)?;
                assert_eq!(written, reference.len());
                quantums.compare_bytes(format!("{}/quantum/{i}", file.display()), reference, &decoded);
            }
            if pos != src.len() {
                return Err("trailing quantum bytes".into());
            }
            let encoded = dll.compress(&got)?;
            roundtrips.compare_bytes(
                file.display().to_string(),
                &got,
                &dll.decompress(&encoded, expected)?,
            );
        }
        println!(
            "{quantums}\n{roundtrips}\nDLL only, whole-stream second decoder blocked by upstream defect: {blocked}"
        );
        quantums.assert_passed();
        roundtrips.assert_passed();
        println!("{report}");
        report.assert_passed();
        Ok(())
    }
}

#[cfg(test)]
mod upstream_defect {
    use super::*;
    /// oozextract 0.5.5 fails at the second independent 256 KiB Kraken quantum:
    /// it keeps the global output base despite restart_decoder=true. Both quanta
    /// work individually and the native DLL decodes their concatenation exactly.
    /// A future oracle fix intentionally fails the final assertion so this known
    /// defect can be removed and the whole-stream gate made unconditional again.
    #[test]
    fn oozextract_055_kraken_restart_keeps_global_output_base()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let Some(path) = primase_core::gate::real_data("PRIMASE_OODLE6") else {
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
            let mut other = vec![0; part.len()];
            oozextract::Extractor::new().read_from_slice(&compressed, &mut other)?;
            assert_eq!(other, part);
            raw.extend(part);
            encoded.extend(compressed);
        }
        assert_eq!(dll.decompress(&encoded, raw.len())?, raw);
        let mut other = vec![0; raw.len()];
        assert!(
            oozextract::Extractor::new()
                .read_from_slice(&encoded, &mut other)
                .is_err(),
            "upstream defect fixed: remove known-defect handling and require whole-stream agreement"
        );
        Ok(())
    }
}
