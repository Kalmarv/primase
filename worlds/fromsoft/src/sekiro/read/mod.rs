//! The Sekiro reader: the area read ([`area`]) and the parts of it with modules of their own (texture catalog, world
//! states, shadow flags), and the program-facing reader adapter ([`convert`]).
pub mod area;
pub mod collision;
pub mod convert;
pub mod extras;
pub mod objects;
pub(super) mod shadows;
pub mod states;
pub(super) mod textures;
