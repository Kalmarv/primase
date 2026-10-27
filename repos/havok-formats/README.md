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

Synthetic tests use hand-authored section/type/item records, including pointer
arrays, bad references, oversized items and every truncated fixture prefix.
`oracles.corpus.sekiro/havok` enables the loose-file container and
compressed-mesh gate: one area's map collision binder written loose by
`worlds/fromsoft/examples/corpus_sekiro_havok.rs`, its `.hkx` files and their
`.compendium`. The FromSoft raw-soup gate additionally reads
`oracles.corpus.sekiro/havok-soup`, an independent decoder's `<name>.hkx.soup` for
each tagfile: little-endian u32 triangle count, count*9 f32 positions, then count
i32 material IDs, written by the clean-room decoder of the primase-oracles harness
`dumps/havok-soup`. Counts, triangle order and materials compare exactly; positions
within a bound derived from the two decoders' different dequantization arithmetic
(the test states it). No game assets or oracle binaries are distributed.

`cloth` reads the cloth and ragdoll objects of a tagfile by class among its items: `hclClothData` with its
`hclSimClothData` (particles, fixed particles, rest poses, and the link, bend-link, compressible-link and local-range
constraint sets; any other constraint set is kept by its class name), `hknpRagdollData` (its skeleton, bone-to-body map and
body tags) and `hknpConstraintCinfo` (bodies, flags, ids and the class of its constraint data; atoms are not read). A field the
file lacks is an error naming it. The field names are those `repos/fromsoft-formats/tests/havok_census.rs` printed (`CLASS`); its
`PHYSICS` lines are the reader's check against the install.

The `tests-a0b6470ed/havok_census.log` failures split into cloth `null pointer` and ragdoll
`expected integer`. Reference arrays must use the reflected element size: HKLib's tagfile
`PointerFormatHandler` reads UInt64 and `IndexItem` advances by one such element. A four-byte stride
instead visits the first pointer's zero high word as the second pointer. Constraint ids in the 2018
metadata are records (`hknpBodyId.serialAndIndex`, `hkHandle.value` inherited by constraint/group ids),
not integer-format values. The reader follows these named fields and retains primitive integer layouts.
`tests/cloth.rs` supplies synthetic layouts for both SDK versions, two-reference cloth/pose arrays,
particle values, ragdoll tags and record/primitive constraint ids. This is a written regression,
not a claim that the install census has been rerun successfully.

## Packfiles (`packfile`)

`Packfile` reads the older binary *packfile* container (`57E0E057 10C0C010`), version 7, contents version `Havok-7.1.0-r1`,
4-byte pointers, big endian; other versions and layout rules are refused by name and file offset. It checks the section
headers, the class-name table and every local, global and virtual fixup (all in range, class-name offsets valid, no slot with
two fixups); objects are the virtual-fixup targets, and a pointer slot is null exactly when it has no fixup (a slot with none
must hold zero). The level files have an empty `__types__`, so `packfile::physics` states the 7.1 class layouts (rigid body,
MOPP tree, storage extended mesh with its subparts and storages, convex vertices, box, sphere, capsule, cylinder, convex
translate, list) in a table with the check that confirmed each on real data. The mesh view keeps the whole `u32` per-triangle
material word (`MeshMaterial::a`); what its bits mean is a game's choice. `hkpMoppCode`, convex connectivity, constraints and
the container/system classes are recognised by name and counted (`physics::census`), not decoded.
