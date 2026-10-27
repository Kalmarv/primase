# Real-data inputs

What this crate's real-data tests read, as tests/inputs/mod.rs declares them: the keys of the configuration file
`GAME_DATA` names (its layout is primase-testdata's), the oracle sets and the files of each, and the debug keys
`GAME_DATA_DEBUG` sets. A test whose input is not configured skips with a line naming its key.

<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `tools.gltf_validator` | yes | the Khronos glTF-Validator executable (2.0.0-dev.3.10 release build) | third-party: Khronos glTF-Validator 2.0.0-dev.3.10 |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

#### Oracle sets (`<oracles.root>/<set>/<game>/`)

| set | about | source |
| --- | --- | --- |
| `oracles.baseline` | our own earlier outputs kept to detect change (an earlier exporter's scene, a map our writer built, a frozen stage result) | regression baselines: our own earlier outputs, not independent oracles |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `oracles.baseline.sekiro/ktx2-tiles` | live | yes | a web tile export with KTX2 textures and its plain set, both written from the same prepared cell worlds (one conversion writing both sets, `primase_web::convert::tiles`): tiles.json, the tiles, images/, plain/tiles.json and its tiles; formats/web/tests/ktx2_audit.rs | our own writer's output: examples/examples/sekiro_ktx2_tiles.rs writes it (slice A of Sekiro's m11_01, both sets in one conversion), so the plain set holds the KTX2 encoder's input; the checks are an independent decoder's (the pure-Rust basisu transcoder, against the plain set) and the viewer's WASM transcoder's (the printed hashes) |

<!-- end of real-data inputs -->
