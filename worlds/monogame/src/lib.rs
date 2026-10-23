//! MonoGame content as games built on it ship it (the XNA 4.0 content format, which FNA reads too).
//!
//! - [`mgfx`]: the compiled effect an XNB of MonoGame's `EffectReader` holds (version 12, OpenGL).
//! - [`programs`]: an effect's shaders lifted to shader programs.
//! - [`xnb`]: the XNB container (uncompressed, version 5): its header, its type-reader table as stored, and the
//!   primary object's bytes, with reads for the values XNA's built-in readers write (7-bit encoded lengths, strings,
//!   byte arrays). A game's own readers are its game crate's to name: the container hands their bytes over.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod mgfx;
pub mod programs;
pub mod xnb;
