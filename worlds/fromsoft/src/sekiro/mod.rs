//! Sekiro area reading: each placed model as its FLVER as stored, its materials as resolved source records, the
//! texture files and layered-shader programs they bind, the placed map collision files and object collision bodies as
//! stored, lights, hang edges and spawn points, with typed source identities ([`source`]). The read selects source
//! records (the region's placements, lights and spawn points; the parts' collision files) and converts frames; it
//! does not lower: geometry, materials, collision, lights and extras become core data through the passes in
//! [`passes`], [`collision`] and [`objects`], which a program chooses and orders. What the read leaves out or cannot
//! read is in its [`Report::notices`].
mod components;
pub mod passes;
mod read;
pub use components::source;
pub use components::source::Region;
pub use passes::collision;
pub use passes::extras;
pub use passes::object_collision as objects;
pub use passes::{blockers, layers, lights, normals, pbr, settings};
pub use read::animation::{AnimationReport, Animations};
pub use read::area::{Options, Parents, Report, StateClasses, read_area, read_game_area, world_bounds};
pub use read::character::{
    CharacterOptions, CharacterReader, CharacterReport, animation_notices, read_character,
    read_character_with, read_parts, read_parts_with,
};
pub use read::collision::Report as CollisionReport;
pub use read::extras::{Report as ExtrasReport, light_record};
pub use read::objects::Report as ObjectCollisionReport;
pub use read::{convert, states};

/// Data1 to Data5 with base-37 u32 path hashes and Sekiro's (Dark Souls III's) file records; the keys the community
/// unpackers publish.
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
    keys: crate::game::KeySource::Published(&crate::keys::SEKIRO),
};
