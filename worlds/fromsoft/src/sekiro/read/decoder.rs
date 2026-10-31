//! The installed decoder library the supported source archives require.
#![allow(
    clippy::disallowed_methods,
    reason = "reader I/O: loads the caller's trusted installed game codec"
)]
use primase_foundations::compress::oodle::{Oodle, Version};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

/// Borrow the installed game's V6 library for the process lifetime, shared across reader sessions.
/// The library is never searched for or replaced by another version.
#[allow(unsafe_code)]
pub fn load_installed(install: &Path) -> fromsoft_formats::Result<&'static Oodle> {
    static CODECS: OnceLock<Mutex<BTreeMap<PathBuf, &'static Oodle>>> = OnceLock::new();
    let path = install
        .join("oo2core_6_win64.dll")
        .canonicalize()
        .map_err(|error| {
            crate::invalid(
                "installed decoder",
                0,
                format!("{}: no readable oo2core_6_win64.dll: {error}", install.display()),
            )
        })?;
    let mut codecs = CODECS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| crate::invalid("installed decoder", 0, "Oodle cache is poisoned"))?;
    if let Some(codec) = codecs.get(&path) {
        return Ok(*codec);
    }
    // SAFETY: the caller selected this trusted installed game's V6 codec; the retained owner outlives every read.
    let codec = unsafe { Oodle::load(&path, Version::V6) }.map_err(|error| {
        crate::invalid(
            "installed decoder",
            0,
            format!("loading {}: {error}", path.display()),
        )
    })?;
    let codec: &'static Oodle = Box::leak(Box::new(codec));
    codecs.insert(path, codec);
    Ok(codec)
}
