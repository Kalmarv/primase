# Web tile sets

`write(&World, &Options)` produces deterministic GLBs and a `tiles.json` index;
`TileSet::write_to` writes them to a directory. Load mesh/texture files and lower
materials to PBR before export. The world uses metres and +Y up. Ground tiles
use floor(X/tile_size), floor(Z/tile_size), with a default size of 200 metres.
Negative coordinates use floor, not truncation toward zero.

Placements are assigned by their pivots. Mark placement entities in
`Options::split_by_tile` to partition complete triangles by their transformed
centres; this does not clip faces, so recorded bounds can cross nominal tile
edges. Each split primitive compacts vertices in first-use order and retains
all streams and resolved material variants. The caller maps any source-dialect
split flag into this option. The web writer has no game-dialect dependency.

The index records coordinates, filename, world bounds, placed triangle count,
placement count and file byte size for each tile. Degenerate placements are
counted and omitted. Malformed geometry, unloaded mesh dependencies and skipped
textures are errors. Unplaced prototypes are not exported. Tile filenames are
validated before filesystem writes; unrelated destination files are retained.

`cargo test -p primase-web` checks deterministic bytes, negative boundaries,
triangle-centre partitioning, bounds and invalid inputs. Set
`PRIMASE_GLTF_VALIDATOR` for the optional official validator gate.
