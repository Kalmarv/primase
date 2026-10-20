//! The stage after writing a map: ReSkate Studio's `compile-map` of a normalized map folder into a mod's staging
//! folder, run as the program states it: which Studio, which game install, which staging folder and mod folder, and
//! every compile choice ([`CompileChoices`]). Nothing here adapts the map; a choice left unstated is Studio's own.
//! Orchestration (which maps, their names, deploying, packaging) is the program's.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::{Error, Result};

/// Where Studio is and which game it compiles against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Studio {
    /// Studio's command line (`reskate_cli.exe`).
    pub cli: PathBuf,
    /// The skate. install Studio reads the stock data from (read only).
    pub game: PathBuf,
}

/// The time of day Studio compiles the map's lighting at (`--time-of-day`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeOfDay {
    Morning,
    Noon,
    Afternoon,
    Evening,
    Night,
}

impl TimeOfDay {
    /// Studio's word for it.
    pub fn word(self) -> &'static str {
        match self {
            TimeOfDay::Morning => "morning",
            TimeOfDay::Noon => "noon",
            TimeOfDay::Afternoon => "afternoon",
            TimeOfDay::Evening => "evening",
            TimeOfDay::Night => "night",
        }
    }
}

/// How the compiled map is loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Streaming {
    /// In World Partition cells, with Studio's vista proxies (`--stream`).
    Cells,
    /// Resident, whole (`--no-stream`).
    Resident,
}

/// The pause map Studio draws (`--pause-map`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseMap {
    /// Flat (`2d`).
    Flat,
    /// In relief (`3d`).
    Relief,
}

/// The compile's choices, each stated by the program; `None` passes no flag, leaving Studio's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompileChoices {
    pub streaming: Option<Streaming>,
    /// Studio's level-of-detail meshes (`true`), or the map's own geometry at every distance (`false`, `--no-lods`).
    pub lods: bool,
    /// Baked global illumination (`true`), or none (`false`, `--no-gi`).
    pub gi: bool,
    pub time_of_day: Option<TimeOfDay>,
    pub pause_map: Option<PauseMap>,
}

impl CompileChoices {
    /// Studio's arguments for them, in a fixed order.
    pub fn arguments(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        match self.streaming {
            Some(Streaming::Cells) => out.push("--stream"),
            Some(Streaming::Resident) => out.push("--no-stream"),
            None => {}
        }
        if !self.lods {
            out.push("--no-lods");
        }
        if !self.gi {
            out.push("--no-gi");
        }
        if let Some(t) = self.time_of_day {
            out.extend(["--time-of-day", t.word()]);
        }
        if let Some(p) = self.pause_map {
            out.extend([
                "--pause-map",
                match p {
                    PauseMap::Flat => "2d",
                    PauseMap::Relief => "3d",
                },
            ]);
        }
        out
    }
}

/// What a compile left.
#[derive(Clone, Debug, PartialEq)]
pub struct Compiled {
    /// The mod's native Patch folder, in the staging folder.
    pub patch: PathBuf,
    /// Studio's output, beside the staging folder (`<staging>.compile.log`).
    pub log: PathBuf,
    pub seconds: f64,
    /// Lines of Studio's output that are warnings (`warning:`).
    pub warnings: usize,
}

/// Compile the normalized map folder `map` with `studio` into `staging` (absent or empty: an existing folder with
/// anything in it is refused, never emptied here) as the mod folder `mod_folder`, by `choices`. Studio's output is kept
/// as the log beside `staging` whatever happens; a compile Studio fails is an error naming it.
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "writer I/O: runs Studio's compile-map, which writes the mod into the staging folder, and writes its log"
)]
pub fn compile_map(
    studio: &Studio,
    map: &Path,
    staging: &Path,
    mod_folder: &str,
    choices: &CompileChoices,
) -> Result<Compiled> {
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| Error::Io { path, source }
    };
    if staging.exists() {
        if std::fs::read_dir(staging).map_err(io(staging))?.next().is_some() {
            return Err(Error::Write(format!(
                "{}: the staging folder is not empty (empty it first if it is to be replaced)",
                staging.display()
            )));
        }
    } else {
        std::fs::create_dir_all(staging).map_err(io(staging))?;
    }
    let log = staging.with_extension("compile.log");
    let started = Instant::now();
    let out = Command::new(&studio.cli)
        .arg("compile-map")
        .arg(&studio.game)
        .arg(map)
        .arg(staging)
        .args(["--mod-folder", mod_folder])
        .args(choices.arguments())
        .output()
        .map_err(io(&studio.cli))?;
    let text = [out.stdout, out.stderr].concat();
    std::fs::write(&log, &text).map_err(io(&log))?;
    if !out.status.success() {
        return Err(Error::Write(format!(
            "Studio's compile-map failed ({}); its output: {}",
            out.status,
            log.display()
        )));
    }
    Ok(Compiled {
        patch: staging.join("Patch"),
        log,
        seconds: started.elapsed().as_secs_f64(),
        warnings: String::from_utf8_lossy(&text)
            .lines()
            .filter(|l| l.starts_with("warning:"))
            .count(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_become_studios_arguments_and_unstated_ones_none() {
        let resident = CompileChoices {
            streaming: Some(Streaming::Resident),
            lods: false,
            gi: false,
            time_of_day: Some(TimeOfDay::Noon),
            pause_map: None,
        };
        assert_eq!(
            resident.arguments(),
            ["--no-stream", "--no-lods", "--no-gi", "--time-of-day", "noon"]
        );
        let studios = CompileChoices {
            streaming: None,
            lods: true,
            gi: true,
            time_of_day: None,
            pause_map: Some(PauseMap::Relief),
        };
        assert_eq!(studios.arguments(), ["--pause-map", "3d"]);
    }

    #[allow(
        clippy::disallowed_methods,
        reason = "test I/O: a staging folder with something in it, and a Studio that is not there"
    )]
    #[test]
    fn a_staging_folder_in_use_is_refused_and_a_missing_studio_named() {
        let root = std::env::temp_dir().join(format!("primase-compile-{}", std::process::id()));
        let staging = root.join("stage");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("kept.txt"), b"x").unwrap();
        let studio = Studio {
            cli: root.join("no_studio.exe"),
            game: root.join("game"),
        };
        let choices = CompileChoices {
            streaming: None,
            lods: true,
            gi: true,
            time_of_day: None,
            pause_map: None,
        };
        let refused = compile_map(&studio, &root.join("map"), &staging, "Mod", &choices).unwrap_err();
        assert!(refused.to_string().contains("not empty"), "{refused}");
        assert!(staging.join("kept.txt").exists(), "nothing was emptied");
        let missing =
            compile_map(&studio, &root.join("map"), &root.join("fresh"), "Mod", &choices).unwrap_err();
        assert!(missing.to_string().contains("no_studio.exe"), "{missing}");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
