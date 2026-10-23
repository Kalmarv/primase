//! Elden Ring's own rules: what differs from the other FromSoftware games and is not stated by the files themselves.
use crate::bhd::{PathHash, RecordLayout};
use crate::game::{ArchivePair, ArchiveSet, KeySource};

/// Data0 to Data3, the downloadable content's DLC and the sound archives `sd/sd` and `sd/sd_dlc02`, as the community
/// unpacker (UXM's `EldenRingGameInfo.xml`) lists them; base-133 u64 path hashes; keys supplied by the caller.
pub(crate) const ARCHIVES: ArchiveSet = ArchiveSet {
    pairs: &[
        ArchivePair::required(0, "Data0"),
        ArchivePair::required(1, "Data1"),
        ArchivePair::required(2, "Data2"),
        ArchivePair::required(3, "Data3"),
        ArchivePair::optional(4, "DLC"),
        ArchivePair::required(5, "sd/sd"),
        ArchivePair::optional(6, "sd/sd_dlc02"),
    ],
    hash: PathHash::Base133,
    record: RecordLayout::ELDEN_RING,
    keys: KeySource::Supplied,
};
