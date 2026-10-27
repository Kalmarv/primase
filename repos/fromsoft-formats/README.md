# fromsoft-formats

Strict readers of FromSoftware's data: Sekiro: Shadows Die Twice and Elden Ring. The installed archives (BHD5/BDT),
DCX envelopes, BND4 binders, and the formats in them: FLVER models, map layouts (MSB), light banks (BTL), hang edges
(EDGE), event scripts (EMEVD), text (FMG), draw params (GPARAM), material definitions (MTD, MATBIN), shader
metaparams, params (PARAM), animation events (TAE), texture packages (TPF), and collision (Havok tagfiles, through
`havok-formats`, and CLM2). Every read is checked; a refusal names the format and the offset. No key, dictionary, native
library or game file is distributed with this crate: the caller supplies the archive keys of its own install.

## Archives

Read-only FromSoftware archive access (Sekiro, Elden Ring): raw RSA BHD5 envelopes, bucket indexes, AES-128-ECB
entry ranges, DCX DFLT/KRAK envelopes, and BND4 file tables.

What differs between the games and is not stated by their files is data in one descriptor, `Game` (`game.rs`), with
each game's table in its own module (`sekiro`, `elden`): its archive set (`ArchiveSet`: the BHD5/BDT pairs, whether
each is required, the path hash and the BHD5 file-record layout), where it keeps its params and the layout of its NPC
param. Formats are read by what their bytes state.

| | Sekiro | Elden Ring |
|---|---|---|
| pairs (shard) | Data1..Data5 (1..5) | Data0..Data3 (0..3), DLC (4, optional), sd/sd (5), sd/sd_dlc02 (6, optional): UXM's archive list order |
| path hash | base 37, u32 (`path_hash`) | base 133, u64 (`path_hash64`) |
| file record | u32 hash, u32 padded size, u64 offset, SHA and AES offsets, u64 logical size | u64 hash, u32 padded and u32 logical size, u64 offset, SHA and AES offsets |
| keys | supplied by the caller (`Keys::from_folder`: one PKCS#1 PEM per pair, `Data1.pem` ... `Data5.pem`) | supplied by the caller (`Keys::from_folder`: one PKCS#1 PEM per pair, `Data0.pem`, `sd.pem`, ...) |

`Archive::open_game(root, game, &keys)` indexes a game's pairs (an optional pair the install lacks is listed in `absent`). Entry hashes are u64 throughout
(Sekiro's zero-extended). `read_hash(hash, decoder)` and `read(path, decoder)` return decrypted, decoded bytes.
`read_stored(hash)` returns decrypted stored bytes including padding. `Header::read(plain, &record)` and
`unwrap_rsa` expose the lower-level index operations; `Entry::decrypt` applies only its listed ranges. SHA metadata
is parsed and retained, not authenticated.

`Decoder { oodle, max_output }` takes an optional borrowed, caller-loaded Oodle
library and an explicit decompressed-byte ceiling. KRAK needs the native decoder;
DFLT does not. `Binder::read(bytes, &decoder)` returns IDs, names, flags, and
member bytes, including decoded nested DCX. Names are UTF-16 or ASCII; non-ASCII
legacy Shift-JIS names are rejected. Unsupported layouts and unaccounted bytes
are errors. BDT padding for non-DCX entries is retained for reference-reader
parity: a zero BHD logical size does not imply an empty entry.

`NameDictionary::read(path)` reads a caller-supplied UTF-8 Sekiro path list, `read_for(path, game)` any game's,
keyed by its path hash. Hash collisions retain all distinct normalized candidates. No dictionary, native library,
key or game asset is distributed with this crate.
