# Havok tagfiles

The reader accepts Havok 2016.2 and 2018.1 `TAG0` object files (`SDK_VERSIONS`)
and `TCM0` external type compendia. A type body's packed member count holds the
field count in its low 16 bits and a count of reflection properties, which store
no records, above them (`Type::properties`; HKLib's 2018 reader reads the word the same way). Class layouts, inheritance and members come from the declared TYPE
metadata; no game-specific offsets or material rules are embedded here.

`Tagfile` retains borrowed DATA with owned type and item indexes. `Value` and
`Array` provide checked views, resolving serialized 32-bit ITEM references on
demand without recursively allocating object graphs. Container sizes, parent
bounds, type references, inheritance cycles, item extents and patch references
are checked. Unknown sections, versions and type flags fail. Optional PTCH is
absent in some empty object states. This is not a complete validator for every
Havok class or unused DATA byte; supported accessors validate the values they
read. Stored nonfinite floats are preserved in unused SIMD lanes; geometry
operations reject nonfinite coordinates.

`compressed_mesh` decodes hknp compressed mesh data. Mesh-tree sections name
their vertex and primitive ranges per SDK version: 2016.2 packs `(first << 8) |
count` into `sharedVertices` and `primitives`, 2018.1 stores
`firstPackedVertexIndex`, `numPackedVertices`, `firstSharedVertexIndex`,
`firstPrimitiveIndex` and `numPrimitives`; `mesh_sections` returns them in one form
(`MeshSection`), as the decoder reads them. The decoder uses the shared vertex toolkit
11/11/10 and 21/21/22 position arithmetic, and handles quad splitting, per-triangle shape
keys, and a counted DEAD primitive sentinel. It returns shape-local geometry;
rigid transforms and game-specific material mappings belong to its caller.
`ConvexCore` retains stored vertices, faces, planes and convex radius. Its
`expanded_triangles` policy builds the intersection of offset half-spaces in
f64, then a closed hull with adaptive exact orientation predicates. It retains
new corners rather than moving each original corner by a least-squares fit.
Output vertices are inside every plane and incident to at least three planes
within 0.01 mm after f32 rounding; sub-nanometre duplicates are welded and
zero-area triangles caused by f32 rounding are omitted. This is a sharp
polytope approximation, not the rounded Havok surface. Capsule cores yield
box approximations. Spheres are not implemented.

`PRIMASE_HAVOK_OBJECT_CORPUS` points to loose self-contained object `.hkx`
files for an independent half-space residual gate. Non-convex classes are
counted separately, not silently claimed as decoded.

Synthetic tests use hand-authored section/type/item records, including pointer
arrays, bad references, oversized items and every truncated fixture prefix.
`PRIMASE_HAVOK_CORPUS` enables the loose-file container and compressed-mesh gate.
The folder contains `.hkx` files and their `.compendium`. The FromSoft raw-soup
gate additionally reads an independent decoder's `<name>.hkx.soup`: little-endian
u32 triangle count, count*9 f32 positions, then count i32 material IDs. No game
assets or oracle binaries are distributed.
