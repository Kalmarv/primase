//! Steps that run after ReSkate Studio's compile, on the native Patch folder it writes: the MeshSet resource
//! (`meshset`), per-vertex blending onto stock material rows (`vblend`) and the map writer's vertex-blend record
//! resolved against the compile (`vbrecord`), over the Frostbite containers of
//! `primase-frostbite` (re-exported as `fb`).

pub mod meshset;
pub mod vblend;
pub mod vbrecord;

/// The Frostbite containers (`primase-frostbite`).
pub use primase_frostbite as fb;
pub use primase_frostbite::{Error, Result};
pub(crate) use primase_frostbite::{Rd, bad, capacity};
