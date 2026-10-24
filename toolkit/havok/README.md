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
keys, and a counted DEAD primitive sentinel. A primitive whose second and fourth
indices are equal is a custom primitive (`CustomPrimitive`): its first index names
its data in the shared-vertex index, `[146, w, low, high]` an extern shape
(`CustomKind::Extern`), records whose first word ends in byte 0x52 a kind not read
(`CustomKind::Unread`), any other an error. `extern_shapes` reads a compressed mesh
shape's `externShapes` instances (transform columns, scale one) and checks each is
referenced exactly once; a 2016.2 shape has no such field. It returns shape-local geometry;
rigid transforms and game-specific material mappings belong to its caller.
`ConvexCore` retains stored vertices, faces, planes and convex radius. Its
`expanded_triangles` policy builds the intersection of offset half-spaces in
f64, then a closed hull with adaptive exact orientation predicates. It retains
new corners rather than moving each original corner by a least-squares fit.
Output vertices are inside every plane and incident to at least three planes
within 0.01 mm after f32 rounding; sub-nanometre duplicates are welded and
zero-area triangles caused by f32 rounding are omitted. This is a sharp
polytope approximation, not the rounded Havok surface. Capsule cores yield
box approximations; a sphere's core is its centre (one vertex, no planes), tessellated as
the cube of half-edge radius around it, the same sharp approximation. A core of zero thickness (two
opposite planes whose offsets agree within the plane bound, every vertex on them)
is a polygon: its two faces offset by the radius. `rounded_triangles(max_error)` is
the rounded surface Havok gives the shape (the core swept by a sphere of the convex
radius) within a stated error: the hull of every core vertex plus every vertex of an
icosphere inscribed in the radius, subdivided until each of its faces lies within the
error of the sphere; every output vertex is on the true surface, so the result lies
inside the shape and within the error of every point of it. An `hknpConvexShape` stores
vertices only: its core is their hull (one triangle a face), or, for vertices within
the plane bound of one plane, their convex polygon as two faces.

`oracles.corpus.sekiro/object-hkx` (the real-data configuration,
CONTRIBUTING.md) holds loose self-contained object `.hkx` files for an
independent half-space residual gate. Non-convex classes are counted
separately, not silently claimed as decoded.

Synthetic tests use hand-authored section/type/item records, including pointer
arrays, bad references, oversized items and every truncated fixture prefix.
`oracles.corpus.sekiro/havok` enables the loose-file container and
compressed-mesh gate: one area's map collision binder written loose by
`worlds/fromsoft/examples/corpus_sekiro_havok.rs`, its `.hkx` files and their
`.compendium`. The FromSoft raw-soup gate additionally reads
`oracles.corpus.sekiro/havok-soup`, an independent decoder's `<name>.hkx.soup` for
each tagfile: little-endian u32 triangle count, count*9 f32 positions, then count
i32 material IDs. No decoder independent of this one exists yet, so that set is
blocked. No game assets or oracle binaries are distributed.
