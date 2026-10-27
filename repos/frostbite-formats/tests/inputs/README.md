# Real-data inputs

What this crate's real-data tests read, as tests/inputs/mod.rs declares them: the keys of the configuration file
`GAME_DATA` names (its layout is primase-testdata's), the oracle sets and the files of each, and the debug keys
`GAME_DATA_DEBUG` sets. A test whose input is not configured skips with a line naming its key.

<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `installs.skate` | yes | the skate. install the ReSkate tools use (`Data`: its `layout.toc`, TOCs and CAS archives; the Oodle 9 DLL) | the user's own install |
| `tools.oodle5` | yes | an Oodle version-5 DLL (`oo2core_5_win64.dll`); on Windows only: elsewhere oozextract decodes and nothing is configured | the game's own DLL from the user's install |
| `tools.oodle6` | yes | an Oodle version-6 DLL (`oo2core_6_win64.dll`); on Windows only: elsewhere oozextract decodes and nothing is configured | the game's own DLL from the user's install |
| `tools.oodle9` | yes | an Oodle version-9 DLL (`oo2core_9_win64.dll`); on Windows only: elsewhere oozextract decodes and nothing is configured | the game's own DLL from the user's install |
| `tools.reskate_cli` | - | ReSkate Studio's CLI executable (`reskate_cli`), an independent EBX reader and reference writer | third-party: ReSkate Studio's CLI (20261012) |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

#### Oracle sets (`<oracles.root>/<set>/<game>/`)

| set | about | source |
| --- | --- | --- |
| `oracles.baseline` | our own earlier outputs kept to detect change (an earlier exporter's scene, a map our writer built, a frozen stage result) | regression baselines: our own earlier outputs, not independent oracles |
| `oracles.corpus` | the games' own files cut from their installs (shader containers, EBX documents, Havok tagfiles, DCX files, CAS frames) | the user's own installs: files extracted as stored |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `installs.skate/Data` | live | yes | the skate. install's `Data` folder (its `layout.toc`, TOCs and CAS archives), read only | the user's own install |
| `oracles.baseline.skate/fbmod` | live | - | a folder of `.fbmod` files (searched recursively); each is read, written again byte for byte and every payload decoded (`tools.oodle9` for Oodle blocks); repos/frostbite-formats/tests/fbmod.rs. The costume files among them (top level, with Studio's `<name>.readback.glb` beside them when present) also gate the costume steps with `installs.skate/Data`: worlds/reskate/tests/costume.rs | ReSkate Studio 2026-10-12 `cosmetic-mesh-import` output: our own producer's files, a round-trip baseline, not an oracle |
| `oracles.corpus.skate/cas` | live | yes | directory of indexed individual `.casblock` frames | the game's own CAS frames from the user's install: worlds/reskate/examples/corpus_skate_cas.rs (one Oodle block per frame, 16 entries per superbundle TOC) |
| `oracles.corpus.skate/ebx` | live | - | a folder of EBX documents as the game wrote them, mirroring their asset paths (searched recursively); every field's value must survive a rewrite, and the byte layout is compared with the reference writer; repos/frostbite-formats/tests/ebx.rs | the game's own EBX documents |

<!-- end of real-data inputs -->
