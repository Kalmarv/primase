//! Frostbite containers as skate. (ReSkate) reads and writes them: superbundle TOCs, bundle regions and manifests,
//! the native DB, data folders and CAS archives, shader block depots, the shader lookup tables of
//! `dingolevel_root`, and EBX documents.
//!
//! It covers the Frostbite generation skate. uses (its build of the containers and their versions, as Studio and
//! ReSkate write them); other Frostbite titles are not assumed to match.
//!
//! Every reader is strict (it walks to the exact end of what it reads and rejects anything it does not know) and
//! every writer is the counterpart of the reader: an unmodified Patch read and written again is byte-identical.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod binding;
pub mod bundle;
pub mod bytecode;
pub mod cas;
pub mod components;
pub mod db;
pub mod depot;
pub mod ebx;
pub mod fbmod;
pub mod geometry;
pub mod inputlayout;
pub mod lookup;
pub mod meshset;
pub mod passes;
pub mod permutation;
mod read;
pub mod selector;
pub mod texture;
pub mod toc;

pub use read::cas_codec::Cas;
pub use read::{entry, folder, listing, program};

/// Resource type of a MeshSet.
pub const MESHSET: u32 = 0x49B1_56D4;
/// Resource type of a texture.
pub const TEXTURE: u32 = 0x6BDE_20BA;

/// Errors of the post-compile readers and writers: what broke and where.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{what} at {offset:#x}: {detail}")]
    Format {
        what: &'static str,
        offset: usize,
        detail: String,
    },
    #[error("{}: {source}", path.display())]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("cas: {0}")]
    Cas(#[from] primase_compress::Error),
    #[error("{0}")]
    Missing(String),
    /// An entry of a kind or form no reader here reads, named.
    #[error("{0}")]
    Unsupported(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Lowercase hexadecimal of `bytes`, in the order given.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// A format error: what was being read, where, and what broke.
pub fn bad(what: &'static str, offset: usize, detail: impl Into<String>) -> Error {
    Error::Format {
        what,
        offset,
        detail: detail.into(),
    }
}

/// Bounds-checked fixed-size reads, little and big endian.
#[derive(Debug)]
pub struct Rd<'a> {
    pub data: &'a [u8],
    pub what: &'static str,
}

impl<'a> Rd<'a> {
    pub fn new(data: &'a [u8], what: &'static str) -> Self {
        Self { data, what }
    }

    pub fn bytes(&self, at: usize, n: usize) -> Result<&'a [u8]> {
        primase_bytes::View::new(self.data).slice(at, n).map_err(|_| {
            bad(
                self.what,
                at,
                format!("{n} bytes past the end ({} bytes)", self.data.len()),
            )
        })
    }

    pub fn arr<const N: usize>(&self, at: usize) -> Result<[u8; N]> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.bytes(at, N)?);
        Ok(a)
    }

    pub fn u8(&self, at: usize) -> Result<u8> {
        Ok(self.arr::<1>(at)?[0])
    }
    pub fn u16le(&self, at: usize) -> Result<u16> {
        Ok(u16::from_le_bytes(self.arr(at)?))
    }
    pub fn u32le(&self, at: usize) -> Result<u32> {
        Ok(u32::from_le_bytes(self.arr(at)?))
    }
    pub fn u64le(&self, at: usize) -> Result<u64> {
        Ok(u64::from_le_bytes(self.arr(at)?))
    }
    pub fn u32be(&self, at: usize) -> Result<u32> {
        Ok(u32::from_be_bytes(self.arr(at)?))
    }
    pub fn i32be(&self, at: usize) -> Result<i32> {
        Ok(i32::from_be_bytes(self.arr(at)?))
    }
    pub fn u64be(&self, at: usize) -> Result<u64> {
        Ok(u64::from_be_bytes(self.arr(at)?))
    }
    /// A zero-terminated Latin-1 string starting at `at`.
    pub fn cstr(&self, at: usize) -> Result<String> {
        let rest = self
            .data
            .get(at..)
            .ok_or_else(|| bad(self.what, at, "string past the end"))?;
        let end = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| bad(self.what, at, "unterminated string"))?;
        Ok(rest[..end].iter().map(|&b| char::from(b)).collect())
    }
}

/// Latin-1 bytes of a name (Frostbite names are stored one byte per character).
pub fn latin1(s: &str, what: &'static str) -> Result<Vec<u8>> {
    s.chars()
        .map(|c| u8::try_from(u32::from(c)).map_err(|_| bad(what, 0, format!("name {s:?} is not Latin-1"))))
        .collect()
}

/// Capacity for `count` entries that each take at least `min_bytes` of the `remaining` input bytes: refused before
/// anything is reserved when they cannot fit, so a corrupt count returns an error instead of exhausting memory.
pub fn capacity(what: &'static str, count: usize, min_bytes: usize, remaining: usize) -> Result<usize> {
    match primase_bytes::checked_span(0, count, min_bytes, remaining) {
        Ok(_) => Ok(count),
        _ => Err(bad(
            what,
            0,
            format!("{count} entries of at least {min_bytes} bytes in {remaining} bytes"),
        )),
    }
}

/// `v` rounded up to a multiple of `a`.
pub fn align(v: usize, a: usize) -> usize {
    v.div_ceil(a) * a
}

#[cfg(test)]
mod bounds_tests {
    use super::*;
    /// The fields of a format error, or a panic for any other kind: the constructors must choose both the kind and
    /// the offset, not only the message text.
    fn format_error(error: Error) -> (&'static str, usize, String) {
        match error {
            Error::Format { what, offset, detail } => (what, offset, detail),
            other => panic!("expected a format error, got {other}"),
        }
    }
    #[test]
    fn wrapper_retains_bounds_and_error_context() {
        let rd = Rd::new(&[1, 2, 3, 4], "bounds fixture");
        assert!(rd.bytes(4, 0).unwrap().is_empty());
        assert_eq!(rd.u32be(0).unwrap(), 0x01020304);
        for (offset, length) in [(4, 1), (5, 0), (usize::MAX, 1)] {
            let (what, at, detail) = format_error(rd.bytes(offset, length).unwrap_err());
            assert_eq!((what, at), ("bounds fixture", offset));
            assert_eq!(detail, format!("{length} bytes past the end (4 bytes)"));
        }
        assert_eq!(capacity("table", 2, 2, 4).unwrap(), 2);
        assert_eq!(capacity("table", usize::MAX, 0, 0).unwrap(), usize::MAX);
        for (count, stride) in [(3, 2), (usize::MAX, 2)] {
            let (what, at, detail) = format_error(capacity("table", count, stride, 4).unwrap_err());
            assert_eq!((what, at), ("table", 0));
            assert_eq!(
                detail,
                format!("{count} entries of at least {stride} bytes in 4 bytes")
            );
        }
    }
}
