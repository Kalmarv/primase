//! Where the pieces of a skate. install lie, as names a caller reads them by. Each is observed in the install the
//! reader was written against; nothing here is read from a file.

/// The folder of the install that holds the superbundles' TOCs, relative to the install.
pub const DATA_FOLDER: &str = "Data";

/// The TOC whose presence marks a data folder as the game's (the folder's own layout).
pub const LAYOUT_TOC: &str = "layout.toc";

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
