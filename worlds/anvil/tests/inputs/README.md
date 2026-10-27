# Real-data inputs

What this crate's real-data tests read, as tests/inputs/mod.rs declares them: the keys of the configuration file
`GAME_DATA` names (its layout is primase-testdata's), the oracle sets and the files of each, and the debug keys
`GAME_DATA_DEBUG` sets. A test whose input is not configured skips with a line naming its key.

<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `installs.riders` | - | Riders Republic's folder of `.forge` archives (Steam build 25394987) | the user's own install |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

#### Oracle sets (`<oracles.root>/<set>/<game>/`)

| set | about | source |
| --- | --- | --- |
| `oracles.rrassetstudio` | RRAssetStudio's read of every Mesh of a Riders Republic install (`summary.jsonl`, one row per Mesh) | third-party: RRAssetStudio v3, run over the user's own install by the primase-oracles harness |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `installs.riders/DataPC_MAD_USNAT.forge` | live | - | a large Anvil forge of version 27 (Riders Republic, Steam build 25394987: `DataPC_MAD_USNAT.forge`, 15,650,919,308 bytes, 767,463 entries; its tables are read, about 163 MB); https://github.com/Kalmarv/anvil-formats/blob/main/tests/mesh_shape.rs, worlds/anvil/tests/shapes.rs, meshes.rs | the user's own install; two MeshShape bodies' prefixes, section runs and tails hand-checked in a separate read-only decode (games/riders-republic docs/recon/meshshape-second-fixture.md, c068d20) |
| `installs.riders/DataPC_MAD_USNAT_patch_01.forge` | live | - | the patch forge of installs.riders/DataPC_MAD_USNAT.forge (Riders Republic, Steam build 25394987: `DataPC_MAD_USNAT_patch_01.forge`, 1,331,014,794 bytes, 320,777 entries); worlds/anvil/tests/meshes.rs | the user's own install; four Mesh bodies' descriptors, buffers and splits hand-checked in a separate read-only decode (games/riders-republic docs/recon/mesh-second-fixture.md, a03c970) |
| `installs.riders/DataPC_MAD_WhiteRoom.forge` | live | - | the base forge of installs.riders/DataPC_MAD_WhiteRoom_patch_01.forge's patch (Riders Republic, Steam build 25394987: `DataPC_MAD_WhiteRoom.forge`, 8,951,822 bytes, 388 entries); https://github.com/Kalmarv/anvil-formats/blob/main/tests/data.rs | the user's own install; two resource blocks' sizes, checksums, names and class ids hand-checked in a separate read-only decode (games/riders-republic docs/recon/archive-reader-fit.md, a9f4959) |
| `installs.riders/DataPC_MAD_WhiteRoom_patch_01.forge` | live | - | one Anvil forge archive of version 27 (Riders Republic, Steam build 25394987: `DataPC_MAD_WhiteRoom_patch_01.forge`, 1,021,872 bytes, 337 entries); https://github.com/Kalmarv/anvil-formats/blob/main/tests/forge.rs | the user's own install; rows hand-checked against a separate read-only census of the file |
| `oracles.rrassetstudio.riders/meshes` | live | - | RRAssetStudio v3's read of every Mesh in installs.riders (a folder: summary.jsonl, one row per Mesh with counts, submeshes and material ids, and SHA-256s of its positions as f32 and absolute indices as u32, little-endian; sample/; identity.json); worlds/anvil/tests/census.rs, run by hand | a third-party oracle: RRAssetStudio v3 (zip SHA-256 f13edec5...) run over the user's own install by the primase-oracles harness dumps/rras-anvil (d5c48a9); identity.json pins its modules and the forges |

#### Debug keys (`GAME_DATA_DEBUG`)

| key | about |
| --- | --- |
| `anvil.bake_digest` | the digest the baked Riders Republic Materials must have |

<!-- end of real-data inputs -->
