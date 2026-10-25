//! The engine-wide passes' machinery: the repairs a geometry lowering may choose ([`geometry`]) and the static
//! layered-shader bake ([`layer_bake`]).
pub mod detail_derive;
pub mod detail_probe;
pub mod clip_events;
pub mod entity_names;
pub mod display_masks;
pub mod material_events;
pub mod geometry;
pub mod layer_bake;
pub mod preview;
pub mod probe_accept;
pub(crate) mod program_reads;
pub mod rigid;
