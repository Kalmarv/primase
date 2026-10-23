//! The FromSoftware games this crate reads, and what differs between them that their files do not say about
//! themselves. Formats are read by what their bytes state (a map layout's section chain, a FLVER's version); a game's
//! own rules (its archive set, where it keeps its files) are data in that game's module (`sekiro`, `elden`), read
//! through [`Game`] by the shared readers.
use crate::bhd::{PathHash, RecordLayout};

/// A FromSoftware game.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Game {
    Sekiro,
    EldenRing,
}

impl Game {
    pub const ALL: [Game; 2] = [Game::Sekiro, Game::EldenRing];

    pub fn name(self) -> &'static str {
        match self {
            Game::Sekiro => "Sekiro",
            Game::EldenRing => "Elden Ring",
        }
    }

    /// The game's archive set.
    pub fn archives(self) -> &'static ArchiveSet {
        match self {
            Game::Sekiro => &crate::sekiro::ARCHIVES,
            Game::EldenRing => &crate::elden::ARCHIVES,
        }
    }
}

/// One BHD5/BDT pair of an install.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArchivePair {
    /// The number entries of this pair report as their shard.
    pub shard: u8,
    /// The pair's path below the install folder without its `.bhd`/`.bdt` extension (`Data1`, `sd/sd`).
    pub stem: &'static str,
    /// Whether every install holds it; an optional pair (downloadable content) that is absent is reported, not an
    /// error.
    pub required: bool,
}

impl ArchivePair {
    /// A pair every install holds.
    pub const fn required(shard: u8, stem: &'static str) -> Self {
        Self {
            shard,
            stem,
            required: true,
        }
    }
    /// A pair an install may lack.
    pub const fn optional(shard: u8, stem: &'static str) -> Self {
        Self {
            shard,
            stem,
            required: false,
        }
    }
    /// The last component of the stem (`sd` for `sd/sd`): the name its key file takes.
    pub fn name(&self) -> &'static str {
        self.stem.rsplit('/').next().unwrap_or(self.stem)
    }
}

/// Where a game's archive keys come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySource {
    /// Published by the community unpackers and built in, one (modulus, exponent) per pair in the set's order.
    Published(&'static [(&'static [u8], &'static [u8])]),
    /// Not distributed with primase: the caller supplies them.
    Supplied,
}

/// A game's archives: its BHD5/BDT pairs in index order, how its paths hash, how its file records are laid out and
/// where its keys come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArchiveSet {
    pub pairs: &'static [ArchivePair],
    pub hash: PathHash,
    pub record: RecordLayout,
    pub keys: KeySource,
}
