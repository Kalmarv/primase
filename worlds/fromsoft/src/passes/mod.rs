//! The engine-wide passes' machinery: the repairs a geometry lowering may choose ([`geometry`]) and the static
//! layered-shader bake ([`layer_bake`]).
pub mod clip_events;
pub mod detail_derive;
pub mod detail_probe;
pub mod display_masks;
pub mod entity_names;
pub mod geometry;
pub mod layer_bake;
pub mod material_events;
pub mod preview;
pub mod probe_accept;
pub(crate) mod program_reads;
pub mod rigid;
pub(crate) mod sampler_pairs;
