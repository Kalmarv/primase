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
| `oracles.corpus` | the games' own files cut from their installs (shader containers, EBX documents, Havok tagfiles, DCX files, CAS frames) | the user's own installs: files extracted as stored |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `oracles.corpus.sekiro/havok` | live | yes | one area's high-resolution map collision as its binder stores it: loose Havok 2016.2 tagfiles and the compendium they share, every body and compressed mesh read; toolkit/havok/tests/corpus.rs, schema in toolkit/havok/README.md | the game's own files from the user's install: worlds/fromsoft/examples/corpus_sekiro_havok.rs (the area's `h*.hkxbhd`/`.hkxbdt` binder, each member out of its DCX envelope) |
| `oracles.corpus.sekiro/object-hkx` | live | yes | a folder of Havok tagfiles holding object convex bodies, checked against their stored half-spaces; `toolkit/havok/tests/convex_corpus.rs` | the game's own tagfiles, checked against their own stored half-spaces |

<!-- end of real-data inputs -->
