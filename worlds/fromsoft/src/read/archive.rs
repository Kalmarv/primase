#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: reads the install's archives"
)]
pub use crate::bhd::{PathHash, path_hash, path_hash64};
use crate::{
    Game, Result,
    bhd::{Entry, Header, unwrap_rsa},
    dcx::Decoder,
    game::{ArchivePair, KeySource},
    invalid,
};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

/// Caller-provided UTF-8 dictionary, one path per line; blank/comment lines ignored. Paths are keyed by a game's
/// path hash. Hash collisions retain every distinct normalized candidate, never overwrite.
#[derive(Debug, Default, Clone)]
pub struct NameDictionary {
    pub names: BTreeMap<u64, Vec<String>>,
}
impl NameDictionary {
    /// A dictionary of Sekiro paths.
    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        Self::read_for(path, Game::Sekiro)
    }
    /// A dictionary of `game`'s paths, keyed by its path hash.
    pub fn read_for(path: impl AsRef<Path>, game: Game) -> Result<Self> {
        Self::parse_for(&std::fs::read_to_string(path)?, game)
    }
    /// Sekiro paths from text.
    pub fn parse(text: &str) -> Result<Self> {
        Self::parse_for(text, Game::Sekiro)
    }
    pub fn parse_for(text: &str, game: Game) -> Result<Self> {
        let hash = game.archives().hash;
        let mut names: BTreeMap<u64, Vec<String>> = BTreeMap::new();
        for (i, line) in text.lines().enumerate() {
            let s = line.trim().trim_start_matches('\u{feff}');
            if s.is_empty() || s.starts_with('#') {
                continue;
            }
            if !s.is_ascii() {
                return Err(invalid("dictionary", i, "non-ASCII archive path unsupported"));
            }
            let mut s = s.replace('\\', "/").to_ascii_lowercase();
            if !s.starts_with('/') {
                s.insert(0, '/');
            }
            let list = names.entry(hash.hash(&s)).or_default();
            if !list.contains(&s) {
                list.push(s);
                list.sort();
            }
        }
        Ok(Self { names })
    }
}

/// The RSA public keys of a game's archive pairs (big-endian modulus and exponent), by pair stem.
#[derive(Clone)]
pub struct Keys(BTreeMap<&'static str, (Vec<u8>, Vec<u8>)>);
impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Keys")
            .field(&self.0.keys().collect::<Vec<_>>())
            .finish()
    }
}
impl Keys {
    /// The keys built in for a game whose keys the community unpackers publish (Sekiro); `None` for a game whose
    /// keys the caller supplies.
    pub fn published(game: Game) -> Option<Self> {
        let set = game.archives();
        let KeySource::Published(keys) = set.keys else {
            return None;
        };
        Some(Self(
            set.pairs
                .iter()
                .zip(keys)
                .map(|(pair, (n, e))| (pair.stem, (n.to_vec(), e.to_vec())))
                .collect(),
        ))
    }
    /// Keys the caller supplies: a PKCS#1 `RSA PUBLIC KEY` PEM file per archive pair in `folder`, named after the
    /// last component of the pair's stem (`Data0.pem`, `sd.pem`). A pair without a file has no key; opening an
    /// install that holds it then fails.
    pub fn from_folder(folder: impl AsRef<Path>, game: Game) -> Result<Self> {
        let folder = folder.as_ref();
        let mut keys = BTreeMap::new();
        for pair in game.archives().pairs {
            let file = folder.join(format!("{}.pem", pair.name()));
            if file.is_file() {
                let key = crate::keys::pem_public_key(&std::fs::read_to_string(&file)?)
                    .map_err(|e| invalid("PEM", 0, format!("{}: {e}", file.display())))?;
                keys.insert(pair.stem, key);
            }
        }
        Ok(Self(keys))
    }
    /// The key of the pair `stem`: its modulus and exponent.
    pub fn get(&self, stem: &str) -> Option<(&[u8], &[u8])> {
        self.0.get(stem).map(|(n, e)| (n.as_slice(), e.as_slice()))
    }
}

/// A game's install: its folder, the game and the keys of its archives; what a reader opens.
#[derive(Clone, Debug)]
pub struct Install {
    pub root: PathBuf,
    pub game: Game,
    pub keys: Keys,
}
impl Install {
    /// A Sekiro install, with the published keys.
    pub fn sekiro(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            game: Game::Sekiro,
            keys: Keys::published(Game::Sekiro).expect("Sekiro's keys are published"),
        }
    }
    pub fn new(root: impl Into<PathBuf>, game: Game, keys: Keys) -> Self {
        Self {
            root: root.into(),
            game,
            keys,
        }
    }
    /// Its archives.
    pub fn open(&self) -> Result<Archive> {
        Archive::open_game(&self.root, self.game, &self.keys)
    }
}

