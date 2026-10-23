# primase-compress

Small codec adapters implement `Decompressor` and, where supported, `Compressor`.
Every decode takes the exact expected output length. `Lz4` reads raw blocks;
`Deflate` and `Zlib` reject incomplete streams and trailing input.
`ZlibUnterminated` explicitly allows incomplete zlib framing while requiring
all input consumed and the exact output size. Missing checksums cannot establish
integrity.

Frostbite's CAS block streams (`primase_frostbite::cas`) are a container of these codecs and live in the Frostbite world.

With the `reference-decoder` feature, `reference::decode` is oozextract as a second Kraken decoder, for the gates that hold the
game's Oodle library to it (a dev-dependency turns it on).

`oodle::Oodle::load(path, version)` loads exactly the supplied trusted native
library. It is unsafe because library initialization executes native code and
the selected ABI must match the exports. `Version::V5` and `V6` use the legacy
one-argument capacity query; `V9` uses the two-argument query. The DLL must never
be redistributed with this crate. The one-shot Oodle decode ABI returns output
length but does not report consumed input; container framing must supply the
exact compressed slice.

Synthetic tests cover empty input, short/trailing/corrupt input, size mismatches,
and block boundaries. Optional real-data gates read the real-data configuration
(CONTRIBUTING.md): the `tools.oodle5`/`oodle6`/`oodle9` DLLs and the corpus
oracle set. Its `general/codecs.tsv` supports `lz4`, `deflate`, `zlib`, and
`zlib-unterminated` rows, paths relative to the TSV; `tests/oracles/codecs.py`
makes it from game streams the `corpus_*` example programs cut (Sekiro's DFLT
envelopes, skate.'s zlib CAS blocks, Riders Republic's LZ4 terrain blocks, GTA
V's raw deflate archive files), decoded by Python's zlib and the lz4 package. Oodle cross-checks use oozextract (the `reference-decoder` feature, `reference::decode`, enabled by dev-dependencies only)
behind one boundary that makes its refusals and its panics named test errors. The DCX check is in `worlds/fromsoft/tests/krak_gate.rs`
(the production envelope reader, then the DLL against oozextract for the whole stream and every 256 KiB quantum, Kraken or stored).
Its one pinned refusal is 0.5.6's single-symbol Huffman chunk (`SINGLE_SYMBOL_HUFFMAN`), held by an upstream-defect test that
fails once upstream fixes it.
