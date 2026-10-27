//! The reader: a data folder's entries as entities of a world.

pub mod entry;
pub mod geometry;
pub mod program;
pub mod skeleton;
pub mod textures;

use frostbite_formats::listing::{Bundle, Listed};
use frostbite_formats::{Error, Result};

/// EBX document `i` of `bundle`'s manifest as an entry; an error when the manifest has none or the bundle's file table
/// holds no file for it.
pub(crate) fn ebx(bundle: &Bundle, i: usize) -> Result<Listed> {
    bundle.ebx(i).ok_or_else(|| unlisted(bundle, "EBX document", i))
}

/// Resource `i` of `bundle`'s manifest as an entry; an error when the manifest has none or the bundle's file table
/// holds no file for it.
pub(crate) fn resource(bundle: &Bundle, i: usize) -> Result<Listed> {
    bundle.resource(i).ok_or_else(|| unlisted(bundle, "resource", i))
}

fn unlisted(bundle: &Bundle, what: &str, i: usize) -> Error {
    Error::Missing(format!(
        "{} ({}): {what} {i} of the manifest has no file in the bundle's file table",
        bundle.name, bundle.toc_path
    ))
}
