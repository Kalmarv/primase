//! Riders Republic's install: which forges make each world, in the order their ids are searched.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: checks which of a world's forges the caller's install folder holds"
)]

use std::path::Path;

use crate::read::convert::{Edition, ForgeFile};

/// A world of the install: its forges, searched in this order (the first holding an id answers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldForges {
    pub name: &'static str,
    pub forges: &'static [&'static str],
}

/// The open world. A patch is searched before its base: inferred, not established from the game (the Riders Republic
/// notes lead to "the patch wins"). The shared `DataPC` forges follow the world's own.
pub const USNAT: WorldForges = WorldForges {
    name: "usnat",
    forges: &[
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_patch_01.forge",
        "DataPC.forge",
    ],
};

/// The small test level, its patch first (as [`USNAT`]).
pub const WHITEROOM: WorldForges = WorldForges {
    name: "whiteroom",
    forges: &[
        "DataPC_MAD_WhiteRoom_patch_01.forge",
        "DataPC_MAD_WhiteRoom.forge",
    ],
};

/// A forge's edition, by its file name: a patch's name holds `_patch_` (as every patch forge of the install is named).
pub fn edition(name: &str) -> Edition {
    if name.contains("_patch_") {
        Edition::Patch
    } else {
        Edition::Base
    }
}

/// The forges of `world` that the install folder `install` holds, in search order, each with its edition. None is
/// an error naming the forges looked for.
pub fn forge_files(install: &Path, world: &WorldForges) -> Result<Vec<ForgeFile>, String> {
    let files: Vec<ForgeFile> = world
        .forges
        .iter()
        .filter(|f| install.join(f).is_file())
        .map(|f| ForgeFile {
            path: install.join(f),
            edition: edition(f),
        })
        .collect();
    if files.is_empty() {
        return Err(format!(
            "anvil: none of {} is in {}",
            world.forges.join(", "),
            install.display()
        ));
    }
    Ok(files)
}
