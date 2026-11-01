//! Reading a track: open it ([`open`]), read the placements of a box and season ([`placements`]), load the models of
//! the prototypes a part reaches ([`models`]) with their materials ([`materials`]) and textures ([`textures`]);
//! [`convert`] is the `convert::Reader` over them.
pub mod convert;
pub mod materials;
pub mod models;
pub mod open;
pub mod placements;
pub mod textures;
