//! The stage after writing: Blender builds a `.blend` from the writer's folder (`build.py`, run in the background:
//! Blender's own glTF importer reads the geometry, each material's graph is built as written). See `build.py` for
//! what it does, step by step.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::Value;

use crate::{Error, Result};

/// The Blender a stage runs: its executable. It runs in the background only (`--background`, no window), from its
/// factory settings (`--factory-startup`: no user preferences or add-ons).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blender {
    pub executable: PathBuf,
}

/// A `.blend` saved.
#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub blend: PathBuf,
    /// Blender's output, beside the `.blend` (`<name>.save.log`).
    pub log: PathBuf,
    pub seconds: f64,
    /// What `build.py` built: objects, meshes, materials, images, lights, nodes and links.
    pub built: Value,
}

/// The script the stage runs, written beside the folder's files as `build.py`.
pub const BUILD_PY: &str = include_str!("build.py");

/// The line `build.py` prints what it built on.
const BUILT: &str = "primase-blender-save: ";

/// Build `blend` from the writer's folder `folder` with `blender`. A script that fails, or a Blender that does not
/// say what it built, is an error naming the log.
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "writer I/O: runs Blender's save of the scene folder to a .blend, and writes the script and its log"
)]
pub fn save(blender: &Blender, folder: &Path, blend: &Path) -> Result<Saved> {
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| Error::Io { path, source }
    };
    let script = folder.join("build.py");
    std::fs::write(&script, BUILD_PY).map_err(io(&script))?;
    if let Some(parent) = blend.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    let log = blend.with_extension("save.log");
    let started = Instant::now();
    let out = std::process::Command::new(&blender.executable)
        .args([
            "--background",
            "--factory-startup",
            "--python-exit-code",
            "1",
            "--python",
        ])
        .arg(&script)
        .arg("--")
        .arg(folder)
        .arg(blend)
        .output()
        .map_err(io(&blender.executable))?;
    let text = [out.stdout, out.stderr].concat();
    std::fs::write(&log, &text).map_err(io(&log))?;
    let built = String::from_utf8_lossy(&text)
        .lines()
        .find_map(|l| l.strip_prefix(BUILT).map(str::to_owned));
    let (true, Some(built)) = (out.status.success(), built) else {
        return Err(Error::Stage(format!(
            "Blender's save of {} failed ({}); its output: {}",
            folder.display(),
            out.status,
            log.display()
        )));
    };
    let built = serde_json::from_str(&built)
        .map_err(|e| Error::Stage(format!("{}: what build.py built is not JSON: {e}", log.display())))?;
    Ok(Saved {
        blend: blend.to_owned(),
        log,
        seconds: started.elapsed().as_secs_f64(),
        built,
    })
}
