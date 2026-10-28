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
pub use components::source::Region;
pub use components::{character as character_data, effect as effect_data, lowered, source};
pub use passes::collision;
pub use passes::extras;
pub use passes::object_collision as objects;
pub use passes::{blockers, effects, layers, lights, normals, pbr, settings};
pub use read::animation::{AnimationReport, Animations, ClipSource, read_clip_skeleton};
pub use read::area::{
    GrassTypes, ObjectAnimations, Options, Parents, Report, StateClasses, read_area_draw_param,
    read_game_area, read_game_area_staged, world_bounds,
};
pub use read::character::{
    CharacterOptions, CharacterReader, CharacterReport, animation_notices, character_display_masks,
    part_forms, part_textures, read_character_with, read_parts_with,
};
pub use read::clip_events::decode as decode_clip_events;
pub use read::collision::Report as CollisionReport;
pub use read::draw_param::{DrawParams, Report as DrawParamReport};
pub use read::effect_lights::Report as EffectLightsReport;
pub use read::effects::{EffectLibrary, Report as EffectsReport, binders_for_area, read_effect};
pub use read::equipment::{character_skeletons, empty_character_reason, outfit_notices};
pub use read::extras::{Report as ExtrasReport, light_record, records as read_map_records};
pub use read::messages::npc_names_into_world;
pub use read::objects::Report as ObjectCollisionReport;
pub use read::render_graph::Report as RenderGraphReport;
pub use read::textures::tpf_image;
pub use read::visibility::decode as decode_visibility;
pub use read::{convert, sky, states};
