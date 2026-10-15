# Web tile format 1.0

This document specifies the output of `primase-web`. It is a consumer contract,
independent of any viewer, source game or hosting service. Numeric coordinates
are metres in a right-handed frame: +X east, +Y up, -Z north.

## Compatibility

`tiles.json` contains `format_version: "1.0"`. The two decimal integers are
major and minor. A new major changes an existing field's meaning or requires
incompatible interpretation. A minor adds optional fields or records; consumers
of the same major ignore unknown optional fields. Consumers reject unknown
majors with an explicit unsupported-format error. Required glTF extensions must
still be supported; a compatible index version does not imply codec support.

Older unversioned indexes with `version: 1` are the legacy 1.0 layout. Readers
may accept that one legacy case explicitly. New writers always emit
`format_version`. The numeric `version` field is retained for old readers and
does not supersede `format_version`.

## Index schema

The root is a JSON object; all fields below are required unless marked optional.
JSON numbers must be finite; counts are nonnegative integers.

| Field | Type and meaning |
|---|---|
| `format_version` | String `major.minor`, currently `"1.0"` |
| `version` | Legacy integer `1` |
| `tile_size` | Positive number, nominal ground-cell side in metres |
| `empty_tile_policy` | `"omit_without_geometry_or_viewer_records; retain_metadata_without_mesh_payload"` |
| `skipped_degenerate` | Count of skipped placements with degenerate transforms |
| `placements_without_mesh` | Count of placements without loaded render geometry |
| `tiles` | Array of tile objects; deterministic order by `(col, row)` |

| Tile field | Type and meaning |
|---|---|
| `col`, `row` | Signed 32-bit integers; floor(east/tile_size), floor(north/tile_size) |
| `file` | Relative GLB filename, resolved against the index directory |
| `bytes` | Exact GLB byte length |
| `content` | `"geometry"` or `"metadata_only"` |
| `min`, `max` | Three-number world-space bounds in XYZ order |
| `triangles` | Placed render triangle count, including each instance |
| `placements` | Render placement count, not glTF mesh or primitive count |
| `instanced_placements` | Placements represented by GPU instancing |
| `viewer_items` | Number of viewer records in this tile |
| `collision` (optional) | Object `{file, bytes, records}` for a collision GLB |

`collision.records` counts collision meshes, not triangles. Each record's index
accessor gives its triangle count. Bounds of geometry tiles cover render
geometry, with record anchors included; metadata-only bounds cover anchors.
They do not promise to contain every collision vertex. Cells are streaming
ownership, not clipping boxes: whole meshes or triangles can extend outside.
Empty cells are omitted. Records without render geometry retain a metadata GLB.

Asset filenames contain the full lowercase SHA256 of their own bytes:
`tile_<col>_<row>.<sha256>.glb` and
`collision_<col>_<row>.<sha256>.glb`. Index references and lengths must match the
files. The index is not content-hashed. Files are self-contained GLB 2.0; image
and geometry payloads are embedded, with no external glTF buffer/image URIs.

## Render GLBs

Interpret transforms, materials, texture coordinates and accessors according to
glTF 2.0 and the declared extensions. Supported output extensions include
`EXT_mesh_gpu_instancing`, `KHR_mesh_quantization`, `EXT_meshopt_compression`,
`KHR_texture_transform` and `KHR_texture_basisu`. Read `extensionsRequired`;
never silently ignore an unsupported required extension.

GPU instancing applies only to compatible placements. Mirrored transforms stay
ordinary nodes so consumers can handle their winding correctly. Materials retain
their alpha modes, cutoffs and double-sided flags. Texture transforms belong to
the specified UV set and must be applied by the consumer.

Compressed geometry has per-mesh 16-bit position quantization and meshopt-coded
streams. `asset.extras["primase.quantization"]` records the approximation.
Quantization may move positions; bounds in the index describe pre-quantization
geometry. Consumers comparing exact vertex bounds must allow the declared step
transformed by each placement, plus floating-point transform error.

Images are PNG or KTX2 UASTC LDR 4x4 with Zstandard supercompression. KTX2 textures
use `KHR_texture_basisu` and are transcoded to a supported GPU format. Colour
textures use sRGB; normal and data textures are linear. The writer may retain
PNG when normal or alpha semantics would exceed its lowering error bounds.
The file's dimensions and mip levels are authoritative; source resolution is
not an implicit promise. Texture lowering metadata is advisory, not required
for rendering.

## Collision sidecars

Collision GLBs use the same world coordinate frame. Each mesh contains indexed
triangle geometry (`POSITION`, mode 4); triangle order is preserved. A surface
is a **per-triangle** value, not a vertex attribute. Primitive extras contain:

```json
{"primase.collision_surfaces":{"version":2,"domain":"triangle","accessor":2,"names":["default","Snow"]}}
```

The writer emits version 2: the accessor is SCALAR, unsigned short
(componentType 5123), one surface ID per triangle, and `names` maps surface IDs
to names (the world's `core.surfaces`; empty when the records given to the
writer name none, in which case IDs are only IDs). Every ID is below the length
of a non-empty `names`. Version 1, written by earlier tile sets, has an unsigned
byte accessor (5121) and no `names`; it is no longer written. These extras
versions are independent of the tile format version. Consumers must read the
accessor type, not assume a byte per surface; unknown extras versions must not
be interpreted as known ones. Names are labels, not universal physical
properties. Triangles keep counter-clockwise fronts in world space: a soup placed
through a mirroring transform has its triangles' index order flipped. Rendering collision is
optional and does not contribute to `triangles` in the index.

## Optional viewer records

Identity glTF nodes carry `extras.primase["web.viewer"]` with
`{"version":1,"value":{"name":...,"kind":...,"anchor":[x,y,z],"data":...}}`.
All payload positions are already world-space. `anchor` selects the cell and
provides a marker position. Known kinds:

- `grind_curve`: `data.points` is an array of XYZ positions and `data.closed`
  closes the polyline. Allocate the closing point when creating its geometry.
- `spawn` and `marker`: draw a gizmo at the anchor; other payload fields are
  optional source information.
- `collision_reference`: `data.file` identifies the tile collision GLB and
  `data.record` indexes its collision mesh record.

Unknown record kinds can be ignored or shown as labelled markers. Other
`extras.primase` entries are namespaced, versioned source diagnostics; a generic
consumer must not require any particular source dialect. `web.instances`
preserves per-instance diagnostic identity where present. Viewer records never
change glTF geometry or material semantics.

## Synthetic conformance fixture

`cargo run -p primase-web --features ktx2 --example synthetic_tiles -- <output>`
generates a small scene entirely from literal geometry and pixels. It includes
instancing, a mirrored placement, transformed texture coordinates, compression,
KTX2, collision and a closed curve. `plain/` is its uncompressed comparison;
`proof.json` contains independently decoded meshopt hashes and
`ktx2-reference.json` contains Rust transcoder mip hashes. These proof files are
test artifacts, not required production tile-set fields.
