# primase-compress

Small codec adapters implement `Decompressor` and, where supported, `Compressor`.
Every decode takes the exact expected output length. `Lz4` reads raw blocks;
`Deflate` and `Zlib` reject incomplete streams and trailing input.
`ZlibUnterminated` explicitly allows incomplete zlib framing while requiring
all input consumed and the exact output size. Missing checksums cannot establish
integrity.

`cas::blocks` inspects Frostbite CAS headers. `cas::Cas` decodes stored, zlib,
and Oodle kinds 0x11, 0x15 and 0x19, with a caller-supplied Oodle handle.
`cas::CasRaw` writes stored 64 KiB blocks. `Cas` compression requires Oodle and
writes Kraken level 4 with stored blocks when compression is not smaller.

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
V's raw deflate archive files), decoded by Python's zlib and the lz4 package. Oodle cross-checks use oozextract only as
a development dependency, behind one boundary that makes its refusals and its
panics named test errors. The DCX check compares the whole stream and every
256 KiB quantum (Kraken or stored) with the DLL. Its one pinned refusal is
0.5.6's single-symbol Huffman chunk (`dcx_gate::SINGLE_SYMBOL_HUFFMAN`), held by
an upstream-defect test that fails once upstream fixes it.
