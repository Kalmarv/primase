//! Where the pieces of a skate. install lie, as names a caller reads them by. Each is observed in the install the
//! reader was written against; [`locate`] checks an install for them.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: checks that an install holds the files the reader opens"
)]

use std::path::{Path, PathBuf};

/// The folder of the install that holds the superbundles' TOCs, relative to the install.
pub const DATA_FOLDER: &str = "Data";

/// The TOC whose presence marks a data folder as the game's (the folder's own layout).
pub const LAYOUT_TOC: &str = "layout.toc";

/// The superbundle TOC of the items (costumes, props, boards), relative to the data folder.
pub const ITEMS_TOC: &str = "Win32/items.toc";

/// The Oodle library the game ships beside its executable; its archives are compressed with it (the `9` is the Oodle
/// version the file name states).
pub const OODLE_LIBRARY: &str = "oo2core_9_win64.dll";

/// The TOCs, relative to the data folder, whose bundles hold the shader programs and permutations that the stock
/// ones a material's database names lie in: the game configuration's and the level root's (see
/// [`crate::read::program`]).
pub const SHADER_PROGRAM_TOCS: [&str; 2] = [
    "Win32/globals.toc",
    "Win32/levels/game/dingolevel_root/dingolevel_root.toc",
];

/// The data folder and the Oodle library of an install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    /// The folder of the superbundles' TOCs.
    pub data: PathBuf,
    /// The Oodle library the archives are decompressed with.
    pub oodle: PathBuf,
}

/// The data folder and the Oodle library of the install at `install`, or why it is not a skate. install: the folder
/// has no [`LAYOUT_TOC`], or the game's own Oodle library is missing.
pub fn locate(install: &Path) -> Result<Installed, String> {
    let data = install.join(DATA_FOLDER);
    if !data.join(LAYOUT_TOC).is_file() {
        return Err(format!(
            "frostbite: {} has no {LAYOUT_TOC}: expected the skate. game folder",
            data.display()
        ));
    }
    let oodle = install.join(OODLE_LIBRARY);
    if !oodle.is_file() {
        return Err(format!(
            "frostbite: {} not found: the game's own Oodle library is needed to read its archives",
            oodle.display()
        ));
    }
    Ok(Installed { data, oodle })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder that is not an install is refused, naming the layout TOC.
    #[test]
    fn a_folder_without_the_layout_toc_is_refused() {
        let error = locate(&std::env::temp_dir().join("no-such-skate-install")).unwrap_err();
        assert!(error.contains(LAYOUT_TOC), "{error}");
    }
}
