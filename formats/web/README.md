# Web tile sets

`write(&World, &Options)` produces deterministic GLBs and a `tiles.json` index;
`TileSet::write_to` writes them to a directory. Load mesh/texture files and lower
materials to PBR before export. The world uses metres and +Y up. Ground tiles
use floor(east/tile_size), floor(north/tile_size), with a default size of 200 metres.
The shared ground frame maps east to X and north to -Z.
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

GPU instancing is enabled by default (`Options::gpu_instancing`). Compatible
positive TRS placements sharing a resolved mesh become `EXT_mesh_gpu_instancing`
nodes. Mirrored/sheared placements stay plain nodes, as do matrices whose
normalized basis cannot be reconstructed within 1e-6 per element. Instance TRS
uses f32 accessors; source names/extras survive in `web.instances` metadata.
`instanced_placements` reports how many placements were packed in each tile.

Compression is enabled by default (`Options::compress`). Positions use signed
16-bit integers with one positive, origin-centred uniform decode scale per mesh;
node/instance scales absorb that scale, preserving normal directions. Normals
and tangents use signed normalized 16-bit components. UV streams wholly in
`[0,1]` use unsigned normalized 16-bit components; repeating UVs remain f32.
This is an explicit lossy approximation made by the web lowering. Each position
axis uses 16 bits (signed integer range -32767 to 32767). A mesh's largest absolute
local coordinate M gives a uniform step M/32767; the nominal 200-m tile grid does
not bound M, because complete triangles and pivot-assigned meshes can cross tiles.
The index records each tile's actual extent as `max - min`.
Positions may move by up to half a quantization step per local coordinate
(plus floating-point rounding); the measured maximum local coordinate error is
stored in `asset.extras["primase.quantization"]`. Scale by the placement basis
to assess world-space error. The metadata also records `position_max_local_step`.
For each world axis, multiply half the local step by the sum of absolute values
in that placement basis row, allowing for f32 transform rounding. Report local
quantization error separately from the measured world-bounds difference against
the unquantized export. Indices retain their exact original bytes/order.
Two real-scene gates measured maximum local steps of 9.044 mm and 11.323 mm,
maximum local coordinate errors of 4.521 mm and 5.662 mm, and maximum world-bounds
differences of 1.636 mm and 5.326 mm respectively. The latter scene's worst-step
tile spans 396.052 by 235.011 by 320.626 metres, despite the 200-m grid.
These are measured corpus results, not a universal maximum-error guarantee.
Vertex and instance streams use `EXT_meshopt_compression` v0 with no filter;
every stream is decoded and checked byte-for-byte before output. Required
extensions include `KHR_mesh_quantization` and meshopt. PNG bytes stay unchanged.

`Options::viewer_items` accepts named, typed records with an anchor in metres and
a JSON payload. They become identity nodes with JavaScript-safe unique names and
versioned `web.viewer` extras. Payload positions remain in world space. Anchors
choose streaming tiles, including metadata-only tiles; records are not clipped
or duplicated across tile boundaries. The example lowers grind curves, markers
(including spawns), and world collision soups into these records. Prototype
collision remains prototype metadata and is not expanded into world soups.
Collision records are stored separately in `collision_<col>_<row>.glb`; each
tile's optional `collision` index entry records its filename, byte size and record
count. The GLB carries only small references. The viewer fetches these payloads
only when **Show collision** is enabled, and disposes collision geometry when it
is disabled or the tile is unloaded. Render byte counts exclude these sidecars.
Collision GLBs store exact f32 positions, u32 indices and one u8 surface per
triangle. Primitive extras `primase.collision_surfaces` reference the surface
SCALAR accessor by index, with `domain: "triangle"` and `version: 1`. This is a
face attribute, not a glTF vertex attribute; consumers can obtain it through
Three.js `parser.getDependency('accessor', index)` while loading geometry normally.
GLTFLoader exposes these primitive extras on `mesh.geometry.userData`, rather
than `mesh.userData` (which receives mesh-level extras).
The index's `empty_tile_policy` omits tiles with neither geometry nor viewer
records. A tile with only records has `content: "metadata_only"`: its GLB contains
identity nodes and extras, with no meshes, buffers or binary mesh payload.
Geometry tiles have `content: "geometry"`.
The example omits collision soups with no triangles; they have nothing to draw
and do not create an origin tile merely because their empty bounds default to zero.

`cargo run --release -p primase-web --example scene_to_tiles -- <scene> <output>`
loads a skate-scene folder and writes compressed tiles, a `plain/` comparison
set, and `proof.json` with artifact and decoded-stream SHA256 hashes. This is an
example composition; the library does not depend on a source dialect.

The static `viewer/` page uses pinned Three.js ES modules from a CDN, without a
build step. Serve it alongside the output and select a relative tile folder with
`?scene=folder/`. Orbit/pan controls stream tiles by distance from the view target;
the checkbox shows curves, markers and wireframe collision. `window.primaseProof`
reports loaded tiles, triangles, pending requests, failures and rendered frames.
Initial framing uses tiles with render triangles. `cam=x,y,z,tx,ty,tz` and optional
`fov` URL parameters restore a camera; `window.primaseCamera()` returns its current
coordinates. **Copy view** (C) copies a JSON report containing that URL, viewer
build, camera, optional note and the object/material/source metadata beneath the
last canvas click (or screen centre). **Screenshot** (S) downloads a PNG named
with scene, camera and time. Shortcuts are disabled while entering text. If
clipboard access is unavailable, the report appears as selected text for manual
copying. `npm test` checks the JSON/URL camera round trip and filename handling.

For the local geometry proof, run `npm ci --ignore-scripts` in `viewer/`, then
`npm run audit -- <output>`. This uses Three.js GLTFLoader and MeshoptDecoder to
load every tile, compare each compressed stream against the Rust decoder hash,
and compare placed triangle counts against the index. It reads PNG dimensions
for loader setup but does not render textures; browser inspection is a separate
gate. The official validator does not implement the instancing or meshopt
extensions, so its zero-error result supplements rather than replaces this gate.
