# Real-data inputs

What this crate's real-data tests read, as tests/inputs/mod.rs declares them: the keys of the configuration file
`GAME_DATA` names (its layout is primase-testdata's), the oracle sets and the files of each, and the debug keys
`GAME_DATA_DEBUG` sets. A test whose input is not configured skips with a line naming its key.

<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `installs.riders` | - | Riders Republic's folder of `.forge` archives (Steam build 25394987) | the user's own install |
| `installs.shaunwhite` | - | Shaun White Skateboarding's (PC, 2010) folder of `.forge` archives, version 26 | the user's own copy: the forges as the release's archives hold them, extracted unchanged |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `installs.riders/DataPC_MAD_USNAT.forge` | live | - | a large Anvil forge of version 27 (Riders Republic, Steam build 25394987: `DataPC_MAD_USNAT.forge`, 15,650,919,308 bytes, 767,463 entries; its tables are read, about 163 MB); repos/anvil-formats/tests/mesh_shape.rs, worlds/anvil/tests/shapes.rs, meshes.rs | the user's own install; two MeshShape bodies' prefixes, section runs and tails hand-checked in a separate read-only decode (games/riders-republic docs/recon/meshshape-second-fixture.md, c068d20) |
| `installs.riders/DataPC_MAD_WhiteRoom.forge` | live | - | the base forge of installs.riders/DataPC_MAD_WhiteRoom_patch_01.forge's patch (Riders Republic, Steam build 25394987: `DataPC_MAD_WhiteRoom.forge`, 8,951,822 bytes, 388 entries); repos/anvil-formats/tests/data.rs | the user's own install; two resource blocks' sizes, checksums, names and class ids hand-checked in a separate read-only decode (games/riders-republic docs/recon/archive-reader-fit.md, a9f4959) |
| `installs.riders/DataPC_MAD_WhiteRoom_patch_01.forge` | live | - | one Anvil forge archive of version 27 (Riders Republic, Steam build 25394987: `DataPC_MAD_WhiteRoom_patch_01.forge`, 1,021,872 bytes, 337 entries); repos/anvil-formats/tests/forge.rs | the user's own install; rows hand-checked against a separate read-only census of the file |
| `installs.shaunwhite/DataPC.forge` | live | - | one Anvil forge of version 26 (Shaun White Skateboarding, PC 1.0: `DataPC.forge`, 20,545,833 bytes, 38 entries: the game's bootstrap, settings and localization), its tables, FILEDATA headers and every entry's data blocks (LZO2A) read; repos/anvil-formats/tests/forge.rs, data.rs | the user's own copy; the first two rows (GlobalMetaFile, Graphic Bootstrap) and the second's two blocks hand-checked in a hex dump, and AnvilToolkit 1.3.7's version-25 forge reader and AC2 compression table as the third-party reading of the layout |
| `installs.shaunwhite/DataPC_01.forge` | live | yes | Shaun White Skateboarding PC 1.0 level-01 forge (version 26): every texture in all 253 GridCellDataBlock entries; focused collision shape and inert-component fixtures in four cells; level placements, materials, collision and stored water-flag lowering, harbour tint and placement; 110 omni and 79 spot placements plus the root SkyLight and sun; restored metro mural and CliffDry layer weights; exact metro graffiti wall key for the REF-backed albedo derivation; worlds/shaunwhite/tests/albedo.rs and texture.rs, collision.rs, level.rs, layers.rs | the user's own copy, extracted unchanged; object directory/envelopes hand-checked separately, texture fields cross-checked with AnvilToolkit 1.3.7's AC2/Brotherhood readers and AC2 schema, duplicated compiled metadata and exact mip lengths checked on every texture; BC1/2/3 decoded against third-party bcdec_rs 0.2.0; code 12's BC3 storage established by sizes and normal-map content, not the AC2 enum; collision fields cross-checked with the AC2 schema and hand-inspected inert and multi-inert entity/shape references; light fields checked with the AC2 schema and exact body ends, SkyLight fields with native property descriptors and serializer; physical light units, colour encoding and direction signs remain explicit lowering approximations; restored layer equation from the shipped SDB2 HLSL and Microsoft-disassembled SM3 pixel shader, native material property hashes identify layer tints and alpha switches |

<!-- end of real-data inputs -->
