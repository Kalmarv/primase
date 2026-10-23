//! RAGE's resource formats as GTA V Enhanced stores them, decoded from bytes: resources (RSC7), META documents and the
//! map data in them, drawables, texture dictionaries, collision bounds and buffer formats. No I/O: the reader hands
//! them bytes, and the components and passes use the types.
pub mod bounds;
pub mod drawable;
pub mod fxc;
pub(crate) mod le;
pub mod map;
pub mod meta;
pub mod resource;
pub mod sga;
pub mod sgd2;
pub mod texture;
