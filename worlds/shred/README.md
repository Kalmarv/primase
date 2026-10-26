# primase-shred

Tony Hawk: SHRED (PS3, 2010), RME archives and scene geometry, including processed crowd/actor source meshes.

`read::pak::Pak::open` reads one PAK v4; `from_bytes` accepts caller-owned bytes. Metadata validates the header,
name hashes, supported attributes/alignment, zero on-disc runtime offsets, bounded NUL-terminated names and exact
payload tiling. Names remain byte strings, including original case and separators. Sections include their marker
entries; entries before a marker have no section. Allocation alignment is a runtime property, not a file constraint.
Stored entries have `compressed_size == 0` and occupy `size` bytes; compressed entries occupy `compressed_size` bytes.

`payload(index)` borrows stored bytes or lazily decodes one CSTR v1. `encoded(index)` exposes its encoded bytes.
`read::cstr::decode` checks the endian tag, version, threshold, block count, block bounds, exact DEFLATE consumption,
each output length, total output length and exact stream end. Errors retain byte offsets. Neither API extracts paths
to disk. The archive bytes remain in memory; decoded payloads are not cached.

The format contracts come from the disc's PAK/CSTR headers and its native RME loader. Raw DEFLATE decoding uses
the toolkit's flate2 adapter, independently of that loader. Tests include hand-built framing and RFC 1951 blocks,
malformed/truncated inputs, and a real-disc census. The latter is a structural check of the original data, not an
independent semantic decode of the enclosed resources.

Configure `installs.shred` in the local test-data file as the extracted disc root containing `PS3_GAME/USRDIR`.
Run `cargo test --release -p primase-shred -- --nocapture` with that configuration. Smoke depth selects eight archives
by a seeded rotation of sorted paths; full depth opens every archive and decodes every stream, checking the known
disc census: 336 archives, 67,320 entry occurrences, 31,227 streams, 111,391 blocks, 4,221 stored blocks.

`read::scene::decode(scp, mlp, mmp, mgp)` reads one decoded quartet in source coordinates. It checks resource IDs,
slice coverage through EOF (only zero alignment padding), finite affine matrices, identity model-local matrices,
stride-16 positions, stored AABB containment (0.05 source units), BE u16 triangle lists, index bounds and exact
submesh partitions. Unsupported mesh variants are individually refused with their identity, submesh material
references and placements retained; supported siblings still decode. Malformed supported records fail with the file
and byte offset. Processed stride-36 declarations use the embedded source format described below.

`read::level::read_level(&[(label, &pak), ...], frame)` assembles all quartets of precisely the supplied archives.
The program chooses the base/layers. There is one prototype per resource occurrence, one primitive per rigid
submesh, one material entity per nonzero global ID and one instance per placement. ID zero is keyed by the
quartet source namespace, so actor and environment defaults remain separate. Submesh vertex ranges are narrowed and
indices rebased. Source render triangles are clockwise relative to their stored normals. The reader normalizes
them to core's counterclockwise fronts, swapping indices when `source_cw XOR frame.flips_winding()` is true.
This format convention is independent of the caller's frame/handedness choice. Mirrored placements retain their
affine transforms. Refused prototypes have `shred.refused_mesh` instead of fabricated geometry. IDs live in `shred.mesh_id`
and `shred.material_id`; zero's source namespace is retained as `shred.material_namespace`.

The caller **must state a `primase_core::frame::SourceFrame`**; there is no default and the reader chooses nothing.
Y-up is evidenced by the geometry recon. Handedness is established from the engine's own matrices (observed in
the running game): the view matrix has determinant +1, the player stands at view-space z +2.456, and world +X maps to
screen right. Native SHRED is therefore left-handed (+X right, +Y up, +Z forward), and a right-handed caller reads it
with the Z mirror, `SourceFrame::new([PosX, PosY, NegZ], scale)`. North and physical metres are not established, and
the census's identity frame makes no claim about either. Conversion of positions, transforms and winding uses core's
frame operations.

`read::texture::TextureCatalog::new(&[(label, &pak), ...])` indexes all texture headers in precisely the supplied
archives. `resolve(extensionless_path)` returns every matching record, including repeated packages, streamed and
prefetch copies; it compares slash and backslash equally and preserves case. `records()` exposes each copy's source
archive/section, exact 84-byte header, unresolved words, GCM description and available pixel entries. Missing pixels
remain explicit. MAIN/LOCAL and CACHE_MAIN/CACHE_LOCAL are paired by their source section convention. Duplicate
pixel entries in the matching sections are refused rather than selected.

