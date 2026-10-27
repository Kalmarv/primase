//! What follows Studio's compile, in its one order: the only public way from a compiled Patch to the one a player
//! installs, so no program can leave a step out.
//!
//! 1. **Blend** ([`super::vbrecord`]): a map folder with a vertex-blend record has its blend materials retargeted onto
//!    the game's stock decal row with their per-vertex opacity; one without is its compile. The written root's program
//!    records are read back and refused unless they are the game's (a lone map's root table is loaded whole, and
//!    Studio's private vertex-blend program in it crashes the boot load).
//! 2. **Level root** ([`super::levelroot`]): every document the Patch places is rewritten with its boxed strings inside
//!    it, and refused unless each then reads with its pointers inside it.
//! 3. **Layering gate** ([`super::layers`]): the result read as the game draws it. A check in [`ENFORCED`] that fails
//!    fails the build; every other finding is reported, counted, in [`Finished::layers`]. A failure family joins
//!    [`ENFORCED`] in the commit that lands its fix in the writer, so the list only grows.

use std::path::{Path, PathBuf};

use primase_foundations::compress::oodle::Oodle;

use crate::postcompile::fb::folder::{Folder, Source};
use crate::postcompile::layers::{self, Check, Limits};
use crate::postcompile::level::ROOT_TOC;
use crate::postcompile::vblend::{self, DECAL_DONOR, Donor, StockPrograms};
use crate::postcompile::{Error, Result, levelroot, map_folder, vbrecord};

/// The level whose decal material blend overlays are retargeted onto.
const DECAL_TOC: &str = "Win32/levels/game/bam_levelroot/bam_levelroot.toc";

/// The layering checks whose failure fails the build: those whose fix the writer carries. [`Check::UnknownProgram`]:
/// the blend stage puts the game's program records back, so no section names a program the game lacks.
pub const ENFORCED: &[Check] = &[Check::UnknownProgram];

/// What [`finish`] did.
#[derive(Clone, Debug, PartialEq)]
pub struct Finished {
    /// The Patch to install.
    pub patch: PathBuf,
    /// The blend stage's report and resolution; `None` when the map has no vertex-blend record.
    pub blend: Option<(vblend::Report, vbrecord::Resolution)>,
    pub level_root: levelroot::Report,
    pub layers: layers::Report,
    /// How long the layering gate took, seconds (reading the result and checking it).
    pub gate_seconds: f64,
    /// The checks that fail but are not yet enforced (reported, the build goes on).
    pub reported: Vec<Check>,
}

/// Finishes Studio's compile `compiled` (its Patch folder) of the map folder `map` into `out` (which must not
/// exist), against the game's `Data` folder `data` and its Oodle library; see the module documentation. The folder
/// `out` with `.blended` appended holds the blend stage's output while the level root is repaired, and is removed.
#[allow(
    clippy::disallowed_methods,
    reason = "writer I/O: refuses an existing output folder and removes the stage between steps"
)]
pub fn finish(map: &Path, compiled: &Path, out: &Path, data: &Folder, oodle: &Oodle) -> Result<Finished> {
    if out.exists() {
        return Err(Error::Missing(format!(
            "{} exists; give a new folder",
            out.display()
        )));
    }
    let game = Source {
        folder: data,
        base: Some(data),
        oodle: Some(oodle),
    };
    let stock = StockPrograms::find(&game, ROOT_TOC)?;
    // 1. blend
    let blended = out.with_extension("blended");
    let (staged, blend) = match map_folder::read(map)? {
        None => (compiled.to_path_buf(), None),
        Some(input) => {
            let decal = Donor::find(&game, DECAL_TOC, ROOT_TOC, DECAL_DONOR)?;
            let done = vbrecord::apply(
                compiled,
                &blended,
                &input,
                Some(data),
                Some(oodle),
                &decal,
                &stock,
            )?;
            (blended.clone(), Some(done))
        }
    };
    let written = Folder::open(&staged)?;
    let got = StockPrograms::find(
        &Source {
            folder: &written,
            base: Some(data),
            oodle: Some(oodle),
        },
        ROOT_TOC,
    )?;
    if got != stock {
        return Err(Error::Missing(format!(
            "{}: the root program records are not the game's",
            staged.display()
        )));
    }
    // 2. level root
    let donor = levelroot::Donor::find(
        &game,
        levelroot::DONOR_TOC,
        levelroot::DONOR_BUNDLE,
        levelroot::DONOR_EBX,
    )?;
    let level_root = levelroot::apply(&staged, out, Some(data), Some(oodle), &donor)?;
    if blend.is_some() {
        std::fs::remove_dir_all(&blended)
            .map_err(|e| Error::Missing(format!("{}: {e}", blended.display())))?;
    }
    // 3. layering gate
    let installed = Folder::open(out)?;
    let limits = Limits::default();
    let started = std::time::Instant::now();
    let report = layers::gate(
        &Source {
            folder: &installed,
            base: Some(data),
            oodle: Some(oodle),
        },
        &limits,
    )?;
    let failing = report.failing(&limits);
    let enforced: Vec<Check> = failing.iter().copied().filter(|c| ENFORCED.contains(c)).collect();
    if !enforced.is_empty() {
        return Err(Error::Missing(format!(
            "{}: the layering gate fails {enforced:?}",
            out.display()
        )));
    }
    Ok(Finished {
        patch: out.to_path_buf(),
        blend,
        level_root,
        layers: report,
        gate_seconds: started.elapsed().as_secs_f64(),
        reported: failing.into_iter().filter(|c| !ENFORCED.contains(c)).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The enforced checks, pinned: a chunk that lands a family's fix in the writer adds its check here and to
    /// [`ENFORCED`] in the same commit; none leaves.
    #[test]
    fn the_enforced_checks_are_pinned() {
        assert_eq!(ENFORCED, &[Check::UnknownProgram]);
    }
}
