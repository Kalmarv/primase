# Real-data inputs

What this crate's real-data tests read, as tests/inputs/mod.rs declares them: the keys of the configuration file
`GAME_DATA` names (its layout is primase-testdata's), the oracle sets and the files of each, and the debug keys
`GAME_DATA_DEBUG` sets. A test whose input is not configured skips with a line naming its key.

<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `installs.skatebird` | yes | SkateBIRD's install folder (`SkateBIRD_Data`: a Unity 2019.4.20f1 player) | the user's own install |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

#### Oracle sets (`<oracles.root>/<set>/<game>/`)

| set | about | source |
| --- | --- | --- |
| `oracles.unitypy` | UnityPy's readings of a Unity player (object tables, samples, meshes, textures, materials, shaders, frames, listings) | independent: UnityPy, through dump scripts kept with the private oracle tools |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `installs.skatebird/SkateBIRD_Data` | live | yes | SkateBIRD's data folder (`SkateBIRD_Data`, a Unity 2019.4.20f1 player: SerializedFile version 21 containers), read with `tools.unity_tpk`; `worlds/unity/tests/serialized_census.rs`, `worlds/skatebird/tests/markers.rs` | the user's own install |
| `oracles.unitypy.skatebird/census.json` | live | yes | an independent UnityPy object-table census of that folder (JSON; schema in `worlds/unity/tests/serialized_census.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |

<!-- end of real-data inputs -->
