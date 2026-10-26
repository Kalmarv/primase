//! Steps that run after ReSkate Studio's compile, on the native Patch folder it writes: the MeshSet resource
//! (`meshset`), per-vertex blending onto stock material rows (`vblend`) and the map writer's vertex-blend record
//! (read from the map folder by `map_folder`) resolved against the compile (`vbrecord`), and the level root's boxed
//! strings that point out of their document (`levelroot`); and the layering gate on the result (`layers`, over the
//! compiled level as drawn, `level`); over the Frostbite containers of `primase-frostbite` (re-exported as `fb`).
//!
//! [`finish::finish`] runs them in their one order and is the only public way to apply them: the steps that write a
//! Patch are this crate's own, so no program can leave one out, the gate included.

pub mod finish;
pub mod layers;
pub mod level;
pub mod levelroot;
pub mod map_folder;
pub mod vblend;
pub mod vbrecord;

/// The Frostbite containers (`primase-frostbite`).
pub use primase_frostbite as fb;
pub(crate) use primase_frostbite::bad;
/// The MeshSet resource reader, which lives with the engine's containers.
pub use primase_frostbite::meshset;
pub use primase_frostbite::{Error, Result};
