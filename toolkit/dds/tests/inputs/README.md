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
| `oracles.scripts` | measurements by our own scripts over game data, separate from the readers they check (normal curl, texture statistics, shadow and state records, the eclipse reference) | our own scripts kept with the private oracle tools: separate from the readers under test, not third-party; some producers are not recorded yet |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `oracles.scripts.sekiro/pbr` | live | yes | source DDS in `census/dds` plus independent `texture-statistics.json` channel extrema and means; `worlds/fromsoft/tests/pbr_census.rs` | the install's own DDS with statistics computed by a script kept with the private oracle tools |

<!-- end of real-data inputs -->
