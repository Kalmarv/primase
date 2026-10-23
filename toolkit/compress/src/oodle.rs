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
/// level 4.
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

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "test I/O: real-data gates"
)]
mod tests {
    use super::*;
    use primase_core::gate::{GateReport, Tolerance};
    use primase_testdata::{Tool, tool};
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
                    report.compare_bytes(
                        format!("ooz/{n}"),
                        &crate::reference::decode(&compressed, n)?,
                        &src,
                    );
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
            assert_eq!(crate::reference::decode(&compressed, part.len())?, part);
            raw.extend(part);
            encoded.extend(compressed);
        }
        assert_eq!(dll.decompress(&encoded, raw.len())?, raw);
        assert_eq!(crate::reference::decode(&encoded, raw.len())?, raw);
        Ok(())
    }
}
