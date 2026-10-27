# rage-engine-formats

Readers for the RAGE engine's file formats as GTA V Enhanced stores them. Bytes (or, for archives, paths) in, plain
records out; every read is bounded, and a malformed file is an `Error`, never a panic.

| module | what it reads |
|---|---|
| `archive` | RPF7 archives with their NG and AES ciphers (`rpf`, `ng`), the install's archives and which one holds each file's live copy (`archive_set`), and its listing (`listing`) |
| `mount` | the game's mount and setup data: which packs and archives are mounted, and the change sets that enable and disable them |
| `resource` | RSC7 resources: page flags, inflation, bounded block reads |
| `meta`, `map`, `ped` | META documents, and the map data (ymap, ytyp) and ped metadata read from them |
| `drawable`, `texture`, `sga` | drawables and fragments, texture dictionaries, buffer formats and their pixel layout |
| `bounds` | collision bounds (ybn) |
| `ycd`, `ycd_channels` | clip dictionaries and their per-bone channel tracks, decoded |
| `fxc`, `sgd2`, `sgd2_effect`, `sgd2_table` | compiled effects, and the SGD2 shader database: programs, effects, parameter tables |
| `xml`, `rbf`, `gtxd` | the game's XML data files, their binary form (RBF), and the texture-dictionary parent list |

## Keys

No key material, and nothing derived from the game's executable, is part of this crate. The archive ciphers need the
NG keys and decrypt tables, the name-hash lookup table and, for AES archives, the AES key: the caller supplies them
(`archive::keys::Keys::from_parts`, `from_dir`) and may pass the SHA-1 digests the material must match
(`archive::keys::KeyDigests`).

## Tests

Synthetic tests run anywhere. The census tests read a GTA V install and the key material and skip, printing why,
without them.
