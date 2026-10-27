# Real-data inputs

What this crate's real-data tests read, as tests/inputs/mod.rs declares them: the keys of the configuration file
`GAME_DATA` names (its layout is primase-testdata's), the oracle sets and the files of each, and the debug keys
`GAME_DATA_DEBUG` sets. A test whose input is not configured skips with a line naming its key.

<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

#### Oracle sets (`<oracles.root>/<set>/<game>/`)

| set | about | source |
| --- | --- | --- |
| `oracles.khronos` | Khronos glTF-Sample-Assets, one folder per model, with their expected readings | third-party: Khronos glTF-Sample-Assets at commit 15cd02d304e54cb42a4db4eefe719f76190f4b68; expectations by pygltflib and Pillow |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `oracles.khronos.general/sample-assets` | live | yes | a folder of Khronos glTF-Sample-Assets models, one folder per model (`<Model>/<Model>.glb` or `.gltf` with its buffers, `expected.json` written by pygltflib from the same file, `validator.json` by the Khronos validator and, for a model with images, `images.json` by Pillow; for a JPEG image `<Model>/images/<n>.jpg` and the RGBA8 rows Pillow decodes of it, `<Model>/images/<n>.rgba`) | third-party: Khronos glTF-Sample-Assets at commit 15cd02d304e54cb42a4db4eefe719f76190f4b68; expected.json by pygltflib 1.16.5; images.json by Pillow 11.0.0 |

<!-- end of real-data inputs -->
