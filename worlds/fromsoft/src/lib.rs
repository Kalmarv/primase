//! FromSoftware (Sekiro, Elden Ring) data in primase: the area and character readers that build a World from what
//! `fromsoft-formats` reads, the components they make and the passes that lower them.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod components;
pub mod game;
mod light_records;
pub mod materials;
mod passes;
mod pose;
pub mod sekiro;
pub use components::clip_source;
pub use components::stored;
pub use passes::{
    clip_events, detail_derive, detail_probe, display_masks, entity_names, geometry, layer_bake,
    material_events, preview, probe_accept, rigid,
};

/// A refusal of this crate's own (a record that does not fit what a pass needs), as the format readers make theirs.
pub(crate) fn invalid(format: &'static str, offset: usize, detail: impl ToString) -> fromsoft_formats::Error {
    fromsoft_formats::Error::invalid(format, offset, detail)
}

/// The real-data inputs this crate's unit tests read (tests/inputs/mod.rs).
#[cfg(test)]
#[path = "../tests/inputs/mod.rs"]
mod inputs;
