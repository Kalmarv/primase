# Havok tagfiles

The reader accepts Havok 2016.2 `TAG0` object files and `TCM0` external type
compendia. Class layouts, inheritance and members come from the declared TYPE
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

`compressed_mesh` decodes hknp compressed mesh data with shared vertex toolkit
11/11/10 and 21/21/22 position arithmetic, quad splitting, per-triangle shape
keys, and a counted DEAD primitive sentinel. It returns shape-local geometry;
rigid transforms and game-specific material mappings belong to its caller.
Other shape geometries are not implemented yet.

Synthetic tests use hand-authored section/type/item records, including pointer
arrays, bad references, oversized items and every truncated fixture prefix.
`PRIMASE_HAVOK_CORPUS` enables the loose-file container and compressed-mesh gate.
The folder contains `.hkx` files and their `.compendium`. The FromSoft raw-soup
gate additionally reads an independent decoder's `<name>.hkx.soup`: little-endian
u32 triangle count, count*9 f32 positions, then count i32 material IDs. No game
assets or oracle binaries are distributed.
