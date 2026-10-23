//! The Sekiro reader: the area read ([`area`]), one character by itself ([`character`]), and the parts of it with modules of their own (texture catalog, world
//! states, shadow flags), and the program-facing reader adapter ([`convert`]).
pub mod area;
pub mod character;
pub mod collision;
pub mod convert;
pub mod extras;
pub mod objects;
pub(super) mod shaders;
pub(super) mod shadows;
pub mod states;
pub(super) mod textures;
