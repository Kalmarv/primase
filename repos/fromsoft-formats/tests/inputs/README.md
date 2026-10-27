# Real-data inputs

What this crate's real-data tests read, as tests/inputs/mod.rs declares them: the keys of the configuration file
`GAME_DATA` names (its layout is primase-testdata's), the oracle sets and the files of each, and the debug keys
`GAME_DATA_DEBUG` sets. A test whose input is not configured skips with a line naming its key.

<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `installs.elden` | - | Elden Ring's `Game` folder (Data0 to Data3, `sd/sd`, DLC archives when installed) | the user's own install |
| `installs.sekiro` | yes | Sekiro's install folder (Data1 to Data5 BHD/BDT pairs, the Oodle 6 DLL) | the user's own install |
| `keys.elden` | - | the user's Elden Ring archive keys, one PKCS#1 `RSA PUBLIC KEY` PEM per archive named after its stem, and `regulation.key`, the key of `regulation.bin` as 64 hexadecimal digits (held to the SHA-256 worlds/fromsoft/src/elden/mod.rs states) | the community unpacker's published keys (UXM's ArchiveKeys.cs) and SoulsFormats' RegulationDecryptor key for the game, written out by a script kept with the game's private tools |
| `keys.sekiro` | yes | the user's Sekiro archive keys: one PKCS#1 `RSA PUBLIC KEY` PEM per archive, Data1.pem to Data5.pem | the community unpacker's published keys (UXM's ArchiveKeys.cs), written out as PEM files |
| `tools.oodle6` | yes | an Oodle version-6 DLL (`oo2core_6_win64.dll`); on Windows only: elsewhere oozextract decodes and nothing is configured | the game's own DLL from the user's install |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

#### Oracle sets (`<oracles.root>/<set>/<game>/`)

| set | about | source |
| --- | --- | --- |
| `oracles.corpus` | the games' own files cut from their installs (shader containers, EBX documents, Havok tagfiles, DCX files, CAS frames) | the user's own installs: files extracted as stored |
| `oracles.soulsformats` | SoulsFormats' readings of FromSoftware archives and files (rigid parts, skins, parts, census, frames, formats) | independent: SoulsFormats 9c0da6b (Sekiro) and SoulsFormatsNEXT ee1dd61 (Elden Ring), through a thin dump harness kept with the private oracle tools |
| `oracles.uxm` | UXM's path dictionaries (`dictionary.txt`, UTF-8, one archive path per line) | third-party: UXM-Selective-Unpack 9501be8's dictionaries |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `oracles.corpus.sekiro/havok` | live | yes | one area's high-resolution map collision as its binder stores it: loose Havok 2016.2 tagfiles and the compendium they share, every body and compressed mesh read; toolkit/havok/tests/corpus.rs, schema in toolkit/havok/README.md | the game's own files from the user's install: worlds/fromsoft/examples/corpus_sekiro_havok.rs (the area's `h*.hkxbhd`/`.hkxbdt` binder, each member out of its DCX envelope) |
| `oracles.corpus.sekiro/havok-soup` | live | yes | an independent decoder's raw triangle soup of each `havok` tagfile, `<name>.hkx.soup`; schema in toolkit/havok/README.md; worlds/fromsoft/tests/collision.rs | independent: a clean-room decoder written from the data alone (no primase decoding code, no third-party code), the primase-oracles harness dumps/havok-soup (5f6dde8, Python standard library) run over `corpus.sekiro/havok`; its README states the format facts and its counts.tsv the per-file triangles |
| `oracles.soulsformats.elden/census` | live | - | an independent census of installs.elden in oracles.soulsformats.sekiro/census's schema (the archives' decrypted headers as `<name>.plain`, entries.tsv, binders.tsv, errors.tsv, samples; hashes 16 hex digits; an archive's shard is its index in UXM's Elden Ring archive list); worlds/fromsoft/tests/census.rs, and asset_collision.rs (the asset collision binders entries.tsv lists) | independent: SoulsFormatsNEXT ee1dd61 with UXM 9501be8's keys, RSA and path hash, the Elden Ring harness's `census` mode (kept with the private oracle tools) |
| `oracles.soulsformats.elden/format` | live | - | an independent reader's field-by-field reading of Elden Ring files read from installs.elden (JSON lines, floats as hex bit patterns): `msb.jsonl` (every map layout: its models, every part with kind, model, transform, entity IDs, shadow controls and hit filter, and its spawn point regions with name, position and rotation), `btl.jsonl` (every light bank: its version and lights, or the reference's refusal), `havok.jsonl` (chosen tiles' map collision and assets' collision: every compressed mesh shape's stored sections, vertices, primitives, shape keys and extern shape instances), `matbin.jsonl` (every material binder's members in member order: shader and source paths, key, every parameter's name, key, type and value, every sampler's type, path, key and two trailing floats), `metaparam.jsonl` (every shader bundle's metaparam: binder, bundle and member names, header words, texture slots, render passes, CAParams and MtdParams); schema in worlds/fromsoft/README.md; worlds/fromsoft/tests/msb_census.rs, btl_census.rs, havok_census.rs, matbin_census.rs, metaparam_census.rs | independent: SoulsFormatsNEXT ee1dd61 with UXM 9501be8's keys and path hash, and HKLib ac40bc9 for Havok, the Elden Ring harness's format modes (kept with the private oracle tools) |
| `oracles.soulsformats.sekiro/census` | live | yes | reference archive outputs; schema in worlds/fromsoft/README.md | independent: SoulsFormats 9c0da6b with UXM's Sekiro dictionary, the dump harness's census mode (kept with the private oracle tools) |
| `oracles.soulsformats.sekiro/materials` | live | yes | independent MTD fields and block lengths (mtd.jsonl, mtd-boundaries.jsonl); schema in worlds/fromsoft/README.md; worlds/fromsoft/tests/mtd_census.rs | independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (`mtd` mode): the fields from its MTD reader, the consumed block lengths from its writer's rewrite of each file |
| `oracles.soulsformats.sekiro/tpf` | live | yes | independent texture-package census (packages.jsonl, area-dds/, errors.tsv); schema in worlds/fromsoft/README.md; worlds/fromsoft/tests/tpf_census.rs, tpf_pixels.rs | independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (`tpf` mode); area-dds/ holds every distinct DDS under `/map/` |
| `oracles.uxm.elden/dictionary.txt` | live | - | caller-provided UTF-8 Elden Ring path dictionary | third-party: UXM's Elden Ring path dictionary |
| `oracles.uxm.sekiro/dictionary.txt` | live | yes | caller-provided UTF-8 Sekiro path dictionary | third-party: UXM's Sekiro path dictionary |

<!-- end of real-data inputs -->