`decode(record_index, Payload::Primary | Payload::CubeTail, color_space, role)` returns core textures, one per cube
face, through the toolkit GCM codec. The caller selects the copy and its colour meaning. Prefetch and `_m1.txp`
use the source's mip tail (smaller axis at most 128, or the single mip), retaining base pitch for linear textures.
Identity-remapped BC1/BC3 remain compressed; ARGB/B8 and remapped BCn become RGBA8. The 2D API refuses volumes;
`decode_volume` returns an explicit `gcm::Volume` with depth retained at each mip, Z-major slices and no automatic
slice selection. It supports uncompressed XYZ Morton storage and BC1/BC3 VTC storage through the toolkit codec.
This API does not assign textures to materials or choose LOD.

`texture_disc` uses smoke/full depth: eight seeded archives or the whole disc. It decodes every available header/pixel
pair, expands every supported mip through the pixel codec, counts volume/header-only entries, and compares each
prefetch copy with its streamed mip tail and each cube `_m1` with prefetch. This is an independent copy stored by
the game; hand-worked fixtures separately check header parsing, pairing and pixel meaning.

`volume_disc` separately decodes all volume copies and compares their stored prefetch versions. It measures RGB
differences across neighbouring Z slices against a deterministic control that shuffles pixel correspondences within
the next slice (preserving its histogram), with a synthetic positive control. For the source's 32-cube grading LUTs,
it compares all six RGB axis assignments against the canonical RGB ramp. These are content checks of this disc,
not universal promises that arbitrary 3D textures must be smooth or close to an identity colour transform. Smoke
selects eight volume-bearing PAKs from a seeded rotation; full covers all volume headers. The layout descriptions
come from NVIDIA's published VTC extension and RPCS3's RSX consumer at commit
`979a52415003fd7a74ddd216e1a5782b3731281f`; no RPCS3 implementation code was copied. The codec documents its
unproven layout combinations as explicit refusals rather than inferring them from these content metrics.

The returned `LevelReport` counts all occurrences (including packaged copies and refused models) and lists every
refused model and unread table region with its source and decoded-file offset. MMP +12 normals use the native signed
11:11:10 CMP conversion and the source frame's direction conversion, without normalization or position scaling.
Nonzero normals must be unit length within 0.005; authored zero normals remain zero and are counted and listed by
resource in the report. All 16 observed packed MGP layouts decode finite colours, half-float UVs and native vertex
inputs. Unknown format bits refuse that mesh while retaining rigid siblings.

Submesh +0x38 maps logical UV slots 0..3 to stored MGP UV sets; byte 0xff selects the same-index stored set.
Missing stored sets produce no stream. UVs retain the source's top-left convention, matching core and decoded
textures. Colours become normalized RGBA streams. Unproved tangent roles and bitangent sign, and tree attributes remain in
`shred.vertex_inputs` as `attr15`, `attr14`, `attr12` and `attr13`; no core tangent is invented. This component also
retains every stored UV set as `stored_uv0` through `stored_uv3`, including sets unused by the remap. Its values
retain the recorded source frame and remain source provenance under geometry transforms. Primitive/vertex remaps
keep these arrays aligned with the mesh. The read world includes them alongside geometry; loading retains the selected arrays.

SCP table strides are checked
against the native loader, but their meanings and nested objects remain unread. SCP's opaque coverage notice
intentionally overlaps interpreted fields and is **not** a count of distinct unread bytes.

`cgbinary_disc` feeds the `.vsp` and `.psp` entries to `primase-rsx`'s generic
CgBinaryProgram reader. SHRED owns the stream's 64-byte zero padding between
containers. Full depth checks 743 VSP and 765 PSP occurrences, containing 199,017
vertex and 181,851 fragment programs and 8,041,219 parameter records. Smoke
selects eight shader-bearing archives by seeded rotation. The corpus checks
container structure and preserved metadata; it does not establish microcode
meaning or identify sampler roles. Those require disassembly and an independent
instruction oracle. No shader-to-material binding is added by this test.

`fragment_disc` compares every distinct fragment microcode with the independently
run RPCS3 TaskFP disassembly, and byte-matches every repeated packaged program to
an already checked one. Configure `oracles.rpcs3.shred/fragment` as documented in
CONTRIBUTING. Full covers all 181,851 occurrences (3,267 distinct microcodes);
smoke visits eight shader-bearing archives. Typed sources, constants, texture
units, destination masks, END and IF boundaries match the fields represented by
the oracle. Its omitted flags and known condition-spelling ambiguity remain
separate fixture checks, rather than silently treating the tool as hardware truth.
The test also analyzes possible unit-to-output/kill/coordinate dependencies and
writes `shred-fragment-flow/units.tsv` plus the source parameter-to-inline-constant
locations in `constants.tsv` under the configured scratch root. These reports
preserve program-family paths; they do not assign PBR roles or bind materials.

