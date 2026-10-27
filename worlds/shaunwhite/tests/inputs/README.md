# Real-data inputs

What this crate's real-data tests read, as tests/inputs/mod.rs declares them: the keys of the configuration file
`GAME_DATA` names (its layout is primase-testdata's), the oracle sets and the files of each, and the debug keys
`GAME_DATA_DEBUG` sets. A test whose input is not configured skips with a line naming its key.

<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `installs.shaunwhite` | - | Shaun White Skateboarding's (PC, 2010) folder of `.forge` archives, version 26 | the user's own copy: the forges as the release's archives hold them, extracted unchanged |
| `tools.ref_sm3` | yes | the private raw SM3 PS/VS draw/readback host using Microsoft D3D9 REF, with constant/texture controls and --vertex-self-test vertex-input/constant controls | third-party: Microsoft D3D9 reference rasterizer d3dref9.dll 10.0.26100.4654, SHA-256 44cb58f54e11cb6d2cb123dbe92ded3ce3c2b3e2e4d2659c20ddd941b8e3eb51; thin private host, no primase lifter or game equations |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

#### Oracle sets (`<oracles.root>/<set>/<game>/`)

| set | about | source |
| --- | --- | --- |
| `oracles.game-code` | a game's own code run over its install (STUNTBOOST's parsers via reflection: one JSON per level part) | independent: the game's own code via reflection, the oracle tool kept with the game's private tools |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `installs.shaunwhite/DataPC_01.forge` | live | yes | Shaun White Skateboarding PC 1.0 level-01 forge (version 26): every texture in all 253 GridCellDataBlock entries; focused collision shape and inert-component fixtures in four cells; level placements, materials, collision and stored water-flag lowering, harbour tint and placement; 110 omni and 79 spot placements plus the root SkyLight and sun; restored metro mural and CliffDry layer weights; exact metro graffiti wall key for the REF-backed albedo derivation; worlds/shaunwhite/tests/albedo.rs and texture.rs, collision.rs, level.rs, layers.rs | the user's own copy, extracted unchanged; object directory/envelopes hand-checked separately, texture fields cross-checked with AnvilToolkit 1.3.7's AC2/Brotherhood readers and AC2 schema, duplicated compiled metadata and exact mip lengths checked on every texture; BC1/2/3 decoded against third-party bcdec_rs 0.2.0; code 12's BC3 storage established by sizes and normal-map content, not the AC2 enum; collision fields cross-checked with the AC2 schema and hand-inspected inert and multi-inert entity/shape references; light fields checked with the AC2 schema and exact body ends, SkyLight fields with native property descriptors and serializer; physical light units, colour encoding and direction signs remain explicit lowering approximations; restored layer equation from the shipped SDB2 HLSL and Microsoft-disassembled SM3 pixel shader, native material property hashes identify layer tints and alpha switches |
| `installs.shaunwhite/DataPC_02.forge` | live | yes | Shaun White Skateboarding PC 1.0 shared level-02 objects referenced by level 01; worlds/shaunwhite/tests/level.rs | the user's own copy, extracted unchanged; source object ids resolve mesh, texture and collision references across forges |
| `installs.shaunwhite/DataPC_03.forge` | live | yes | Shaun White Skateboarding PC level 03, material-key and SDB2 binding coverage; worlds/shaunwhite/tests/resolve_shaders.rs | the user's own copy; material key constraints from native property descriptors and setters, hand-worked key/record fixtures; shader reflection from the stored SM3 programs |
| `installs.shaunwhite/DataPC_04.forge` | live | yes | Shaun White Skateboarding PC 1.0 shared level-04 objects referenced by level 01; worlds/shaunwhite/tests/level.rs | the user's own copy, extracted unchanged; source object ids resolve mesh, texture and collision references across forges |
| `installs.shaunwhite/DataPC_05.forge` | live | yes | Shaun White Skateboarding PC level 05, material-key and SDB2 binding coverage; worlds/shaunwhite/tests/resolve_shaders.rs | the user's own copy; material key constraints from native property descriptors and setters, hand-worked key/record fixtures; shader reflection from the stored SM3 programs |
| `installs.shaunwhite/DataPC_06.forge` | live | yes | Shaun White Skateboarding PC 1.0 level-06 masked wall and layered material lowering in the restored state; worlds/shaunwhite/tests/layers.rs | the user's own copy; layer equation established from the shipped HLSL and compiled D3D9 pixel shader, material fields identified by native property hashes |
| `installs.shaunwhite/DataPC_extra.forge` | live | yes | Shaun White Skateboarding PC 1.0 standalone textures: all 48 TextureMapSpec entries and their owned TextureMaps; worlds/shaunwhite/tests/texture.rs | the user's own copy, extracted unchanged; same field/layout checks as DataPC_01.forge and all stored mip levels decoded against third-party bcdec_rs 0.2.0 |
| `installs.shaunwhite/ekshadersPChigh.bin` | live | yes | Shaun White Skateboarding's uncompressed high-quality SDB2 v39 database: source CRCs and identities, all raw stage programs, pass metadata tails, state-only and missing records; named material-only RGB derivation and full-PS REF proof for basic/layered/metro-compatible records; worlds/shaunwhite/tests/shaders.rs and albedo.rs | the user's own install; hand-worked fixture and native version-39 loader establish framing; record 5's PS is also checked against Microsoft's disassembly in repos/iridia/iridia-d3d9/tests/disassembly.rs |
| `installs.shaunwhite/ekshadersPClow.bin` | live | yes | Shaun White Skateboarding's uncompressed low-quality SDB2 v39 database, every record and metadata tail; worlds/shaunwhite/tests/shaders.rs | the user's own install; hand-worked fixture and native version-39 loader establish framing |
| `installs.shaunwhite/ekshadersPCmed.bin` | live | yes | Shaun White Skateboarding's uncompressed medium-quality SDB2 v39 database, every record and metadata tail; worlds/shaunwhite/tests/shaders.rs | the user's own install; hand-worked fixture and native version-39 loader establish framing |
| `oracles.game-code.shaunwhite/uv-rotation.tsv` | live | yes | Original native UV setter inputs and six written matrix words (float32 hex): controls and the stored BlueRusty material's slots; worlds/shaunwhite/tests/albedo.rs and material_bindings.rs | independent: original executable SHA256 c7eb65c89ab016694a2f60a5f586c451a5a02bf05f226bffc943d256128d35c4, original function at 0x00cd7590 SHA256 335612ce548156ea10272422486f40db1958cad4d32315272968185eef0d320d, executed in a private x86 host without game initialization; original HIGHLOW relocations applied, untouched row padding checked by canaries; full PS independently executed by Microsoft D3D9 REF |

<!-- end of real-data inputs -->
