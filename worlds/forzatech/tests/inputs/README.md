# Real-data inputs

What this crate's real-data tests read, as tests/inputs/mod.rs declares them: the keys of the configuration file
`GAME_DATA` names (its layout is primase-testdata's), the oracle sets and the files of each, and the debug keys
`GAME_DATA_DEBUG` sets. A test whose input is not configured skips with a line naming its key.

<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `installs.fh6` | - | Forza Horizon 6's install folder (the folder that holds `media`) | the user's own install |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `installs.fh6/media/Tracks/Brio` | live | - | the Tokyo track's folder (`media/Tracks/Brio`: four GeoChunks with their chunk maps and contents lists, `Ribbon_00`'s `.pvs` and template libraries, `bin.zip` with its asset manifest, materials and shaders) and, through it, the install's texture library (`media/_library/Textures.zip` and the `TexturesPG` zips); tests/shibuya.rs | the user's own install |

<!-- end of real-data inputs -->
