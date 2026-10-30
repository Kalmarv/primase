# Real-data inputs

What this crate's real-data tests read, as tests/inputs/mod.rs declares them: the keys of the configuration file
`GAME_DATA` names (its layout is primase-testdata's), the oracle sets and the files of each, and the debug keys
`GAME_DATA_DEBUG` sets. A test whose input is not configured skips with a line naming its key.

<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `installs.peak` | yes | PEAK's install folder (`PEAK_Data`: a Unity 6000.3.15f1 player with level AssetBundles) | the user's own install |
| `tools.unity_tpk` | yes | a type tree package (TPK, uncompressed or LZ4) with the class layouts for the Unity players read | input data: class layouts for the player's Unity version (repacked with a script kept with the private oracle tools) |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

#### Oracle sets (`<oracles.root>/<set>/<game>/`)

| set | about | source |
| --- | --- | --- |
| `oracles.unity-editor` | what Unity's own editor makes of bytes cut from a player as stored (a script's steps run by the engine on them) | independent: Unity's editor (6000.6.4f1, batchmode), through editor scripts kept with the private oracle tools |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `installs.peak/PEAK_Data` | live | yes | PEAK's data folder (`PEAK_Data`), its scene `level4` and the material M_SaltRock (`resources.assets` path id 136) read with `tools.unity_tpk`; `worlds/peak/tests/globals.rs`, `worlds/peak/tests/resolve.rs` | the user's own install |
| `oracles.unity-editor.peak/light-volume-level4` | live | yes | level4's light volume as the engine assembles it (`volume_rgba16f.bin`: the RGBAHalf Texture3D's level 0, 166,712,832 bytes, after `SetPixels` and `Apply` of its 572 BC6H and BC4 slices' `GetPixels`, as `Peak.CompressableLightMap.Decompress` does); `worlds/peak/tests/globals.rs` | Unity's editor 6000.6.4f1 (the player is 6000.3.15f1) on the slices cut out as stored by UnityPy, through scripts kept with the private oracle tools |

<!-- end of real-data inputs -->
