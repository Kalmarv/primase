//! Reading: the game's own assets from its data folder ([`game`]), the skate-scene interchange folder into a world ([`interchange`]), and as a source of the convert loop
//! ([`scene::SceneReader`]).

pub mod game;
pub mod animation;
pub mod interchange;
pub(crate) mod scene;
pub use scene::EMITTED_NAMES;
