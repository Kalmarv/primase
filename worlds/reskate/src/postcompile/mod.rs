//! Steps that run after ReSkate Studio's compile, on the native Patch folder it writes: the MeshSet resource
//! (`meshset`), per-vertex blending onto stock material rows (`vblend`) and the map writer's vertex-blend record
//! (read from the map folder by `map_folder`) resolved against the compile (`vbrecord`), and the level root's boxed
//! strings that point out of their document (`levelroot`), over the Frostbite containers of `primase-frostbite`
//! (re-exported as `fb`).

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
