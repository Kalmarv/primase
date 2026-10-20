//! A content-addressed disk cache for heavy, pure steps (texture encoding, shader bakes): the key is a hash of
//! everything the output depends on, the value is the output's bytes. Opt-in: a caller opens a [`Cache`] only on a
//! folder it names (the program's choice); the library has no default
//! location and reads no environment. Only [`Cache`] (module `disk`) touches files: the steps it serves are handed
//! one by their caller.
//!
//! - **Keys** ([`KeyBuilder`]): SHA-256 over a domain name and tagged, length-prefixed fields, so two different inputs
//!   cannot meet by concatenation, plus the operation's versions: an explicit version, a hash of the source code that
//!   computes it ([`source_hash`]), and the versions of external encoders. Keys hash canonical input bytes only,
//!   never `Debug` text.
//! - **Entries** are files `<dir>/v1/<first two hex digits>/<key hex>`: a header (magic, format version, the full key,
//!   payload length, payload SHA-256) and the payload. Every read verifies all of it; an entry that fails is counted
//!   as corrupt, removed, and its value computed again. A cache never returns bytes it cannot vouch for.
//! - **Concurrency**: several processes may share a directory. An entry is written to a unique temporary file in its
//!   folder and renamed into place, so a reader sees a whole entry or none. Producers are deterministic, so two
//!   writers of one key write the same bytes; losing the rename race is not an error. Entries vanishing under a
//!   reader (another process trimming) are misses.
//! - **Size**: entries are evicted least recently used first (a hit refreshes the file's modified time) when
//!   [`Cache::trim`] runs: at the end of a run, and by the cache itself after a tenth of the cap has been written.
//!   Under concurrent trims a little more than needed may go; that only costs recomputation.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

use sha2::{Digest, Sha256};

mod disk;
pub use disk::Cache;

pub(crate) const MAGIC: [u8; 4] = *b"PRMC";
pub(crate) const FORMAT: u32 = 1;
pub(crate) const HEADER: usize = 4 + 4 + 32 + 8 + 32;

/// What a cache entry is the output of.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key(pub(crate) [u8; 32]);

impl Key {
    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Key({})", self.hex())
    }
}

/// Builds a [`Key`]: a domain (`"web.ktx2-uastc"`), then fields, each a tag and a value, in a fixed order.
#[derive(Clone)]
pub struct KeyBuilder(Sha256);

impl std::fmt::Debug for KeyBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KeyBuilder")
    }
}

impl KeyBuilder {
    pub fn new(domain: &str) -> Self {
        KeyBuilder(Sha256::new()).bytes("primase-cache key 1", domain.as_bytes())
    }

    /// A field: its tag and value, each length-prefixed.
    pub fn bytes(mut self, tag: &str, value: &[u8]) -> Self {
        self.0.update((tag.len() as u64).to_le_bytes());
        self.0.update(tag.as_bytes());
        self.0.update((value.len() as u64).to_le_bytes());
        self.0.update(value);
        self
    }

    pub fn str(self, tag: &str, value: &str) -> Self {
        self.bytes(tag, value.as_bytes())
    }

    pub fn u32(self, tag: &str, value: u32) -> Self {
        self.bytes(tag, &value.to_le_bytes())
    }

    pub fn u64(self, tag: &str, value: u64) -> Self {
        self.bytes(tag, &value.to_le_bytes())
    }

    /// By bit pattern: `0.0` and `-0.0` are different keys.
    pub fn f32(self, tag: &str, value: f32) -> Self {
        self.u32(tag, value.to_bits())
    }

    pub fn bool(self, tag: &str, value: bool) -> Self {
        self.bytes(tag, &[u8::from(value)])
    }

    pub fn finish(self) -> Key {
        Key(self.0.finalize().into())
    }
}

/// SHA-256 over source files, for keys: each file's name and bytes, length-prefixed, in the order given. Pass the
/// files that compute a cached value (`include_bytes!` them where they are compiled), so editing them retires the
/// entries they made without a version bump to forget.
pub fn source_hash(files: &[(&str, &[u8])]) -> [u8; 32] {
    let mut b = KeyBuilder::new("source");
    for (name, bytes) in files {
        b = b.bytes(name, bytes);
    }
    b.finish().0
}

/// The size cap when none is given: 10 GiB.
pub const DEFAULT_MAX_BYTES: u64 = 10 << 30;

/// What a cache did in this process.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub hits: u64,
    pub misses: u64,
    /// Entries that failed verification (removed and recomputed).
    pub corrupt: u64,
    pub written: u64,
    pub written_bytes: u64,
    /// Entries that could not be written (the value was still returned).
    pub write_errors: u64,
    pub evicted: u64,
    pub evicted_bytes: u64,
}

/// Whether `entry` is a whole, uncorrupted entry for `key`.
pub(crate) fn verify(key: &Key, entry: &[u8]) -> Option<()> {
    let header = entry.get(..HEADER)?;
    let payload = &entry[HEADER..];
    (header[..4] == MAGIC
        && header[4..8] == FORMAT.to_le_bytes()
        && header[8..40] == key.0
        && header[40..48] == (payload.len() as u64).to_le_bytes()
        && header[48..80] == Sha256::digest(payload)[..])
        .then_some(())
}

/// An entry's bytes: the header for `key` and `value`, then `value`.
pub(crate) fn entry(key: &Key, value: &[u8]) -> Vec<u8> {
    let mut entry = Vec::with_capacity(HEADER + value.len());
    entry.extend_from_slice(&MAGIC);
    entry.extend_from_slice(&FORMAT.to_le_bytes());
    entry.extend_from_slice(&key.0);
    entry.extend_from_slice(&(value.len() as u64).to_le_bytes());
    entry.extend_from_slice(&Sha256::digest(value));
    entry.extend_from_slice(value);
    entry
}