`read::convert::ShredReader` implements core's conversion reader for exactly the PAKs and `SourceFrame` the caller
states; the reader chooses nothing. `read` returns the whole level: placements, prototypes with their meshes and
vertex inputs, material entities, spot chunks and, for the inputs the caller adds, the `.hkw` collision of the same
PAKs (`with_collision(SurfaceTable)`, MAIN.DB's `surfaces` built by the caller) and the source materials with the
texture copies they bind (`with_materials`, below). Geometry is read with the level, so a lowering that runs on the
read world sees the meshes, and `load` returns the subset of the world it is given that the roots reach (the selected
world, not a saved copy), with the pixels of the texture copies in it decoded: a change a pass made to a mesh is what
comes back. Collision entities are not reached from placements, so a program that wants them roots them
(`shred.rigid_body`). `load_all` takes the world, decodes its texture copies in place and returns it.
`prototype_box` is the box of the world's mesh. Incremental archive I/O is not implemented. The adapter emits only
`core` and `shred` namespaces.

`with_scripts(ScriptInputs { scripts, level })` also reads the level's look from the game's scripts archive. The
archive is the PAK inside `SCRIPTS.SDAT`, which the disc stores encrypted; the caller decrypts it (primase holds no
key) and passes the plain PAK. A level's `scriptfx_level_load_<level>.lua` is Lua 5.1 bytecode that sets fog, shadow
colours and bounds, bloom, back light, ambient fall-off, the colour LUT and more by calling engine functions with
literal numbers and strings. `primase-lua51` lists its calls; every call whose callee is a global (or a field path) and
whose arguments are all literals becomes a `ScriptCall` of the `shred.level_params` world resource, in script order,
with its arguments as the script writes them. Nothing says what an argument means or its unit; no value is
hard-coded, and nothing is lowered to a core `Environment` or `Atmosphere` until the engine function is proven. A call
with a variable argument is counted in the report with its instruction; a script with control flow or nested
functions is not read (its report line says so); a level with no script is a report line. The ten levels'
scripts (charlie, delta, echo, foxtrot, india, juliet, mike, november, oscar, papa) are straight-line and all their
calls read. Proven so far from the engine's code (not applied): `change_fog` arguments 5 to 8 are the fog colour and
9 a blend value; arguments 1 to 4 reach the fog pass as the boundaries `(a1, a3, a2, a4)`.

The hand-worked quartet tests cover two resources, two submeshes of one resource, shared materials, instancing,
reflection, frame scaling/mirroring, malformed inputs, truncated metadata and refusal within a mixed quartet.
`geometry_disc` always traverses every quartet, regardless of smoke/full depth: expected totals are 657 quartets
in 324 PAKs, 21,309 resources (17,897 rigid, 3,412 embedded), 35,114 submeshes and 168,887 placements. These are checks
against source tables and bounds. The attribute gate checks every rigid normal and stream, counts the 11,488 authored
zero normals, and reports p1/median signed cosines between geometric face normals and average corner normals under
both identity and mirrored frames. The full-disc source census found 5,384,944 opposing and 1,469 agreeing triangles
at cosine thresholds -0.5/+0.5, with 23,293 ambiguous and 7,078 degenerate or zero-average-normal triangles. After
source-CW normalization, both frames must produce 5,384,944 agreeing, 1,469 opposing and 23,293 ambiguous triangles:
99.97% of the triangles with a decisive normal agree. Authored exceptions remain unchanged; the reader applies one
format convention rather than changing individual triangles to follow their normals. Both frames measure signed
cosines p1 = 0.798705 and median = 0.999549 over 5,409,706 nondegenerate rigid triangles with nonzero average normals.
Lab/game visual verification remains separate.

Processed skin source records (formerly called "STR-FF") decode for all 3,412 observed meshes: 1,375 meshes in 103
character PAKs and 2,037 meshes in 21 layer PAKs, with 3,302,877 stored vertices and 311,835 palette batches. The layer
records are male/female crowd outfits under `actors/male/male` and `actors/female/female`: each of those 21 archives
has 97 processed meshes and 97 placements across two quartets. They now have geometry. There are 3,412 processed
placements over the full corpus. These counts include packaged copies, not unique characters or an assembled outfit.

The observed format has mask `0x8007f`, processed flags `+0xb4 = 1`, a stride-36 declaration, no MGP, and MMP indices.
Its actual source vertices are 64 bytes in the MLP, at the mesh-relative `+0x9c` offset. The native preprocessing
writer (`0x430e38`, vertex writer `0x430b28`) establishes this layout; it does not establish a complete runtime bind
convention. The 64-byte envelope starts with a byte size and 60 bytes of AABBCCDD padding. Each batch has a 64-byte
header: four bone indices, BE u16 vertex count, BE u16 influence count (1..4), and DDCCBBAA padding. Inactive bone
indices are 255, with zero weights. The terminal header has both counts zero. Stream size includes envelope and
terminator and equals `64 * (vertices + batches + 2)`; the following 16 metadata bytes remain opaque and reported.

| Source vertex offset | Decoding |
| --- | --- |
| +0 | float3 source position, finite and within the stored AABB |
| +12 | four weight bytes summing to 255, for the batch's shared bone palette |
| +16 | float3 normal; original bits also retained, including stock NaNs |
| +28 | unproved u32 attribute, retained as `extra`; every observed value is zero |
| +32 | finite float4 basis candidate, retained as `skin_basis`, with no inferred core tangent |
| +48, +52 | half2 UV0 and UV1, using the same submesh remap/top-left convention as rigid meshes |
| +56 | eight source padding bytes, retained |

The reader emits the authored source positions, normals and UVs through the requested frame, without posing them.
`shred.source_skin` retains per-primitive bone indices, quantized weights, normal bits, extra word and padding;
`shred.vertex_inputs` retains the basis candidate and both stored UV sets. Submesh narrowing and geometry remaps
keep every array aligned. `LevelReport.processed_vertices` counts source vertices once per resource, before expansion
into primitive ranges. The read world includes this provenance alongside geometry; loads retain the source skeleton
through the source skin's explicit reference in World closure.

Each processed quartet's SCP supplies its named bone table: count/pointer at +0x20/+0x24, 16 bytes per record, two
unassigned words followed by file-relative name and matrix pointers. All 145 source skeletons / 18,455 bone records
retain names, words and finite affine matrices in `shred.source_skeleton`, with the original source-frame context.
Their matrices remain source provenance under subsequent geometry transforms. The +4 word is not assigned a core
parent relationship, and the matrix is not assigned a local-rest or inverse-bind meaning: the native SCP loader
relocates it, while skin-job setup supplies separate runtime matrix buffers and has an identity fallback. Runtime
rest/pose/bind and deformation conventions remain unproved. The reader reports this gap and preserves binding data
in SHRED components; it emits no fabricated `core.skeleton` or `core.skin`. Proving that mapping is the next chunk.

Four character resources have 14 authored NaN normals. Exactly four primitive normal streams are omitted; geometry
and UVs remain present, and the original IEEE bits remain in source provenance. Reports list each resource/count:
`2494c946` (4), `0c190ea7` (4), `5c6bd829` (4), `c5f5bfcf` (2). No normals are repaired or synthesized. All other source
normals are finite and unit length within 0.005. The basis candidate is preserved without orthonormalization.

`skin_disc` checks all processed meshes, every palette/weight and source-skeleton reference, these numerical
exceptions, all 97 placements in each crowd layer, and source-CW normalization under identity and mirrored frames.
The source-only face census found 4,860,801 opposing and 528 agreeing triangles at cosine thresholds -0.5/+0.5,
with 16,389 ambiguous. After winding normalization, expected agreement is reversed. Signed comparisons include
finite authored normals retained in provenance on the four omitted normal streams; degenerate faces and nonfinite
or zero-average normals are excluded. Both frames compare 4,877,718 valid triangles with signed p1 = 0.645460 and
median = 0.983114. Hand-worked fixtures check literal palettes, weights, halfs, raw normal bits,
names/matrices, range failures, remapping and loading. No runtime skinning or lowering is claimed by these checks.

## Collision

`read::collision::read_collision(&mut world, &[(label, &pak), ...], &surfaces, frame)` reads every `.hkw` entry of
precisely the supplied archives (the program chooses a level's base and layers) through `primase_havok::packfile`
(layouts and censuses: `toolkit/havok/README.md`). `SurfaceTable::from_main_db` reads MAIN.DB's `surfaces` table
(54 rows, `id` 0..53 in order, `NAME`) through `read::database` (`quantydb-sqlite`); the world's `core.surfaces` is that table, so a
surface id is its index. There is no skate. mapping here.

One entity per leaf shape of each rigid body (12,641 bodies, 12,672 leaves in the 482 `.hkw` of the PAKs; the 8
ragdoll `PHYSICS/*.HKX` are not level collision), with the body's transform and `shred.rigid_body` (name, motion
type, response type, friction, restitution, collision filter word) and the shape in the body's space.

- **Fixed bodies** (motion type 5; 2,021) are static collision: a mesh leaf is one `core.collision` soup of its
  subparts in stored order (646,081 triangles on the disc, all in fixed bodies), any other leaf a `core.collider`.
- **Other bodies** (motion type 2 or 3) are kept as data and never merged into the static collision: their shape is
  `shred.dynamic_shape` (a soup or a collider), their motion type is in `shred.rigid_body`.
- **Triangle material word `a`**: surface id `(a >> 3) & 0x3F` indexes `core.surfaces`; the rest of the word is the
  opaque per-triangle `shred.collision_flags` (bits 0x1, 0x4, 0x800, 0x1000, 0x2000, 0x4000, 0x80000000 occur; their
  meanings are not established and nothing names them). Shapes without triangles carry no surface: they are surface 0.
- **Shapes**: convex-vertices as `Shape::Convex` of the vertices (the stored planes are not exact faces); box,
  sphere, capsule and cylinder (its cylinder radius) as core's primitives, so nothing is tessellated; `ConvexTranslate`
  offsets its child, lists and MOPP trees hold their children. Havok's convex radius (at most 0.05 in the data) is not
  added to any shape.
- **Winding** is the data's: counter-clockwise-front, core's convention, so no triangle is reordered (a frame that
  mirrors reverses them, as for meshes). It is the render geometry's opposite (render triangles are clockwise against
  their stored normals). Proof across the disc (`collision_winding_is_counter_clockwise_front`): the topmost
  near-horizontal triangle of each 2 m cell of a file's fixed meshes is an exterior surface and must face up; 40,368
  such cells face up and 4,103 down, and each of the 29 files with at least 200 cells is mostly up (the 18 files that
  are mostly down are the small layers, under 200 cells each: sloped or one-sided pieces).
