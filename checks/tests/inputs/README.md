# Real-data inputs

What this crate's real-data tests read, as tests/inputs/mod.rs declares them: the keys of the configuration file
`GAME_DATA` names (its layout is primase-testdata's), the oracle sets and the files of each, and the debug keys
`GAME_DATA_DEBUG` sets. A test whose input is not configured skips with a line naming its key.

<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `installs.elden` | - | Elden Ring's `Game` folder (Data0 to Data3, `sd/sd`, DLC archives when installed) | the user's own install |
| `installs.gta` | yes | a GTA V install folder (Enhanced or Legacy: the executable and the `.rpf` archives) | the user's own install |
| `installs.riders` | - | Riders Republic's folder of `.forge` archives (Steam build 25394987) | the user's own install |
| `installs.sekiro` | yes | Sekiro's install folder (Data1 to Data5 BHD/BDT pairs, the Oodle 6 DLL) | the user's own install |
| `installs.skate` | yes | the skate. install the ReSkate tools use (`Data`: its `layout.toc`, TOCs and CAS archives; the Oodle 9 DLL) | the user's own install |
| `installs.skatebird` | yes | SkateBIRD's install folder (`SkateBIRD_Data`: a Unity 2019.4.20f1 player) | the user's own install |
| `installs.sledding` | yes | Sledding Game's install folder (`Sledding Game_Data`: a Unity 6000.3 player) | the user's own install |
| `installs.stuntboost` | yes | STUNTBOOST's install folder (Steam buildid 25832744: `Content/Models/Export`) | the user's own install |
| `keys.elden` | - | the user's Elden Ring archive keys, one PKCS#1 `RSA PUBLIC KEY` PEM per archive named after its stem, and `regulation.key`, the key of `regulation.bin` as 64 hexadecimal digits (held to the SHA-256 worlds/fromsoft/src/elden/mod.rs states) | the community unpacker's published keys (UXM's ArchiveKeys.cs) and SoulsFormats' RegulationDecryptor key for the game, written out by a script kept with the game's private tools |
| `keys.gta` | yes | the user's GTA V archive key material in the community layout (`gtav_ng_key.dat`, `gtav_ng_decrypt_tables.dat`, `gtav_hash_lut.dat`, optional `gtav_aes_key.dat`); never committed | the user's own key material, checked against published digests |
| `keys.sekiro` | yes | the user's Sekiro archive keys: one PKCS#1 `RSA PUBLIC KEY` PEM per archive, Data1.pem to Data5.pem | the community unpacker's published keys (UXM's ArchiveKeys.cs), written out as PEM files |
| `tools.oodle6` | yes | an Oodle version-6 DLL (`oo2core_6_win64.dll`); on Windows only: elsewhere oozextract decodes and nothing is configured | the game's own DLL from the user's install |
| `tools.oodle9` | yes | an Oodle version-9 DLL (`oo2core_9_win64.dll`); on Windows only: elsewhere oozextract decodes and nothing is configured | the game's own DLL from the user's install |
| `tools.unity_tpk` | yes | a type tree package (TPK, uncompressed or LZ4) with the class layouts for the Unity players read | input data: class layouts for the player's Unity version (repacked with a script kept with the private oracle tools) |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

#### Oracle sets (`<oracles.root>/<set>/<game>/`)

| set | about | source |
| --- | --- | --- |
| `oracles.baseline` | our own earlier outputs kept to detect change (an earlier exporter's scene, a map our writer built, a frozen stage result) | regression baselines: our own earlier outputs, not independent oracles |
| `oracles.uxm` | UXM's path dictionaries (`dictionary.txt`, UTF-8, one archive path per line) | third-party: UXM-Selective-Unpack 9501be8's dictionaries |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `installs.skate/Data` | live | yes | the skate. install's `Data` folder (its `layout.toc`, TOCs and CAS archives), read only | the user's own install |
| `installs.skatebird/SkateBIRD_Data` | live | yes | SkateBIRD's data folder (`SkateBIRD_Data`, a Unity 2019.4.20f1 player: SerializedFile version 21 containers), read with `tools.unity_tpk`; `worlds/unity/tests/serialized_census.rs`, `worlds/skatebird/tests/markers.rs` | the user's own install |
| `installs.sledding/Sledding Game_Data` | live | yes | a Unity player's data folder (`<Game>_Data`, holding `globalgamemanagers` and the `levelN` / `*.assets` files) | the user's own install |
| `oracles.baseline.sekiro/scene` | live | yes | our earlier exporter's skate-scene folder of an area: scene.json (region, materials), its collision tables (a regression baseline, not an independent reference) and textures; checks/tests/web_texture_decode.rs, worlds/fromsoft/tests/material_assignments.rs, shadow_census.rs | regression baseline: our earlier C# exporter's scene of the area, not an independent reference; placements are checked against the SoulsFormats dump instead |
| `oracles.uxm.sekiro/dictionary.txt` | live | yes | caller-provided UTF-8 Sekiro path dictionary | third-party: UXM's Sekiro path dictionary |

<!-- end of real-data inputs -->
