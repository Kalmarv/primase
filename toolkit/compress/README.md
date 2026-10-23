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
`zlib-unterminated` rows. Oodle cross-checks use oozextract only as
a development dependency. Its 0.5.5 Kraken restart defect has a synthetic
regression test; DCX checks report blocked whole-stream comparisons separately
from per-quantum comparisons and native round trips.
