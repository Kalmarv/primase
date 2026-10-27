//! Sekiro's own rules: what differs from the other FromSoftware games and is not stated by the files themselves.

/// Sekiro's params: the binder `param/gameparam/gameparam.parambnd.dcx` of its archives.
pub(crate) const PARAMS: crate::game::ParamSource =
    crate::game::ParamSource::Archived("/param/gameparam/gameparam.parambnd.dcx");

/// Sekiro's NPC param, `NPC_PARAM_ST` data version 14 with rows of 896 bytes: `modelDispMask0..15` in bytes 334 and
/// 335, `16..31` in bytes 338 and 339 (Paramdex's `SDT/Defs/NpcParam.xml`, FormatVersion 202; the `npc_outfits` test
/// holds every row's bits to SoulsFormats' reading of the install's file with that definition).
pub(crate) const NPC_PARAM: crate::param::NpcLayout = crate::param::NpcLayout {
    data_version: 14,
    row_size: 896,
    mask_bytes: [334, 335, 338, 339],
};

/// Where Sekiro's NPC param keeps the fields [`crate::param::NpcStats`] reads: four bytes each from the start of the row,
/// `behaviorVariationId` at 0, then after `deadMoveForceRate` (4) `nameId`, `turnVellocity`, `hitHeight`, `hitRadius`,
/// `weight`, `hitYOffset`, `hp`, `mp` and `getSoul` (Paramdex's `SDT/Defs/NpcParam.xml`, data version 14).
pub(crate) const NPC_STATS: crate::param::NpcStatLayout = crate::param::NpcStatLayout {
    data_version: 14,
    row_size: 896,
    offsets: [0, 8, 12, 16, 20, 24, 28, 32, 36, 40],
};

/// Data1 to Data5 with base-37 u32 path hashes and Sekiro's (Dark Souls III's) file records.
pub(crate) const ARCHIVES: crate::game::ArchiveSet = crate::game::ArchiveSet {
    pairs: &[
        crate::game::ArchivePair::required(1, "Data1"),
        crate::game::ArchivePair::required(2, "Data2"),
        crate::game::ArchivePair::required(3, "Data3"),
        crate::game::ArchivePair::required(4, "Data4"),
        crate::game::ArchivePair::required(5, "Data5"),
    ],
    hash: crate::bhd::PathHash::Base37,
    record: crate::bhd::RecordLayout::SEKIRO,
};
