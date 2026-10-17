# Web tile format 2.0

This document specifies the output of `primase-web`. It is a consumer contract,
independent of any viewer, source game or hosting service. Numeric coordinates
are metres in a right-handed frame: +X east, +Y up, -Z north.

## Compatibility

`tiles.json` contains `format_version: "2.0"`. The two decimal integers are
major and minor. A new major changes an existing field's meaning or requires
incompatible interpretation. A minor adds optional fields or records; consumers
of the same major ignore unknown optional fields. Consumers reject unknown
majors with an explicit unsupported-format error. Required glTF extensions must
still be supported; a compatible index version does not imply codec support.

### From 1.x to 2.0

1.x tile GLBs were self-contained: every image was embedded in every tile that
used it, so an image shared by many tiles was stored, downloaded and decoded once
per tile. In 2.0 images are files of their own, shared by the tiles of a set (see
[Images](#images)); tile GLBs keep geometry and materials and reference images by
relative `uri`. The index lists the image files (`images`) and each tile the files
it uses (`images`). Geometry, materials (layered ones included), collision sidecars, scene lights and
viewer records are unchanged: decoding a 2.0 tile with its image files gives what the 1.x tile
held. A 1.x consumer must reject 2.0 (an unknown major), and a 2.0 consumer
resolves image URIs; nothing else changes for it. 2.0 also adds optional shadow extras (see
[Shadows](#shadows)), which a consumer may ignore.

Older unversioned indexes with `version: 1` are the legacy 1.0 layout. Readers
may accept that one legacy case explicitly. New writers always emit
`format_version`. The numeric `version` field is retained for old readers and
does not supersede `format_version`.

## Index schema

The root is a JSON object; all fields below are required unless marked optional.
JSON numbers must be finite; counts are nonnegative integers.

| Field | Type and meaning |
|---|---|
| `format_version` | String `major.minor`: the lowest minor holding every optional field present: `"2.2"` with a `camera`, else `"2.1"` with an `environment`, else `"2.0"` |
| `version` | Legacy integer `1` |
| `tile_size` | Positive number, nominal ground-cell side in metres |
| `empty_tile_policy` | `"omit_without_geometry_or_viewer_records; retain_metadata_without_mesh_payload"` |
| `skipped_degenerate` | Count of skipped placements with degenerate transforms |
| `placements_without_mesh` | Count of placements without loaded render geometry |
| `tiles` | Array of tile objects; deterministic order by `(col, row)`, then `cell` as `(level, x, y)` |
| `images` | Array of `{file, bytes}`: every image file the tiles reference, ordered by `file` |
| `environment` (optional, 2.1) | Object `{sun}`: the scene's environment as the source names it, scene-wide (see [Environment](#environment)); absent when the source names none |
| `camera` (optional, 2.2) | Object `{eye, target}`: a starting view (see [Camera](#camera)); absent: the consumer frames the scene |

| Tile field | Type and meaning |
|---|---|
| `col`, `row` | Signed 32-bit integers; floor(east/tile_size), floor(north/tile_size) |
| `cell` (optional) | Object `{level, x, y}` for a subcell of the cell; absent for a whole cell |
| `file` | Relative GLB filename, resolved against the index directory; the tile's identity |
| `bytes` | Exact GLB byte length |
| `content` | `"geometry"` or `"metadata_only"` |
| `min`, `max` | Three-number world-space bounds in XYZ order |
| `triangles` | Placed render triangle count, including each instance |
| `placements` | Render placements drawn by this tile, not glTF mesh or primitive count. A placement whose triangles are filed by cell counts in every tile that draws part of it, so the sum over tiles can exceed the scene's placements |
| `instanced_placements` | Placements represented by GPU instancing |
| `viewer_items` | Number of viewer records in this tile |
| `collision` (optional) | Object `{file, bytes, records}` for a collision GLB |
| `lights` (optional) | Scene lights in this tile's GLB; omitted when 0 |
| `layered_materials` (optional) | Layered materials in this tile's GLB; omitted when 0 |
| `images` (optional) | Array of the image files the tile's GLB references, in its glTF image order; absent when none |

`collision.records` counts collision meshes, not triangles. Each record's index
accessor gives its triangle count. Bounds of geometry tiles cover render
geometry, with record anchors included; metadata-only bounds cover anchors.
They do not promise to contain every collision vertex. Cells are streaming
ownership, not clipping boxes: whole meshes or triangles can extend outside.
Empty cells are omitted. Records without render geometry retain a metadata GLB.

Asset filenames contain the full lowercase SHA256 of their own bytes:
`tile_<col>_<row>.<sha256>.glb`, `collision_<col>_<row>.<sha256>.glb` and
`images/<sha256>.<ext>`; a subcell's tile and collision files insert
`_<level>-<x>-<y>` after the row. Index references and lengths must match the files. The
index is not content-hashed. Tile and collision files are GLB 2.0 with embedded
geometry and no external buffers; images are external (see [Images](#images)).

## Images

Every image a tile set uses is one file, `images/<sha256>.<ext>` beside
`tiles.json`, named by the SHA256 of its bytes: `.ktx2` (`image/ktx2`) or `.png`
(`image/png`). Tiles that use the same image reference the same file: a glTF image
in a tile GLB has `uri: "images/<sha256>.<ext>"` (relative to the GLB, which sits
beside `tiles.json`), its `mimeType` and no `bufferView`. A consumer can load each
image file once and share it among the tiles that use it, releasing it when the
last of them is unloaded; `tiles.json` lists every file with its length, so a
consumer can prefetch them or account for them before loading a tile. Image files
are immutable: the same name always holds the same bytes.

## Byte budget and subcells

The writer has a byte budget for each file a tile carries, its GLB and its
collision sidecar alike (96 MiB by default; the producer may change or disable
it). Image files are separate and not counted against a tile. A cell with a file
over the budget is split into a quadtree: halved along both ground axes into four
subcells, and each subcell still over the budget again, down to level 8 at most.
Content is divided by the same rules as cells: placements by pivot, placements
partitioned by triangle centre by their triangles' centres, and viewer records,
collision references included, by anchor, and scene lights by position. A
subcell's collision sidecar holds
the collision records anchored in it. Empty subcells are omitted, and a cell that
fits stays whole, with the bytes and name it would have without a budget.

`cell: {level, x, y}` names a subcell: `level` (1 to 8) halvings of the cell, and
`x` east and `y` north of the cell's minimum corner, each in `0..2^level`. A cell
is either one whole tile or a set of subcell tiles, never both; the subcells of a
cell may have different levels and do not overlap. `(col, row, cell)` is unique.
Bounds are each tile's own content's, as for cells. Subcells of a cell share its
image files like any other tiles.

The budget is not a guarantee. A file can stay over it when what makes it too
large is one placement or one record, which no cell boundary divides, or at the
level limit. Such tiles are written as they are; they still load.

## Render GLBs

Interpret transforms, materials, texture coordinates and accessors according to
glTF 2.0 and the declared extensions. Supported output extensions include
`EXT_mesh_gpu_instancing`, `KHR_mesh_quantization`, `EXT_meshopt_compression`,
`KHR_texture_transform`, `KHR_texture_basisu`, `KHR_materials_emissive_strength`
(emission brighter than 1: a factor within 0-1 times its strength; used, never required) and
`KHR_materials_specular` (the source's specular reflectance, written only when the source states it, also at the
extension's defaults; used, never required: a consumer that ignores it draws glTF's default specular). Read
`extensionsRequired`; never silently ignore an unsupported required extension.

GPU instancing applies only to compatible placements. Mirrored transforms stay
ordinary nodes so consumers can handle their winding correctly. Materials retain
their alpha modes, cutoffs and double-sided flags. Texture transforms belong to
the specified UV set and must be applied by the consumer.

Compressed geometry has per-mesh 16-bit position quantization and meshopt-coded
streams. `asset.extras["primase.quantization"]` records the approximation.
Quantization may move positions; bounds in the index describe pre-quantization
geometry. Consumers comparing exact vertex bounds must allow the declared step
transformed by each placement, plus floating-point transform error.

Image files are PNG or KTX2 UASTC LDR 4x4 with Zstandard supercompression. KTX2 textures
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

The writer makes records of the world's curves and markers (`core.curve` and
`core.marker`), after the caller's own: a curve is a `grind_curve` anchored at
its first point, a marker a `spawn` (marker kind `spawn`) or a `marker` (any
other kind) anchored at its position. `name` is the entity's name (else
`Entity_<n>`) and `data` the core component: `{"kind", "points", "closed"}` for
a curve, `{"kind", "ground"}` for a marker. `data.kind` is a lower-case word:
known curve kinds are `rail`, `ledge` and `coping`, known marker kinds `spawn`,
`travel`, `trainer`, `vfx` and `trigger_effect`; a source may give others, which
a consumer shows as a plain curve or marker. A marker whose source leaves its
height to be found (`data.ground` is its east/north position) is anchored at
height 0. The record node's `extras.primase` also carries the entity's other
components (a source dialect's, such as `skate.marker` or `skate.grind_curve`)
as `{"version", "value"}`, as glTF node extras do.
- `collision_reference`: `data.file` identifies the tile collision GLB and
  `data.record` indexes its collision mesh record.

Unknown record kinds can be ignored or shown as labelled markers. Other
`extras.primase` entries are namespaced, versioned source diagnostics; a generic
consumer must not require any particular source dialect. `web.instances`
preserves per-instance diagnostic identity where present. Viewer records never
change glTF geometry or material semantics.

## Provenance

Where a reader recorded it, a tile says where its content came from. The scene's
`extras.primase["web.origins"]` is `{"version":1,"value":[origin, ...]}`, one
table per tile, each origin `{"archive"?: string, "file": string, "item"?: string}`
in the source game's own naming (`file` inside `archive`; `item` the part of the
file, such as a mesh or a model and geometry). Meshes, nodes, instances in
`web.instances`, materials and textures may carry
`extras.primase["web.provenance"]` with
`{"version":1,"value":{"origins":[i, ...],"parts"?:[i, ...],"passes"?:[text, ...]}}`:
`origins` are indices into the tile's table, `parts[k]` (meshes only) is the
origin of the mesh's primitive `k`, and `passes` names, in order, the conversion
passes that split, merged or baked the object. A picked primitive `k` of mesh `m`
came from `web.origins.value[m.web.provenance.value.parts[k]]`. Both keys are
diagnostics: consumers may ignore them. Tiles without provenance have neither
key and are byte for byte what they were before provenance existed.

## Scene lights

Scene lights (introduced in 1.1) are carried as before. Each light is a glTF node with `KHR_lights_punctual`
(listed in `extensionsUsed`, never `extensionsRequired`) in the tile whose cell
contains the light's position, like a viewer record's anchor; a cell with lights
but no render geometry gets a metadata-only GLB. The node's `matrix` is the
light's world transform: position in the last column, a spot shining along its
local -Z. Light values follow `KHR_lights_punctual`: linear RGB `color`,
`intensity` in candela for point and spot lights, optional `range` (absent:
unlimited), spot cone angles in radians from the axis. Bounds include light
positions. Lights are illumination sources only; consumers may render a subset
(for example the nearest few) and must ignore them without breaking geometry.

## Environment

2.1 adds the optional index field `environment`. Its `sun` is the directional light the source names as its sun,
carried once for the whole scene in the index, not as a light node of a tile, so a consumer has it whichever tiles are
loaded:

| Field | Type and meaning |
|---|---|
| `name` | String, the light's name |
| `color` | Three numbers, linear RGB (as `KHR_lights_punctual`) |
| `intensity` | Number, illuminance in lux (as a `KHR_lights_punctual` directional light) |
| `direction` | Three numbers, a unit vector in the index's world frame (metres, +X east, +Y up, -Z north): the way the light travels, its transform's local -Z |
| `cast` (optional) | `"on"` or `"off"`: whether it casts shadows, as the shadow extras; absent: unknown |

Other directional lights stay scene lights of the tile whose cell holds their position. Without an environment (and without a camera) the index is 2.0, unchanged. Ambient light, sky and fog are not carried.

## Camera

2.2 adds the optional index field `camera`, a starting view the writer was given:

| Field | Type and meaning |
|---|---|
| `eye` | Three numbers: where the view starts, in the index's world frame (metres, +X east, +Y up, -Z north) |
| `target` | Three numbers: the point it looks at, same frame, apart from `eye` |

Up is +Y. A consumer may start elsewhere (a view the user asked for); without a camera it frames the scene itself.

## Shadows

What the source states about shadows is carried as glTF extras, `extras["primase.shadows"]`, an object with
`"version": 1` and these keys, each present only when known (absent: unknown, use the renderer's default; a stated
`"off"` or `false` is a fact):

- on a placement's node: `cast`, `"off"`, `"on"` or `"only"` (casts a shadow but is not drawn: a shadow proxy), and
  `receive`, a boolean;
- on a material: `cast`, `"off"` or `"on"`, for the surfaces drawn with it;
- on a scene light's node: `cast`, `"off"` or `"on"`.

A node's value overrides its materials'. An instanced node (`EXT_mesh_gpu_instancing`) only groups placements with the
same shadow extras, and carries them itself. Shadow map sizes, bias and cascades are renderer settings, not scene
data, and are not carried.

## Layered materials

Layered materials (introduced in 1.2) are carried as before. A layered material blends up to four ordinary materials per pixel from an
interpolated vertex channel. It is a glTF material carrying the vendor extension `PRIMASE_layered_material`, listed in
`extensionsUsed` and never in `extensionsRequired`:

```json
"extensions": {"PRIMASE_layered_material": {
  "weights": {"attribute": "COLOR_1", "mapping": "rgb_product_4"},
  "layers": [null, {"material": 12, "texCoord": 1, "transform": {"offset": [0, 0], "rotation": 0.5, "scale": [4, 4]}},
             {"material": 14}, {"material": 15}]}}
```

- `weights.attribute` names a primitive attribute, `COLOR_n`, VEC3 or VEC4 (float or normalised unsigned; VEC3 has an
  implied fourth channel of one). `mapping` is `identity` (the channels weight the slots in order) or `rgb_product_4`
  (four slots: (1-r)(1-g)(1-b), (1-r)(1-g)b, (1-r)g(1-b), (1-r)gb: the weights sum to 1-r, red darkens, r = 1 is black).
- `layers` has one to four entries, in slot order; `null` is a black slot (its weight is not redistributed). A layer's
  `material` indexes this file's `materials`: an ordinary material (never itself layered) whose base colour and normal
  textures and factors are the layer's (its metallic-roughness and occlusion data are only used when it is the host).
  `texCoord` selects the UV set for all of the
  layer's textures (absent: TEXCOORD_0), and `transform` is `KHR_texture_transform` semantics (offset, rotation in
  radians, scale) applied to those UVs before sampling; layers may differ in both, so their tangent frames differ.
- Per pixel: interpolate the channels, evaluate the mapping, colour = sum(weight[i] * layer[i]) with the weights not
  clamped or renormalised, normals blended with the same weights after each is taken into the common frame, then
  normalised (an all-zero sum has no direction; the consumer states its fallback).
- **Scalar channels are not layered.** The equation above is exact for colour and (once each is in the common frame)
  for normals. Roughness, metallic and occlusion are the **host material's own** values: its factors times its
  metallic-roughness and occlusion textures, sampled with the host's own UV sets, constant across the layers whatever
  the weights. This follows GTA's terrain shaders (specular from per-material constants, constant metalness, per-vertex
  occlusion). A layer's own metallic-roughness and occlusion data are not used by a layered consumer (they are what the
  host copies from the first layer, so the first layer's values apply everywhere). A source whose shader varies a
  scalar per layer is not reproduced: such a channel would need an explicit, versioned extension field (none exists
  yet), not a guess by the consumer.
- The host material (the one primitives reference) is a verbatim copy of the first non-null layer's material, so a
  consumer that ignores the extension renders that layer. A primitive drawn with a layered material but lacking the
  weight attribute renders as the host.
- Layer materials are written once, like any material; their images are shared with every other user.

## Synthetic conformance fixture

`cargo run -p primase-web --features ktx2 --example synthetic_tiles -- <output>`
generates a small scene entirely from literal geometry and pixels. It includes
instancing, a mirrored placement, transformed texture coordinates, compression,
KTX2, collision and a closed curve. `plain/` is its uncompressed comparison;
`proof.json` contains independently decoded meshopt hashes and
`ktx2-reference.json` contains Rust transcoder mip hashes. These proof files are
test artifacts, not required production tile-set fields.
