# Vertex data

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

`cargo test -p primase-vertex` gates arithmetic against synthetic C# oracle
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