#[derive(Debug)]
pub struct IndexedEntry {
    pub shard: u8,
    pub entry: Entry,
}
/// A game's immutable archive indexes. BDT files are opened for each read, so reads need no shared seek cursor. Names
/// and Oodle loading remain caller-controlled.
#[derive(Debug)]
pub struct Archive {
    root: PathBuf,
    game: Game,
    pub entries: BTreeMap<u64, IndexedEntry>,
    /// The indexes of the pairs read, in the game's pair order.
    pub headers: Vec<Header>,
    /// Optional pairs the install does not hold (downloadable content not installed).
    pub absent: Vec<&'static str>,
}
impl Archive {
    /// Sekiro's archives, with the published keys.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let keys = Keys::published(Game::Sekiro).expect("Sekiro's keys are published");
        Self::open_game(root, Game::Sekiro, &keys)
    }
    /// `game`'s archives under `root`, each index unwrapped with its pair's key in `keys`. An entry hash present in
    /// two pairs is an error.
    pub fn open_game(root: impl AsRef<Path>, game: Game, keys: &Keys) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let set = game.archives();
        let mut entries = BTreeMap::new();
        let mut headers = Vec::new();
        let mut absent = Vec::new();
        for pair in set.pairs {
            let bhd = root.join(format!("{}.bhd", pair.stem));
            if !pair.required && !bhd.exists() {
                absent.push(pair.stem);
                continue;
            }
            let (n, e) = keys
                .0
                .get(pair.stem)
                .ok_or_else(|| invalid("RSA", 0, format!("no key for {}", pair.stem)))?;
            let plain = unwrap_rsa(&std::fs::read(&bhd)?, n, e)?;
            let header = Header::read(&plain, &set.record)?;
            let size = std::fs::metadata(root.join(format!("{}.bdt", pair.stem)))?.len();
            for entry in &header.entries {
                if entry
                    .offset
                    .checked_add(entry.padded_size as u64)
                    .is_none_or(|end| end > size)
                {
                    return Err(invalid("BDT", entry.offset as usize, "entry beyond file"));
                }
                if entries
                    .insert(
                        entry.hash,
                        IndexedEntry {
                            shard: pair.shard,
                            entry: entry.clone(),
                        },
                    )
                    .is_some()
                {
                    return Err(invalid("BHD5", 0, "cross-archive hash collision"));
                }
            }
            headers.push(header);
        }
        Ok(Self {
            root,
            game,
            entries,
            headers,
            absent,
        })
    }
    pub fn game(&self) -> Game {
        self.game
    }
    /// The archive pair whose entries report `shard`.
    pub fn pair(&self, shard: u8) -> Option<&'static ArchivePair> {
        self.game.archives().pairs.iter().find(|p| p.shard == shard)
    }
    /// The game's hash of an archive path.
    pub fn hash(&self, path: &str) -> u64 {
        self.game.archives().hash.hash(path)
    }
    /// Whether the index lists `path`.
    pub fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(&self.hash(path))
    }
    /// Read decrypted stored bytes, including BDT padding. Metadata logical_size
    /// lets consumers distinguish padding without losing reference-reader parity.
    pub fn read_stored(&self, hash: u64) -> Result<Vec<u8>> {
        let e = self.entries.get(&hash).ok_or(crate::Error::Missing(hash))?;
        let pair = self
            .pair(e.shard)
            .ok_or_else(|| invalid("BHD5", 0, "entry from an unknown archive"))?;
        let mut f = std::fs::File::open(self.root.join(format!("{}.bdt", pair.stem)))?;
        f.seek(SeekFrom::Start(e.entry.offset))?;
        let mut out = vec![0; e.entry.padded_size];
        f.read_exact(&mut out)?;
        e.entry.decrypt(&mut out)?;
        Ok(out)
    }
    /// Decode a DCX envelope when present; non-DCX bytes retain their stored padding.
    pub fn read_hash(&self, hash: u64, decoder: &Decoder<'_>) -> Result<Vec<u8>> {
        let bytes = self.read_stored(hash)?;
        decoder.decode_if_dcx(&bytes)
    }
    pub fn read(&self, path: &str, decoder: &Decoder<'_>) -> Result<Vec<u8>> {
        self.read_hash(self.hash(path), decoder)
    }
}
/// Public key parameters for one Sekiro shard (1 through 5).
pub fn public_key(shard: u8) -> Result<(&'static [u8], &'static [u8])> {
    crate::keys::SEKIRO
        .get(usize::from(shard).wrapping_sub(1))
        .copied()
        .ok_or_else(|| invalid("RSA", 0, "shard must be 1..5"))
}
