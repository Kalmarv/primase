//! The Sekiro reader: the area read ([`area`]), one character by itself ([`character`]), and the parts of it with modules of their own (texture catalog, world
//! states, shadow flags, the sky), and the program-facing reader adapter ([`convert`]).
pub mod animation;
pub mod area;
pub mod character;
pub mod clip_events;
pub mod collision;
pub mod convert;
pub mod draw_param;
pub mod effect_lights;
pub mod effects;
pub mod equipment;
pub mod extras;
pub mod groups;
pub mod irradiance;
pub mod material_events;
pub mod messages;
pub mod objects;
pub mod render_graph;
pub(super) mod shaders;
pub(super) mod shadows;
pub mod sky;
pub mod states;
pub mod texture_switch;
pub(super) mod textures;
pub mod visibility;
pub mod weapons;

pub mod world_state;
pub mod weather;
