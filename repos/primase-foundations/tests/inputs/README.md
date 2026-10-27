# Real-data inputs

What this crate's real-data tests read, as tests/inputs/mod.rs declares them: the keys of the configuration file
`GAME_DATA` names (its layout is primase-testdata's), the oracle sets and the files of each, and the debug keys
`GAME_DATA_DEBUG` sets. A test whose input is not configured skips with a line naming its key.

<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `tools.oodle5` | yes | an Oodle version-5 DLL (`oo2core_5_win64.dll`); on Windows only: elsewhere oozextract decodes and nothing is configured | the game's own DLL from the user's install |
| `tools.oodle6` | yes | an Oodle version-6 DLL (`oo2core_6_win64.dll`); on Windows only: elsewhere oozextract decodes and nothing is configured | the game's own DLL from the user's install |
| `tools.oodle9` | yes | an Oodle version-9 DLL (`oo2core_9_win64.dll`); on Windows only: elsewhere oozextract decodes and nothing is configured | the game's own DLL from the user's install |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

#### Oracle sets (`<oracles.root>/<set>/<game>/`)

| set | about | source |
| --- | --- | --- |
| `oracles.corpus` | the games' own files cut from their installs (shader containers, EBX documents, Havok tagfiles, DCX files, CAS frames) | the user's own installs: files extracted as stored |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `oracles.corpus.general/codecs.tsv` | live | yes | TSV: codec, compressed path, oracle decoded path (tab-separated) | independent: the games' own streams (Sekiro's DFLT DCX envelopes, skate.'s zlib CAS blocks, Riders Republic's terrain LZ4 blocks, GTA V's raw deflate archive files, cut by the corpus_* example programs) decoded by Python's zlib and the lz4 package 4.4.5, repos/primase-foundations/tests/oracles/codecs.py |

#### Debug keys (`GAME_DATA_DEBUG`)

| key | about |
| --- | --- |
| `oodle.dump` | a folder under the scratch root: the Oodle gate writes there the payload and the game library's output of each stream oozextract refuses or decodes differently |

<!-- end of real-data inputs -->
