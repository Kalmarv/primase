# oozextract (primase's copy)

oozextract 0.5.6 (https://github.com/lvlvllvlvllvlvl/oozextract, tag `v0.5.6`, commit 78f0e1d), a Rust Kraken / Mermaid /
Selkie / Leviathan / LZNA / Bitknit decompressor, with the changes below. It is `primase-compress`'s Oodle decoder off
Windows (`[patch.crates-io]` in the workspace manifest); on Windows the game's own library decodes and the gates hold this
copy equal to it. The crate is not a workspace member: its tests run with `cargo test` in this folder.

**Licence.** The upstream manifest declares `license = "MIT"`. The upstream repository and the published package carry no
licence file and no copyright notice, and the port is of the C++ `ooz`; the licence of `ooz` has not been established
here. Asking the author for a licence file is part of the upstream report. Until it answers, treat this folder as
declared-MIT, unverified.

**What differs from 0.5.6** (the sources otherwise are upstream's, so the changes can go back as a pull request):
- `Cargo.toml` is reduced to the library: no `unoodle` binary, benches, wasm package, `cli` feature or `build.rs`;
  `[lints.rust] unexpected_cfgs = "allow"` stands for what `build.rs` declared.
- `src/main.rs`, `benches/`, `wasm-test/`, `perf/` and the CI files are not copied.
- `decode_bytes_type12` (`src/decoder/mod.rs`): a Huffman chunk whose code lengths name a single symbol returns the chunk's
  declared size, not `src - src_end`. 0.5.6 reported such a chunk as using 0 bytes and refused it ("Expected 0 == 3 for
  chunk type 4"); the game's library decodes it. Test: `single_symbol_huffman_chunk_uses_its_declared_size`.
- LZNIB (decoder type 2, block header `8C 02`, `src/algorithm/lznib.rs`): 0.5.6 and the C `ooz` it ports refuse the header
  ("Unknown decoder type 2"). The format is written out in that file's comments; it was derived from 50 streams of
  Sekiro's archives and the game's Oodle 6 library's output for them (all 50 decode to the same bytes), not from
  another decoder's source. Tests: `lznib_commands`, `lznib_refuses_a_match_before_its_history`. Its block size (16 KiB,
  with the two-byte quantum header) and the long length forms are checked against the game library's own compressor 4,
  which writes decoder type 2: 60 inputs of 1 to 600000 bytes, all exact (primase-compress `lznib_cross_check`); and
  Sekiro's 7829 and Elden Ring's 71312 Kraken-framed streams decode to the library's bytes (`krak_archive`).
