# Web tile sets

`write(&World, &Options)` produces deterministic GLBs and a `tiles.json` index;
`TileSet::write_to` writes them to a directory. Load mesh/texture files and lower
materials to PBR before export.

A scene too large to hold loaded at once goes through the same writer one cell at
a time. `plan(&World, &Options)` assigns placements, viewer records and the
world's collision to cells without loading anything; collision is filed by its
anchor and held only by reference (the holder is one of the cell's roots, and its
soup is made from the cell's world when the cell is written), so the plan holds no
geometry. With KTX2, encodings are held in memory between tiles up to
`Options::encoding_memory` (least recently used dropped, then read from the disk
cache or encoded again); the bytes are the same at any bound. `TilePlan::split` files the
triangles of each `split_by_tile` placement from a world holding its prototype
loaded. Then, per cell, the driver takes the world's `World::closure` of
`TilePlan::roots(cell)` as a `World::subset`, loads its meshes and textures,
lowers its materials, calls `TilePlan::write_cell` and stores each `WrittenTile`
(`WrittenTile::write_to`) before dropping that world. `TilePlan::finish` returns
the index, and `Index::write_to` publishes `tiles.json` once every file it names
is in place. `write` is that pipeline over an already prepared world. Each tile is
written as a `primase_gltf::Selection` of the cell's world, which stays unchanged,
so one loaded cell can be written by several plans (the example's plain set).
`target()` declares what a cell's world may hold when it is written (a converter
checks it after the cell's passes): its files loaded, levels of detail chosen,
transforms rigid. `plan` refuses a mirrored or sheared placement: `bake-nonrigid`
(primase-meshops) moves an authored mirror or shear into geometry, which needs the
prototype's geometry loaded, so a streamed scene with one loads that prototype and
runs the pass before planning (no real scene converted so far has one).

A reader that loads meshes only per cell plans with `plan_with_geometry`, saying
which prototypes have geometry still to load; `spanning` finds, from prototype
boxes (`primase_gltf::mesh_file_box` reads one from a GLB's JSON alone), the
placements whose pivot does not locate their geometry, to be filed by triangle
centre.

`Options::tile_byte_budget` (96 MiB by default, `None` to disable) caps each file a
tile carries, its GLB and its collision sidecar: a cell with a larger one is split
into quadtree subcells, by the cell rules, until each fits (FORMAT.md, "Byte
budget and subcells"). Cells that fit keep the bytes and names they would have
without a budget. A tile still over it, because one placement or viewer record
makes it so or at the level limit, is written and listed in
`TilePlan::over_budget` (`TileSet::over_budget`). `scene_to_tiles` takes
`--budget-mib N` or `--no-budget`. The writer
never loads or lowers: what it needs missing is an error. A tile holds its
placements and what they refer to, and the world's lights whose position it contains. The world uses metres and +Y up. Ground tiles
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
16-bit integers in a frame per mesh: centred on the mesh's bounding box, with one
positive uniform step that reaches its farthest coordinate (uniform decoding keeps
normal directions). The frame is composed into every placement of the mesh (node
matrix, node translation and scale, or the instance TRANSLATION and SCALE
streams), so the decoded mesh lands where it was. The change in world position is
measured at every vertex through every placement, from the numbers as stored (f32
instance streams, node JSON as written); a mesh whose error would exceed
`POSITION_BUDGET` (0.25 mm) keeps f32 positions and its placements unchanged.
Meshes sharing a position accessor share a frame. Normals and tangents use signed
normalized 16-bit components. UV streams wholly in `[0,1]` use unsigned normalized
16-bit components; repeating UVs remain f32. This is an explicit lossy
approximation made by the web lowering; `asset.extras["primase.quantization"]`
records `position_max_world_error`, `position_max_local_error`,
`position_max_local_step`, `meshes_float_positions` and the budget. Indices retain
their exact original bytes/order.
Measured on two GTA V regions converted with and without quantization (compressed
set against its plain twin, every vertex through every placement, node numbers as
the f32 values they are): Paleto Bay (6.4 km from the origin) went from a largest
vertex error of 93.7 mm (water meshes 6.5 km across, origin-centred) to 0.249 mm,
seams from 103.6 mm to 0.29 mm, with 243 of 566 meshes keeping f32 positions and
3.5% more tile bytes; downtown's north-east edge went from 3.09 mm to 0.24 mm,
seams from 3.92 mm to 0.15 mm, 93 of 239 meshes f32, 4.2% more bytes. These are
measured corpus results; the budget is what the writer guarantees.
Vertex and instance streams use `EXT_meshopt_compression` v0 with no filter;
every stream is decoded and checked byte-for-byte before output. Required
extensions include `KHR_mesh_quantization` and meshopt. PNG bytes stay unchanged.

`Options::viewer_items` accepts named, typed records with an anchor in metres and
a JSON payload. They become identity nodes with JavaScript-safe unique names and
versioned `web.viewer` extras. Payload positions remain in world space. Anchors
choose streaming tiles, including metadata-only tiles; records are not clipped
or duplicated across tile boundaries. The writer adds records of the world's
curves and markers (`core.curve`, `core.marker`) and of its world collision
soups (FORMAT.md, "Optional viewer records"). Prototype
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
Images are files of their own (format 2.0): each encoded image is written once,
as `images/<sha256>.<ext>`, and every tile that uses it references that file by
`uri`, so tile GLBs carry geometry and materials only. The index lists the image
files with their lengths, each tile the ones it uses. `WrittenTile::write_to`
writes a tile's images (those not there yet) before the tile.

Render and collision filenames include the full SHA256 of their own bytes.
Collision names are finalized before their references are embedded in render
tiles. `write_to` rejects missing files, mismatched lengths, and stale hashes
before publishing anything; it writes images, then tiles and collision, and
`tiles.json` last, which stays unhashed.
When publishing a new export over an old one, copy the immutable assets first
and replace `tiles.json` last, and keep older assets while clients may still
reference them. Indexes need short cache lifetimes; the content-addressed assets
may be cached immutably.
The example omits collision soups with no triangles; they have nothing to draw
and do not create an origin tile merely because their empty bounds default to zero.

`cargo run --release -p primase-web --example scene_to_tiles -- <scene> <output>`
loads a skate-scene folder and writes compressed tiles, a `plain/` comparison
set, and `proof.json` with artifact and decoded-stream SHA256 hashes, preparing
one cell at a time for both sets, so memory holds one cell's closure. It files a
placement by triangle centre (`split_by_tile`) when its pivot does not locate its
geometry: its world-space box, from its prototype GLB's POSITION bounds, is wider
than a cell on a ground axis, or does not contain its pivot (beyond 1 m), as with
a model placed at the origin with world-space vertices. A small prop across a cell
edge keeps pivot filing and instancing. It prints the counts. This is an
example composition; the library does not depend on a source dialect.

To look at the output, use [primase-viewer](https://github.com/kalmarv/primase-viewer), an example
consumer of this format (a three.js viewer, not part of the library). Its `audit.mjs` loads every tile with
three.js' GLTFLoader and MeshoptDecoder, compares each compressed stream with the Rust decoder's hashes and the
placed triangle counts with the index. The official glTF validator does not implement the instancing or meshopt
extensions, so its zero-error result supplements rather than replaces that audit.

## Optional KTX2 evaluation

Build with `--features ktx2` and set `Options.ktx2 = true` (or pass `--ktx2`
to the example). The default remains PNG. The optional encoder is pinned to
`basisu_c_sys = 0.9.1`; see [source identity and licences](third-party/README.md).
Only this feature adds the native Basis encoder and bindgen/libclang requirement.

The web lowering uses UASTC LDR 4x4 with Zstandard, quality 75, effort 2,
threaded encoding disabled, and a complete wrapping mip chain. Colour maps are
sRGB; normal and mask maps are linear. A shared source used for different roles
gets separate texture bindings. The DFD primaries are BT709 for colour and
unspecified for data, as required by
[KHR_texture_basisu](https://github.com/KhronosGroup/glTF/tree/main/extensions/2.0/Khronos/KHR_texture_basisu).

Colour uses UASTC quality 100 (RDO disabled), effort 2, with 512 pixels per
axis and 360,448 encoded bytes; masks use 256 pixels and 98,304 bytes. Normals
use lossless PNG at up to 512 pixels per axis, with no encoded-byte cap. Each
axis is lowered to a power of two, at least four pixels. Small axes are
replicated. Source/output dimensions, role, caps and actual bytes are recorded
in image extras. These limits describe the web target, not the source format.

The per-axis caps are `Options::texture_caps` (defaults above: colour 512,
normal 512, mask 256; powers of two, at least 4). `Options::texture_caps_by_name`
sets tighter caps for images whose glTF name starts with a given prefix (the
first matching rule wins), for example to bound generated texture variants. The
`scene_to_tiles` example exposes this as `--texture-cap <prefix>=<colour>,<normal>,<mask>`.

Colour filtering averages in linear light. Normal filtering uses
`primase_image::resize_normals`: normalize source vectors, average them, then
normalize the mean. Means shorter than sqrt(3)/255 become flat (0,0,1); image
extras count these replacements. `normal_mip_chain` exposes the same policy to
other writers. PNG carries one level; browser-generated mip filtering is not
claimed to implement this vector filter. Resolution loss is measured separately
from codec error, including its angular distribution.

The encoder checks alpha against the capped source before accepting a colour
image. Every cutout binding must change visibility for fewer than 0.1% of pixels
at its material cutoff (including base-colour alpha factor). Blended alpha must
have p99 absolute error at most 8 levels. Otherwise the capped image stays PNG.
The decision and per-binding counts are stored in image extras; opaque bindings
ignore alpha error. Shared images use all applicable binding requirements.

`cargo test -p primase-web --features ktx2` checks a pure-Rust independent
decoder, all mip dimensions, role metadata and separate-process SHA256 equality.
The synthetic gradient bound is 8 channel values (measured maximum 1); this is
not a bound for arbitrary images. Real-corpus colour and normal-angular error,
an independent WASM transcoder and peak browser memory are measured separately
for each export.

The `ktx2_audit` example hashes every decoded mip with the pure-Rust oracle;
primase-viewer's `audit-ktx2.mjs` produces corresponding hashes with Three.js' pinned WASM
transcoder. When `plain/tiles.json` is available, the Rust audit additionally
reports per-role channel maxima/means and normal-angle maxima/means/p99.
Quality samples are deduplicated by source content, role, output dimensions and encoded content.
Two comparisons distinguish codec error against capped reference pixels from
end-to-end error against original pixels with nearest-centre reconstruction.
The latter includes resolution loss and does not model GPU filtering.
The p99 angle is an upper bound using 0.001-degree bins; max and mean use the
unbinned angles. The geometry audit treats both PNG and KTX2 as dimension-only
textures; these independent pixel audits supply the separate texture gate.
