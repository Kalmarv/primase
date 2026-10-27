# Real-data inputs

What this crate's real-data tests read, as tests/inputs/mod.rs declares them: the keys of the configuration file
`GAME_DATA` names (its layout is primase-testdata's), the oracle sets and the files of each, and the debug keys
`GAME_DATA_DEBUG` sets. A test whose input is not configured skips with a line naming its key.

<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `installs.shred` | yes | Tony Hawk: SHRED's (PS3, 2010) disc folder: `PS3_GAME/USRDIR` with its PAK archives | the user's own disc, extracted unchanged |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

#### Oracle sets (`<oracles.root>/<set>/<game>/`)

| set | about | source |
| --- | --- | --- |
| `oracles.corpus` | the games' own files cut from their installs (shader containers, EBX documents, Havok tagfiles, DCX files, CAS frames) | the user's own installs: files extracted as stored |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `installs.shred/PS3_GAME/USRDIR/database/main.db` | live | yes | Tony Hawk: SHRED's SQLite 3 database of levels, sessions, goals and drop spots (142 tables); worlds/shred/tests/main_db.rs | the user's own disc, extracted unchanged |
| `oracles.corpus.shred/scripts.pak` | live | yes | SHRED's scripts archive, decrypted: the PAK (version 4) inside the disc's `SCRIPTS.SDAT`, whose `script\*.lua` entries are Lua 5.1 bytecode (1,278 scripts, 1,459,346 bytes); worlds/shred/tests/level_params_disc.rs | the user's own disc's SCRIPTS.SDAT decrypted outside primase by RPCS3's own decryptor (`rpcs3 --headless --decrypt`, no key in primase); the plaintext PAK is used unchanged |

<!-- end of real-data inputs -->
