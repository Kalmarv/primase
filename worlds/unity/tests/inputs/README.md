# Real-data inputs

What this crate's real-data tests read, as tests/inputs/mod.rs declares them: the keys of the configuration file
`GAME_DATA` names (its layout is primase-testdata's), the oracle sets and the files of each, and the debug keys
`GAME_DATA_DEBUG` sets. A test whose input is not configured skips with a line naming its key.

<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `installs.skatebird` | yes | SkateBIRD's install folder (`SkateBIRD_Data`: a Unity 2019.4.20f1 player) | the user's own install |
| `installs.sledding` | yes | Sledding Game's install folder (`Sledding Game_Data`: a Unity 6000.3 player) | the user's own install |
| `tools.unity_tpk` | yes | a type tree package (TPK, uncompressed or LZ4) with the class layouts for the Unity players read | input data: class layouts for the player's Unity version (repacked with a script kept with the private oracle tools) |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

#### Oracle sets (`<oracles.root>/<set>/<game>/`)

| set | about | source |
| --- | --- | --- |
| `oracles.port` | another exporter's output for a game (its level folder, materials joined to their objects, terrain unpacked) | another exporter's output: an appearance baseline, not a reading of the source |
| `oracles.unitypy` | UnityPy's readings of a Unity player (object tables, samples, meshes, textures, materials, shaders, frames, listings) | independent: UnityPy, through dump scripts kept with the private oracle tools |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `installs.skatebird/SkateBIRD_Data` | live | yes | SkateBIRD's data folder (`SkateBIRD_Data`, a Unity 2019.4.20f1 player: SerializedFile version 21 containers), read with `tools.unity_tpk`; `worlds/unity/tests/serialized_census.rs`, `worlds/skatebird/tests/markers.rs` | the user's own install |
| `installs.sledding/Sledding Game_Data` | live | yes | a Unity player's data folder (`<Game>_Data`, holding `globalgamemanagers` and the `levelN` / `*.assets` files) | the user's own install |
| `oracles.port.sledding/level1` | live | yes | optional: another exporter's output folder for that game's `level1` and its materials joined to their objects (JSON; schema in `worlds/unity/tests/lower.rs`) | another exporter's output: an appearance baseline, not a reading of the source |
| `oracles.port.sledding/materials.json` | live | yes | optional: another exporter's output folder for that game's `level1` and its materials joined to their objects (JSON; schema in `worlds/unity/tests/lower.rs`) | another exporter's output, joined by a script kept with the private oracle tools |
| `oracles.port.sledding/terrain` | live | yes | optional: that exporter's terrain pieces unpacked (raw arrays, `index.json`, its layer means; schema in `worlds/unity/tests/terrain.rs`) | another exporter's terrain, unpacked by a script kept with the private oracle tools |
| `oracles.unitypy.skatebird/listing.json` | live | yes | optional: an independent reader's (UnityPy) object names and BuildSettings scene list of `installs.skatebird/SkateBIRD_Data` (JSON; schema in `worlds/unity/tests/listing.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.sledding/frames.json` | live | yes | an independent reader's (UnityPy) decoding of a few meshes in Unity's own frame (vertices, normals, tangents, UV0, submesh triangles); `worlds/unity/tests/handedness.rs` | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.sledding/listing.json` | live | yes | optional: an independent reader's (UnityPy) object names and BuildSettings scene list of `installs.sledding/Sledding Game_Data` (JSON; schema in `worlds/unity/tests/listing.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.sledding/materials.json` | live | yes | optional: an independent reader's materials of that folder (JSON; schema in `worlds/unity/tests/material.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.sledding/meshes.json` | live | yes | optional: an independent reader's decoded meshes of that folder (JSON; schema in `worlds/unity/tests/mesh.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.sledding/samples.json` | live | yes | optional: an independent reader's decoded sample objects of that folder (JSON; schema in `worlds/unity/tests/oracle.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.sledding/scan.json` | live | yes | optional: an independent reader's scan of a level of that folder (JSON; schema in `worlds/unity/tests/hierarchy.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.sledding/shader-programs.json` | live | yes | an independent reader's (UnityPy) compiled fragment programs of every shader (per pass the player variants, keywords, program type and code hash, and the constant buffers, textures and samplers); `worlds/unity/tests/program.rs` | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.sledding/shader-state.json` | live | yes | an independent reader's (UnityPy) shader render state of every material (subshader tags; per pass LightMode, blend factors, zwrite and cull with their bound properties); `worlds/unity/tests/material.rs` | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.sledding/sun.json` | live | yes | an independent reader's (UnityPy) record of the scan level's RenderSettings sun as stored, the render-pipeline asset classes, the shining directional lights and the main-light choice; `worlds/unity/tests/light.rs` | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.sledding/textures` | live | yes | optional: an independent reader's decoded textures of that folder (a folder of PNGs and `index.json`; schema in `worlds/unity/tests/texture.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |

<!-- end of real-data inputs -->
