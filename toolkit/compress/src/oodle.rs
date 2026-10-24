//! Oodle, from a caller-selected, trusted game library (no discovery) where the game ships one for the platform
//! (Windows), else from oozextract: [`Oodle::open`] picks. The game's library stays the reference; the gates hold
//! oozextract to it (`reference`).
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
struct Library {
    _library: libloading::Library,
    decode: Decode,
    encode: Encode,
    capacity: Capacity,
}
/// An Oodle codec: the game's library, or off Windows oozextract (which decodes and cannot compress).
#[derive(Debug)]
pub struct Oodle {
    backend: Backend,
}
#[derive(Debug)]
enum Backend {
    Library(Library),
    #[cfg(not(windows))]
    Portable,
}
impl Oodle {
    /// The codec for this platform: the game's library `path` where one is given, else (off Windows) oozextract.
    /// Windows has no built-in decoder, so no path is an error there.
    ///
    /// # Safety
    /// As [`Oodle::load`] when `path` is given.
    pub unsafe fn open(path: Option<&Path>, version: Version) -> Result<Self> {
        match path {
            // SAFETY: the caller's contract is `load`'s.
            Some(path) => unsafe { Self::load(path, version) },
            #[cfg(not(windows))]
            None => Ok(Self::portable()),
            #[cfg(windows)]
            None => Err(invalid(
                "oodle",
                0,
                "no game library given, and Windows has no built-in decoder",
            )),
        }
    }
    /// oozextract as the codec: decodes every format it supports (Kraken, Mermaid, Selkie, Leviathan) and refuses a
    /// stream it cannot decode, naming why; it cannot compress.
    #[cfg(not(windows))]
    pub fn portable() -> Self {
        Self {
            backend: Backend::Portable,
        }
    }
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
                backend: Backend::Library(Library {
                    _library: library,
                    decode,
                    encode,
                    capacity,
                }),
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
        let library = match &self.backend {
            Backend::Library(library) => library,
            #[cfg(not(windows))]
            Backend::Portable => {
                return crate::reference::decode(input, expected).map_err(|e| invalid("oodle", 0, e));
            }
        };
        let src_len = isize::try_from(input.len()).map_err(|_| crate::Error::Limit)?;
        let dst_len = isize::try_from(expected).map_err(|_| crate::Error::Limit)?;
        let mut out = buffer(expected)?;
        // SAFETY: slices are live, non-overlapping and sized as passed; fuzz-safe
        // decode enabled, no callbacks or external dictionary, synchronous phases.
        let got = unsafe {
            (library.decode)(
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
/// The Oodle compressors this crate asks the game's library for, by the library's compressor number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// `OodleLZ_Compressor_LZNIB`: the older byte-aligned codec of decoder type 2.
    Lznib = 2,
    /// `OodleLZ_Compressor_Kraken`: what [`Compressor::compress`] uses.
    Kraken = 8,
}

impl Oodle {
    /// Compress with `codec` at level 4, through the game's library: the portable decoder has no encoder, and the
    /// gates need streams of every codec the decoders read.
    pub fn compress_as(&self, codec: Codec, input: &[u8]) -> Result<Vec<u8>> {
        if input.is_empty() {
            return Ok(Vec::new());
        }
        let library = match &self.backend {
            Backend::Library(library) => library,
            #[cfg(not(windows))]
            Backend::Portable => {
                return Err(invalid(
                    "oodle",
                    0,
                    "compressing needs the game's library; oozextract only decodes",
                ));
            }
        };
        let compressor = codec as i32;
        let n = isize::try_from(input.len()).map_err(|_| crate::Error::Limit)?;
        // SAFETY: pure size query with the codec's enum and validated positive length.
        let cap = unsafe {
            match library.capacity {
                Capacity::Legacy(f) => f(n),
                Capacity::Modern(f) => f(compressor, n),
            }
        };
        if cap < n {
            return Err(invalid("oodle", 0, "invalid compressed capacity"));
        }
        let mut out = buffer(cap as usize)?;
        // SAFETY: output has the DLL-reported capacity; input is live and disjoint;
        // default options, no dictionary, long-range matcher or external scratch.
        let got = unsafe {
            (library.encode)(
                compressor,
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
impl Compressor for Oodle {
    fn compress(&self, input: &[u8]) -> Result<Vec<u8>> {
        self.compress_as(Codec::Kraken, input)
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

    /// Streams the game's library compresses with LZNIB (decoder type 2), from inputs of the shapes game files have (UTF-16
    /// names, long zero runs, repeated structures, incompressible bytes) at sizes around the block and header spans:
    /// oozextract must give the input back, as the library does. A stream either decoder gets wrong is kept under the
    /// debug key `oodle.dump`'s folder, and every failure is reported, not the first.
    #[test]
    fn lznib_cross_check() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let Some(path) = tool(Tool::Oodle6) else {
            return Ok(());
        };
        // SAFETY: gate configuration explicitly identifies the user's trusted game DLL.
        let dll = unsafe { Oodle::load(path, Version::V6)? };
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let words = [
            "Parts",
            "Group",
            "model",
            "FAR_MODEL_PARAM_ST",
            "N:\\NTC\\data\\Target",
            "c0000",
            "o110000",
        ];
        let mut failures = Vec::new();
        let dump = primase_testdata::debug("oodle.dump").map(|name| primase_testdata::scratch().join(name));
        for n in [
            1, 9, 100, 5000, 16383, 16384, 16385, 40000, 65535, 65536, 65537, 262143, 262144, 262145, 600000,
        ] {
            for shape in ["utf16-names", "zeros-and-structs", "random", "long-runs"] {
                let mut src = Vec::with_capacity(n);
                while src.len() < n {
                    let r = next();
                    match shape {
                        "utf16-names" => {
                            for c in words[(r % 7) as usize].bytes() {
                                src.extend_from_slice(&[c, 0]);
                            }
                            src.extend_from_slice(&[0, 0]);
                        }
                        "zeros-and-structs" => {
                            src.extend((0..(r % 40) as usize).map(|_| 0));
                            src.extend_from_slice(&(r as u32).to_le_bytes());
                        }
                        "random" => src.extend_from_slice(&r.to_le_bytes()),
                        _ => src.extend((0..(r % 3000) as usize).map(|_| (r >> 40) as u8)),
                    }
                }
                src.truncate(n);
                let compressed = dll.compress_as(Codec::Lznib, &src)?;
                let item = format!("{shape}/{n}");
                let from_dll = dll.decompress(&compressed, n);
                let from_ooz = crate::reference::decode(&compressed, n);
                let problem = match (&from_dll, &from_ooz) {
                    (Ok(a), _) if *a != src => Some("the library does not give the input back".to_owned()),
                    (Err(e), _) => Some(format!("the library: {e}")),
                    (_, Ok(b)) if *b != src => Some("oozextract decodes other bytes".to_owned()),
                    (_, Err(e)) => Some(e.to_string().lines().next().unwrap_or_default().to_owned()),
                    _ => None,
                };
                if let Some(problem) = problem {
                    if let Some(dir) = &dump {
                        std::fs::create_dir_all(dir)?;
                        let stem = item.replace('/', "-");
                        std::fs::write(dir.join(format!("lznib-{stem}.compressed")), &compressed)?;
                        std::fs::write(dir.join(format!("lznib-{stem}.source")), &src)?;
                    }
                    failures.push(format!(
                        "{item} ({} compressed bytes): {problem}",
                        compressed.len()
                    ));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "LZNIB streams oozextract does not read as the library does:\n{}",
            failures.join("\n")
        );
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