- **Frame**: collision is in the same space as the SCP placements and render geometry (identity). On MIKE, the median
  distance from a collision vertex to the nearest render surface point is under 1 cm.

`collision_disc` checks the disc against an independent Python reader's census (`HK.md`): per-file bodies and fixed
triangles equal for all 482 files (pinned for the ten level bases and CHARLIE's training layer), the surface
histogram (36 ids) and the flag-word census equal, every world verifies, and a mirroring, scaling frame moves points
and radii and reverses winding.
## Scene

`read::scp_tables::decode(scp)` (called by `scene::decode`, result in `Scene::tables`) reads the SCP's node table
(0x70-byte records: id, optional bounding-sphere words, child and placement links resolved to indices), its
world-matrix table and the blob section chain. A section is `{flags, size, count, data offset}`; the data offset is
0x10 except 0x20-0x40 for type 0xE, and the chain ends in a 16-byte `{junk, 8, 0, 0}` record: all 411 level SCPs with a
blob walk to it (136 have no blob). Type 2 sections are spot chunks (0x200 bytes + a 64-byte world matrix each); every
other section is listed with its flags and reported unread until a reader decodes it (types 6, b, d, e, f and 10 are not). Nodes stay
in `Scene::tables` and are counted in `LevelReport`; `read_level` puts each spot chunk in the World as an entity with
its world transform and `shred.spot` (class hash and, for the 14 proven class names in `scp_tables::CLASS_NAMES`, the
name; shape; name hash; ids; node index). `Teleport` chunks also carry a core `Marker` (kind `teleport`): the class
name is proven, the facing is not (the frame is left-handed, above; which local axis is forward is unverified), and the report
says so. Twelve class hashes name no proven class and are kept by hash, counted in
`LevelReport::unnamed_classes`.

`read::manifest::Manifest::new(db, &[(file, &pak)])` lists the `LEVELS` and `LEVELS/LAYERS` archives: bases, layer
archives (`0xDSZZ` name: difficulty nibble, `session_types.id`, map id; quartets they hold) and MAIN.DB's `levels` and
`session_types` rows. `map_ids(level)`, `common(map)`, `session(map, difficulty, type)`, `layers_naming(level)` and
`base(level)` filter; nothing is chosen. The row-to-map join is the level's Lua load script (inside `SCRIPTS.SDAT`),
which is not read: the program picks the map id among `map_ids`. `drop_spots(db, level)` reads `<Level>_dropspots`;
`resolve_drop_spots(world, rows)` returns, per row, the spot chunks whose name hash is sdbm of `SPOT_NAME` (as written
or lower-cased): 183 of the 194 rows resolve, all to `Teleport` chunks.

`scene_disc` recounts every level from the disc: 547 SCPs, 166,089 nodes, 167,503 placements, 53,015 spot chunks
(538 Teleport), no chain stop, and the per-level spawn resolution.

### Splines and lights

Five section types hold splines (`read::scp_splines`, 56-byte records whose pointers are section-relative; the
points are already in level space and the record's matrix is not applied). They are the game's five spline classes:
the executable registers the loaders of types 0, 1, 9, 0xA and 0xC with one spline manager (a pool of 5 classes of
1,000 records of 56 bytes) under the class indices 0 to 4, and the low half of the section flags varies and means
nothing. Type 0 is the `rail_candidate` splines (2,334), type 1 the `guide` ribbons and flythrough cameras (2,200),
type 9 the `ai_path` network (25,250), type 0xA the `nav_path` splines of the navigation layers (3,760, long) and type
0xC the `short_path` splines (31). Each is an entity with a core `Curve` of that kind (only with two or more points)
and `shred.spline` (id, parent, counts, words, surface id, matrix, per-point parameters). The recount over the disc is
33,575 splines; SCENE.md's 31,105 total and the 29,560 of the first reader (which took only low flags 0x80 and 0x81 of
types 0 and 1) do not match it. All 33,575 records lie inside their sections.

Rail candidates are not called rails. The evidence for it: record word 9 is a MAIN.DB `surfaces.id` (0 or a row of the
table, for all 2,334; the snow surfaces occur only on the snow levels), the other four classes have words 7, 8 and 9
all zero, their parent groups are nodes named `grind1` to `grind3` and one spline is named `Grinds`, and the DB's
GRIND-task volumes contain candidate points more often (76% of 212) than other task volumes (39% of 803). The engine
sets a character's rail position by Lua `SetRailData`, which writes a spline index into a per-character state, but no
code was found that gives a character the class of these splines, and the grind-surface setter (`GRIND_SURFACE` when
the skater's physics state is RAIL) takes its surface key from a collision hit record. The hypothesis (grind rails,
surface from `surfaces`) is stated, not proven.

`read::names` names splines whose id is sdbm of a `NavGuide_<A..ZZ><0..39>` name (1,897 on the disc) or of a
`FLYTHROUGH_RAIL` name from `<Level>_sessions` (300), as written or lower-cased; the rest stay unnamed.

Type 4 sections are light records (`read::scp_lights`, 80 bytes + a 64-byte matrix each): colour, name hash, three
parameters, a kind code (0: 5,251; 7: 4,079; 8: 398; 1: 23; 4: 23; 3: 4), links and a leftover text buffer. They are
`shred.light` entities with the record's transform, not core lights: units, range, cone and the shining axis are not
established.

Type 3 sections are spot cameras (`read::scp_cameras`, 48-byte records + two 64-byte matrices each; 281 records in 10
base SCPs). Each record has the hash of a spot name (`BigJumpOne`, `Chlg17_ObjSpot_Cam12_AnyTrick`: the names of
the database's spots), an id and parent, an f32 (53.638 in all 281) and a flag (1 in all 281), and offsets of two
frames. The engine's handler for the section copies them into the object with the record's id, so they are placements
of two frames; "camera and the point it looks at" is read from the names, the function that uses them is not traced.
They are `shred.spot_camera` entities whose transform is the first frame, with the second as `target`. Type 5 holds
one record (the `echo` level's): class `RMLocator`, kind `Waypoint` (both by hash), a name hash and a matrix. It is a
`shred.locator` entity with a core `Marker` of kind `waypoint`.

## Material banks and texture dependencies

`ShredReader::with_materials(MaterialInputs { banks, assets, textures })` attaches source materials to the
existing material entities. `banks` is MATERIALS.PAK; `assets` supplies additional archives, alongside the
geometry PAKs already given to the reader. `textures: TexturePreference::FullChainThenPrefetch` explicitly
requests the full mip chain where present and a prefetch tail otherwise. Every packaged copy remains data.
The geometry-only constructor remains available and reports that material inputs were not supplied.

`read::material_join::MaterialLibrary` also exposes the source join directly. It reads all three MBP/MDP
banks, resolves a nonzero ID only when it has one bank candidate, and retains unresolved candidates in
`shred.material_resolution`. On the PS3 disc BLUS30542 version 01.00, all 34,469 nonzero submesh references
resolve uniquely: 26,669 environment, 1,521 character and 6,279 actor occurrences. The 645 ID-zero references
have three bank candidates each; their quartet namespaces identify 514 environment (`levels`), five character
(`characters`) and 126 actor (`actors`) occurrences. Applying this namespace convention to zero is **inferred**,
supported by every nonzero reference with no exceptions; it is not a native-loader observation. Both the
resolution component and report identify the inference. An unknown zero namespace stays unresolved. PAK kind
alone cannot select a bank: LEVELS PAKs contain both environment and actor resources.

The bound PSP's own named constants populate parameters. Cg sampler names are absent from these program
tables. Four material sampler names instead have exact sdbm matches in the executable's own NUL-terminated
strings (decrypted executable SHA256 `27918ced575f802a5e372622cfa3899789b8af4490c53b07084c7a99b9431622`):

| Hash | Engine string | Slot occurrences across all 4,102 materials |
|---|---|---:|
| `95591c40` | `Color_Map` | 3,995 |
| `d79d6be4` | `Normal_Map` | 3,948 |
| `ffc34d98` | `Spec_Map` | 2,998 |
| `2538db2e` | `Effect_Map` | 2,816 |

These name 13,757 of 30,421 slots. The other 16,664 slots (17 hashes) remain unnamed. Samplers retain
`name_source: ExecutableStringSdbm`; the component version is 2. The dictionary search covers the executable,
all 1,278 decoded Lua chunks and all 743 VSP, 765 PSP and three MBP archive occurrences. Matching a name does
not establish its shader operation, hardware texture unit, colour space or normal-map convention. No `slot i`
to texture-unit assumption is made. The reader retains every state word without deriving blend or PBR state.

`shred.material_textures` links sampler ordinals to `shred.source_texture` entities. Empty source paths, paths
unavailable in the supplied archives, and headers without pixels remain explicit. The full bank census has
15,319 empty bindings, 14,117 `.thp` matches and 985 missing nonempty references (91 character, 894 environment);
these are source facts, not substituted files. A reader's unavailable-path report describes its caller-supplied
archive scope, which may be smaller than the disc. All source headers, sections, residencies, first-mip indices,
cube tails and copy identities are retained. Texture references participate in World closure before any pixels
load, including dependencies of the prototypes already present in the read world.

`read()` creates the copy graph without decoded texture payloads. `load()` decodes all reached copies into
core Texture components, one entity per cube face, through the GCM reader. Values are raw linear samples with
role Other; the reader makes no shader colour-space claim. Volumes outside this 2D material API preserve
their encoded payload in `shred.encoded_texture` and emit a notice; `decode_volume` remains available separately. Malformed supported layouts are errors, not fallback data.
Shading and PBR conversion remain separate passes. This chunk supplies no shaded-render or runtime claim.

Hand-worked join fixtures check exact pixels, both full and prefetch copies, preference, dependency closure,
missing paths/pixels, ID-zero separation, inference reporting and unsupported-versus-malformed payloads.
`material_join_disc` checks every submesh ID against its bank and loads material/texture closures of the first
20 placements in CHARLIE (base plus default layer), OSCAR and MIKE. These are source and pixel integration checks,
not independent appearance evidence.

## PBR material approximation

Register with `primase_shred::register(&mut registry)`, then select
`lower-shred-materials{normals=omit,alpha=opaque}` through the pass manager **after** `reader.load`.
Both options are required. `normals` accepts `omit`, `rgb-up`, `rgb-down`, `ag-up`, `ag-down`;
`alpha` accepts `opaque`, `named`. These are explicit interpretations of unproved source state,
not native shader execution. `ag` reads X from post-remap alpha, Y from green and rebuilds positive Z;
`down` inverts Y. Converted normal views have core's glTF convention and include every loaded mip.
An AG interpretation still does not prove which source tangent axis each component follows.

The pass retains `shred.material`, its ID/resolution and the entire texture-copy graph. PBR references
new views of the preferred loaded copy, shared across materials using the same source face and role.
It never changes raw source texture bytes or metadata, picks a different packaged copy, or silently
decodes pending pixels. A pending preferred copy is an error naming `reader.load`; an absent path,
header without pixels, encoded volume or cube map is reported without inventing a 2D texture.
Existing `pbr.material` values are authoritative and left unchanged, counted by the pass.

| Source | PBR mapping | Evidence and limits |
|---|---|---|
| `Color_Map` | sRGB base colour, UV0/repeat | Exact executable-string sdbm name; shader operation, transfer and sampler state remain approximations. Exception: `env_water` binds the same normal texture to Color/Normal, so its Color_Map is not treated as colour. |
| `Normal_Map` | Program-selected encoding, or omitted | The name is exact. One checked `char_fabric` microcode reads expanded green/alpha and reconstructs a third component; binding hash to hardware unit and basis orientation are not established. No convention is selected automatically. |
| `Spec_Map` | sRGB `KHR_materials_specular` colour | Exact name, approximate channel/transfer and reflectance model. It is not silently packed into metal/rough channels. |
| `Effect_Map` and unnamed slots | Preserved as source data, unmapped | No emissive or other operation is established by the name. Each nonempty slot is reported with shader family and ordinal/hash. |
| `BaseColor` in character families; `PrimaryColor` in design; `SolidColor` in env_solid | RGB base factor, alpha 1 | Named in bound PSP; multiplication and linear colour are approximations. Values outside PBR's factor range are clamped and reported. Stored unused w is not opacity. |
| `baseWaterColor` in env_water | RGB base factor | Approximation of static water; animated normals, reflection, murkiness and blending are not reproduced. |
| `GlassColor` in env_glass | RGB base factor; w as blend alpha only with `alpha=named` | Named parameter and family; refraction/Fresnel and native blend state are not reproduced. `alpha=opaque` leaves alpha 1. |
| `Alpha_test_ref` in seven environment families, only with `alpha=named` | Mask with sampled alpha >= x | Requested heuristic for env_base, env_base_specular, env_base_detail, env_base_detail_specular, env_tree_frond, env_tree_leafmesh and env_uvanim. Source census: 121 material occurrences. Native enable/comparison and selected permutation remain unproved. Other families stay opaque. |
| Unknown state, gloss and metallic | Single-sided, dielectric 0, roughness 1 | Named approximations on every material; no unknown state word or exponent lane is assigned a native meaning. |

Every approximation is attached to its material in the pass log and names the source fragment shader.
All parameters not mapped, including unnamed hashes, are counted individually as omitted from PBR;
the source values remain intact. Missing colour textures produce a reported factor-only material.
Emission remains zero until a shader operation is established; an effect slot is not proof of emission.
Detail/overlay/AO, dynamic fades, animation and lighting are source data rather than silently invented PBR terms.

Bounded shader evidence prevents two tempting name rules. `env_base` microcode SHA256
`29488e5d43b53f54439931471d8aa79c7e619186eba01e3e590e00483f4eefd5` adds ColorBias
to TEX0 RGB with saturation at 0xc0 and blends toward ColorConstant using a dynamic fade at 0x120/0x160.
Consequently ColorConstant is **not** an unconditional base multiplier and is left unmapped.
The same checked program does not prove that its TEX0 corresponds to every record's Color_Map.
`actor` microcode `ff10a62fba90c489c933f25ff33049917f9ee288ed8e154ad382d81d2c817076`
discards TEX0 alpha strictly below Alpha_test_ref.x (0x120/0x140/0x170), but checked effect_params and
char_matcap programs use the same constant without that sampled-alpha kill. Constant presence alone is
therefore insufficient: the environment heuristic is opt-in and explicitly approximate.
SpecExponentValues can control lookup LOD; no direct Phong-to-roughness rule is applied.
These are observations of particular programs, not claims about every shader permutation.

Hand-worked tests run through a pass manager with effect/value checks. They cover colour/normal mips,
AG reconstruction, green inversion, preferred copies, shared views, unchanged raw graph, water and alpha
exceptions, missing/encoded/pending pixels, ambiguous fields and mandatory options. `material_lower_disc`
loads the first 20 placements in CHARLIE (base plus training), OSCAR and MIKE and checks both omitted normals
and the explicitly selected AG-down approximation. This is reader/pass integration, not appearance proof.
Shaded lab comparisons with the textured Blender references belong to the lab integration chunk.

`lower-shred-fog{metres-per-unit=1}` turns the level script's `change_fog(a1 .. a9)` into core fog: `Linear { a1, a2 }`
in metres (the option is the reader's frame scale, stated again because a pass has no frame) with colour `a5, a6, a7`
as stored. It needs `shred.level_params`, gives no fog when the script never calls `enable_fog`, when `a8` (opacity) is
0, or when it sets no fog, and refuses several `change_fog` calls, `a1 >= a2` or `a1 < 0`. The game's fog is the
fullscreen pass `shaders\pixelsfxog.psp`, whose microcode was decoded whole: with `dA` the surface's view-space z,
`dB` its world height and `S(t) = t^2 (3 - 2t)`, `tD = sat((dA - a1) / (a2 - a1))`,
`tH1 = sat((dB - a3) / ((a4 - a3) / 2))`, `tH2 = sat((a4 - dB) / ((a4 - a3) / 2))`,
`f = S(tD) S(tH1) S(tH2)` and `out = scene + f a8 (fogColor - scene)`. The pass drops and reports `a8` below 1,
the smoothstep, the height window, view-space z against radial distance and the colour encoding. That MainInv, which
unprojects the depth, is the inverse projection is inferred (the shader divides by w, the engine patches only the
height plane), not read from a camera writer.
