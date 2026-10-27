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
| `oracles.rpcs3` | RPCS3's RSX shader disassembly, produced by an external batch driver | independent: RPCS3's original TaskFP at 979a52415003fd7a74ddd216e1a5782b3731281f, built as a tool; driver and unchanged upstream decoding source in private modding/tools/rpcs3-cg-oracle |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `installs.shred/PS3_GAME/USRDIR/database/main.db` | live | yes | Tony Hawk: SHRED's SQLite 3 database of levels, sessions, goals and drop spots (142 tables); worlds/shred/tests/main_db.rs | the user's own disc, extracted unchanged |
| `oracles.corpus.shred/main-db.tsv` | live | yes | TSV of every table of SHRED's MAIN.DB (sqlite_master first): `@table` and its column names, then one line per row, its rowid and typed values (`n`, `i:`, `r:` big-endian double, `t:` UTF-8 and `b:` as hex); worlds/shred/tests/main_db.rs | independent: Python's standard-library sqlite3 (SQLite's own C library) reading the database read-only, worlds/shred/tests/oracles/sqlite_dump.py |
| `oracles.corpus.shred/scripts.pak` | live | yes | SHRED's scripts archive, decrypted: the PAK (version 4) inside the disc's `SCRIPTS.SDAT`, whose `script\*.lua` entries are Lua 5.1 bytecode (1,278 scripts, 1,459,346 bytes); worlds/shred/tests/level_params_disc.rs | the user's own disc's SCRIPTS.SDAT decrypted outside primase by RPCS3's own decryptor (`rpcs3 --headless --decrypt`, no key in primase); the plaintext PAK is used unchanged |
| `oracles.rpcs3.shred/fragment` | live | yes | per-microcode SHA256 fragment disassembly plus occurrence index: 3267 distinct microcodes covering all 181851 packaged fragment programs; worlds/shred/tests/fragment_disc.rs | independent: official RPCS3 979a52415003fd7a74ddd216e1a5782b3731281f TaskFP, run by private modding/tools/rpcs3-cg-oracle (build.ps1, build_driver.py, provenance.json); represented operands and control boundaries compared, omitted modifiers and sampling operands checked with hand-worked fixtures; disagreements are settled with fixtures rather than deferring to this tool |

<!-- end of real-data inputs -->
