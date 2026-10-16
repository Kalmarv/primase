//! Steps that run after ReSkate Studio's compile, on the native Patch folder it writes: the MeshSet resource
//! (`meshset`) and per-vertex blending onto stock material rows (`vblend`), over the Frostbite containers of
//! `primase-frostbite` (re-exported as `fb`).

pub mod meshset;
pub mod vblend;

/// The Frostbite containers (`primase-frostbite`).
pub use primase_frostbite as fb;
pub use primase_frostbite::{Error, Result};
pub(crate) use primase_frostbite::{Rd, bad, capacity};
