//! PS3 RSX data: shader program containers (CgBinary and fragment programs) and the `CellGcmTexture` descriptor.
//! The container reader keeps microcode encoded; it does not infer shader behaviour from parameter names.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod cgbinary;
pub mod fragment;
pub mod gcm;
