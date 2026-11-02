# Real-data inputs

What this crate's real-data tests read, as tests/inputs/mod.rs declares them: the keys of the configuration file
`GAME_DATA` names (its layout is primase-testdata's), the oracle sets and the files of each, and the debug keys
`GAME_DATA_DEBUG` sets. A test whose input is not configured skips with a line naming its key.

<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `tools.blender` | yes | Blender's executable (`blender.exe`, 5.2.3 LTS), run in the background only: the save and render stages build and render the scenes the tests write | third-party: the Blender Foundation's Blender 5.2.3 LTS release build |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

<!-- end of real-data inputs -->
