//! The reader's I/O entry points: the installed archives (BHD5/BDT), their DCX envelopes (Oodle and zlib) and the
//! BND4 binders read through them. The formats they decode are pure codecs at the crate root.
pub mod archive;
pub mod binder;
pub mod dcx;
