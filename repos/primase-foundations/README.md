# primase-foundations

The plumbing the primase format crates share, for reading game files safely: checked byte reading (`bytes`), the
hashes game files key names by (`hash`), vertex layouts and quantization (`vertex`), an ordered parallel map
(`parallel`), and, behind the `compress` feature, the compression codecs games store (`compress`).

## compress

Small codec adapters implement `Decompressor` and, where supported, `Compressor`.
Every decode takes the exact expected output length. `Lz4` reads raw blocks;
`Deflate` and `Zlib` reject incomplete streams and trailing input.
`ZlibUnterminated` explicitly allows incomplete zlib framing while requiring
all input consumed and the exact output size. Missing checksums cannot establish
integrity.

Frostbite's CAS block streams (`primase_frostbite::cas`) are a container of these codecs and live in the Frostbite world.

Off Windows `oodle::Oodle::open(None, version)` (`Oodle::portable`) decodes with oozextract, which refuses by name a stream
it cannot decode and cannot compress; on Windows the game's library is the codec. With the `reference-decoder` feature
(always there off Windows), `reference::decode` is oozextract as a second Kraken decoder, for the gates that hold the
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

## vertex

All arithmetic preserves the documented f32 operation order, without fused
multiply-add or an implicit coordinate-frame conversion. Unknown formats,
non-finite decoded values, invalid indices, and out-of-bounds streams are errors.

- `Format`: DXGI float2/3/4, half2/4, SNORM16x2/x4, UNORM16x2,
  UNORM8x4 and UNORM10x3/A2 elements. Missing components decode as zero.
- `resolve_layout`, `decode_stream`: explicit offsets and independent per-slot
  append cursors. The reference append cursor sums element sizes even when an
  element has an explicit offset; this differs from D3D append semantics.
- `snorm16`, `scale_translate`, `transform_uv`, `split_normal`, `tangent`:
  clamped signed quantization, per-axis transforms, split normal components,
  and packed tangent direction/handedness. UV transforms flip V explicitly.
- `packed_position32` / `packed_position64`: section-scaled 11/11/10 and
  box-relative 21/21/22 coordinates.
- `strip_to_list`: alternating winding with duplicate-index joints omitted;
  degenerate joints still advance parity. No primitive-restart sentinel.
- `index_width`: u16 through index 65535, u32 above it.
- `split_triangles`: first-use remapping and the reference 65,535-vertex piece
  limit. Reserves three slots before each triangle, even when vertices are reused.

`cargo test -p primase-foundations` gates arithmetic against synthetic C# oracle
outputs: all signed 16-bit values, all half bit patterns (non-finite results
rejected), seeded packed tangent and quantization cases. Layout bounds and
triangle-strip/chunk boundary behavior have explicit regression tests.

Fixture streams are little-endian. `arithmetic.bin` stores 65,536 f32 signed
normalization outputs in signed-input order, then 4,096 records of packed u32
input and four f32 tangent outputs (LCG seed 0x31415926, multiplier 1664525,
increment 1013904223). `quantization.bin` begins with a u32 case count; each
record has two u32 inputs, then normal[3], UV[2], scaled position[3], packed
32-bit position[3], and packed 64-bit position[3] as f32. It ends with all 65,536
half-to-f32 outputs in bit-pattern order. Quantization seed is 0x1234 with the
same LCG. Constants and expected operation order are explicit in the gate test.
Only synthetic inputs and oracle outputs are included; no external mesh data.

`split_by_vertex_budget` exposes the same chunk traversal for budgets from 3 to
65,535 (including the render-piece budget of 60,000). `renormalize_stored_normal`
preserves lengths within 1e-3 of one and otherwise uses component division, with
+Z fallback for lengths <=1e-12. `stored-normals.bin` has a u32 count followed by
input/output f32 triples; it covers threshold cases and seeded vectors.
