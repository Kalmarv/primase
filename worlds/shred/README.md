# primase-shred

Tony Hawk: SHRED (PS3, 2010), RME archives and rigid scene geometry.

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
submesh partitions. Unknown formats fail with the file and byte offset. Stride-36 embedded models are individually
refused, retaining their identity, submesh material references and placements; rigid siblings still decode.

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
keep these arrays aligned with the mesh, and deferred loading restores them alongside geometry.

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
cosines p1 = 0.798705 and median = 0.999549 over 5,409,706 nondegenerate triangles with nonzero average normals.
Lab/game visual verification remains separate.

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
other section is listed with its flags and reported unread until a reader decodes it (types 1, 3, 5, a, c, e and 10 are not). Nodes stay
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

Three section kinds hold splines (`read::scp_splines`, 56-byte records whose pointers are section-relative; the
points are already in level space and the record's matrix is not applied): type 9 is the `ai_path` network (25,250),
type 1 with low flags 0x80/0x81 the `guide` ribbons and flythrough cameras (2,083) and type 0 with 0x80/0x81 the
`rail_candidate` splines (2,227). Each is an entity with a core `Curve` of that kind (only with two or more points)
and `shred.spline` (id, parent, counts, words, surface id, matrix, per-point parameters). The recount over the disc is
29,560 splines; SCENE.md's 31,105 total does not reconcile with it.

Rail candidates are not called rails. The evidence for it: record word 9 is a MAIN.DB `surfaces.id` (0 or a row of the
table, for all 2,227; the snow surfaces occur only on the snow levels), the AI paths and guides have words 7, 8 and 9
all zero, and the DB's GRIND-task volumes contain candidate points more often (76% of 212) than other task volumes
(39% of 803). Against it, no engine code was found that reads the section flags: the grind-surface setter
(`GRIND_SURFACE` when the skater's physics state is RAIL) takes its surface key from a collision hit record. The
hypothesis (grind rails, surface from `surfaces`) is stated, not proven.

`read::names` names splines whose id is sdbm of a `NavGuide_<A..ZZ><0..39>` name (1,897 on the disc) or of a
`FLYTHROUGH_RAIL` name from `<Level>_sessions` (186), as written or lower-cased; the rest stay unnamed.

Type 4 sections are light records (`read::scp_lights`, 80 bytes + a 64-byte matrix each): colour, name hash, three
parameters, a kind code (0: 5,251; 7: 4,079; 8: 398; 1: 23; 4: 23; 3: 4), links and a leftover text buffer. They are
`shred.light` entities with the record's transform, not core lights: units, range, cone and the shining axis are not
established.

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
load, including dependencies of prototypes whose geometry is deferred.

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
