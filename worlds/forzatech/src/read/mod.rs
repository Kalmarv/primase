//! Reading a track: open it ([`open`]), read the placements of a box and season ([`placements`]), load the models of
//! the prototypes a part reaches ([`models`]) with their materials ([`materials`]) and textures ([`textures`]);
//! [`convert`] is the `convert::Reader` over them. A track's time-of-day file is read apart, for the sky a caller
//! names ([`sky`]).
pub mod convert;
pub mod materials;
pub mod models;
pub mod open;
pub mod placements;
pub mod sky;
pub mod textures;
