# Real-data inputs

What this crate's real-data tests read, as tests/inputs/mod.rs declares them: the keys of the configuration file
`GAME_DATA` names (its layout is primase-testdata's), the oracle sets and the files of each, and the debug keys
`GAME_DATA_DEBUG` sets. A test whose input is not configured skips with a line naming its key.

<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `installs.gta` | yes | a GTA V install folder (Enhanced or Legacy: the executable and the `.rpf` archives) | the user's own install |
| `keys.gta` | yes | the user's GTA V archive key material in the community layout (`gtav_ng_key.dat`, `gtav_ng_decrypt_tables.dat`, `gtav_hash_lut.dat`, optional `gtav_aes_key.dat`); never committed | the user's own key material, checked against published digests |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

#### Oracle sets (`<oracles.root>/<set>/<game>/`)

| set | about | source |
| --- | --- | --- |
| `oracles.codewalker` | CodeWalker's readings of a GTA V install (archive listings, live-file tables, texture parents, resources, frames, bounds) | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `oracles.corpus` | the games' own files cut from their installs (shader containers, EBX documents, Havok tagfiles, DCX files, CAS frames) | the user's own installs: files extracted as stored |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `oracles.codewalker.gta/ShadersGen9Conversion.xml` | live | - | optional: CodeWalker's `ShadersGen9Conversion.xml`, which names every effect's texture and constant parameters (current and previous name, constant-buffer offset); `worlds/gta/tests/program_tables.rs` holds the shader database's parameter records to it | independent: CodeWalker.Core 485d56b, a data file of its repository (CodeWalker.Core/ShadersGen9Conversion.xml) |
| `oracles.codewalker.gta/bounds` | live | yes | props' embedded collision bounds (a ydr's own, a yft's first physics LOD's) as an independent reader (CodeWalker) reads them: a folder of JSON files, one per prop, `{path, leaves: [{type, flags, polygons: [{type, material, vertices (placed)}] or corners (8, placed)}]}`; `lower-gta-embedded-collision`'s triangles are checked against them (needs `installs.gta`, `keys.gta`); `worlds/gta/tests/embedded_bounds.rs` | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `oracles.codewalker.gta/frames.json` | live | yes | one ydr's high-detail geometries as an independent reader (CodeWalker) reads them, as stored in GTA's frame: one JSON file `{path, value: {models: [{geometries: [{positions, normals, tangents (xyzw), texcoords, indices}]}]}}`; the frame change, triangle fronts and bitangent sign are checked against it (needs `installs.gta`, `keys.gta`), and its stored tangent frames against the frames its texture coordinates give; `worlds/gta/tests/handedness.rs`, `uv_frames.rs` | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `oracles.codewalker.gta/parity` | live | - | optional: a folder holding `dump.jsonl`, an independent reader's decoding of every file the gate regions' placements reach (the CodeWalker harness's `batch` mode over the list `parity_list` writes; schema in `worlds/gta/tests/parity.rs`) | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools (`batch` mode, written when needed) |
| `oracles.codewalker.gta/resources` | live | yes | optional: a folder of an independent reader's decodings of chosen drawables and texture dictionaries of that install (JSON; schema in `repos/rage-engine-formats/tests/resource_oracle.rs`) | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `oracles.corpus.gta/legacy-effects` | unheld: it needs GTA V Legacy's install, which no machine holds (the Enhanced edition has no effect files) | - | a folder (searched recursively) of GTA V Legacy's compiled effects (`.fxc`, RAGE `rgxe`) as extracted from the install (the Enhanced edition has none); every one is read, its programs' containers checked and lifted; worlds/gta/tests/fxc_census.rs | the user's own install: the effect files extracted from its archives |
| `oracles.corpus.gta/shader-database.awc` | live | - | GTA V Enhanced's shader database (`sga_win32_60_final.awc`, magic `SGD2`, from update/update2.rpf `common/shaders/fxdb`) as the install stores it after decryption and decompression: every program and effect is read, every program lifted; worlds/gta/tests/fxc_census.rs | the user's own install: the file extracted from its archive, worlds/gta/examples/corpus_gta_shader_database.rs |

#### Debug keys (`GAME_DATA_DEBUG`)

| key | about |
| --- | --- |
| `gta.archive_filter` | only archives whose path contains this text |
| `gta.region` | only this named gate region of worlds/gta/tests/region_pipeline.rs |

<!-- end of real-data inputs -->
