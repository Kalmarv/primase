# Real-data inputs

What this crate's real-data tests read, as tests/inputs/mod.rs declares them: the keys of the configuration file
`GAME_DATA` names (its layout is primase-testdata's), the oracle sets and the files of each, and the debug keys
`GAME_DATA_DEBUG` sets. A test whose input is not configured skips with a line naming its key.

<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `installs.skate` | yes | the skate. install the ReSkate tools use (`Data`: its `layout.toc`, TOCs and CAS archives; the Oodle 9 DLL) | the user's own install |
| `tools.oodle9` | yes | an Oodle version-9 DLL (`oo2core_9_win64.dll`); on Windows only: elsewhere oozextract decodes and nothing is configured | the game's own DLL from the user's install |
| `tools.reskate_cli` | - | ReSkate Studio's CLI executable (`reskate_cli`), an independent EBX reader and reference writer | third-party: ReSkate Studio's CLI (20261012) |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `installs.skate/Data` | live | yes | the skate. install's `Data` folder (its `layout.toc`, TOCs and CAS archives), read only | the user's own install |

<!-- end of real-data inputs -->
