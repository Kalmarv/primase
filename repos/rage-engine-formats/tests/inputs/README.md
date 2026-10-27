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

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `oracles.codewalker.gta/active.json` | live | yes | optional: an independent reader's live-file tables of that install (JSON; schema in `repos/rage-engine-formats/tests/archive_set.rs`) | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `oracles.codewalker.gta/gtxd.json` | live | yes | optional: an independent reader's texture dictionary parents of that install with their source files (JSON; schema in `repos/rage-engine-formats/tests/gtxd.rs`) | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `oracles.codewalker.gta/listing.jsonl` | live | yes | optional: an independent reader's listing of every archive of that install (JSON lines; schema in `repos/rage-engine-formats/tests/census.rs`) | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `oracles.codewalker.gta/resources` | live | yes | optional: a folder of an independent reader's decodings of chosen drawables and texture dictionaries of that install (JSON; schema in `repos/rage-engine-formats/tests/resource_oracle.rs`) | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `oracles.codewalker.gta/ycd` | live | - | a folder holding `dictionaries.jsonl`: an independent reader's decoding of every clip dictionary (`.ycd`) of that install, the CodeWalker harness's `batch` mode over the install's `.ycd` list (`DumpYcd`: per animation its frames, duration, bone ids, and per sequence block and bone track the channel types and SHA-256 of the channel values and of the evaluated vectors); the channel decoder's values are checked against it (needs `installs.gta`, `keys.gta`); `repos/rage-engine-formats/tests/ycd_channels.rs` | independent: CodeWalker.Core 485d56b (GPL-licensed: run as a tool, no code taken), through a thin dump harness kept with the private oracle tools (`batch` mode) |

#### Debug keys (`GAME_DATA_DEBUG`)

| key | about |
| --- | --- |
| `gta.archive_filter` | only archives whose path contains this text |

<!-- end of real-data inputs -->
