//! Steps that run after ReSkate Studio's compile, on the native Patch folder it writes: the Frostbite containers
//! they read and write (`fb`) and the MeshSet resource (`meshset`).
//!
//! Every reader is strict (it walks to the exact end of what it reads and rejects anything it does not know) and
//! every writer is the counterpart of the reader: an unmodified Patch read and written again is byte-identical.
//!
//! The Frostbite code lives here until a second game needs it; it then moves to a toolkit crate.

pub mod fb;
pub mod meshset;

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
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub(crate) fn bad(what: &'static str, offset: usize, detail: impl Into<String>) -> Error {
    Error::Format {
        what,
        offset,
        detail: detail.into(),
    }
}

/// Bounds-checked fixed-size reads, little and big endian.
pub(crate) struct Rd<'a> {
    pub data: &'a [u8],
    pub what: &'static str,
}

impl<'a> Rd<'a> {
    pub fn new(data: &'a [u8], what: &'static str) -> Self {
        Self { data, what }
    }

    pub fn bytes(&self, at: usize, n: usize) -> Result<&'a [u8]> {
        at.checked_add(n)
            .and_then(|end| self.data.get(at..end))
            .ok_or_else(|| {
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
pub(crate) fn latin1(s: &str, what: &'static str) -> Result<Vec<u8>> {
    s.chars()
        .map(|c| u8::try_from(u32::from(c)).map_err(|_| bad(what, 0, format!("name {s:?} is not Latin-1"))))
        .collect()
}

pub(crate) fn align(v: usize, a: usize) -> usize {
    v.div_ceil(a) * a
}
