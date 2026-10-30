# primase-fromsoft

What primase makes of FromSoftware's data (Sekiro, Elden Ring): the area and character readers that build a world
from what `fromsoft-formats` (`https://github.com/Kalmarv/fromsoft-formats`) reads, the components they make, and the passes that lower
them. The archives, binders and formats themselves, and the archive keys a caller supplies, are described in
`https://github.com/Kalmarv/fromsoft-formats/blob/main/README.md`.

## Gates

Archive/container unit tests live in `fromsoft-formats`; this crate checks world reads and lowerings.
Real-data tests skip when their declared inputs are not configured in `GAME_DATA`. Use
`cargo test --profile gate -p primase-fromsoft -- --nocapture`; the gate profile keeps debug assertions enabled.

- `installs.sekiro`, `keys.sekiro`: directory containing the five BHD/BDT pairs, and the folder of their keys (`Data1.pem`
  to `Data5.pem`).
- `tools.oodle6`: explicitly trusted version-6 native library.
- `oracles.uxm.sekiro/dictionary.txt`: optional path dictionary for the named-count gate.
- `oracles.soulsformats.sekiro/census`: directory of independent reference decoder
  output (SoulsFormats 9c0da6b with UXM), generated from the same installation. Its files are:
  - `<name>.plain` per archive (`Data1.plain` ... ; named after the stem's last
    component): complete RSA-unwrapped headers.
  - `entries.tsv`: shard, eight-digit hex hash, dictionary path (empty when the
    dictionary has none), decoded length, uppercase SHA-256, tab-separated.
  - `binders.tsv`: shard, hash, zero-based member index, signed ID, member name,
    decoded length, uppercase SHA-256: top-level BND4 members.
  - `errors.tsv`: shard, hash, the reference's own message: the entries it
    cannot decode. Empty when it rejects nothing; never a list of ignored entries.
  - `samples.tsv`: hash and archive path, tab-separated; corresponding exact
    decoded bytes in `samples/<hash>.bin`. Include varied entries and every
    indexed path for a selected map area.

- `installs.elden`, `keys.elden`, `oracles.uxm.elden/dictionary.txt`, `oracles.soulsformats.elden/census`: the same for
  Elden Ring (its `Game` folder, its key folder, UXM's dictionary, and a census in the same schema written by
  SoulsFormatsNEXT ee1dd61 with UXM's keys, RSA and 64-bit hash; hashes are 16 hex digits). `tests/census.rs` runs
  one body for each game configured. The dictionary must name exactly the entries the reference names.

The census attempts every indexed entry. Accepted results must match reference
lengths and digests; samples compare actual bytes. A rejection counts as
agreement when both readers reject the same entry; their messages are an
independent decoder's own words and are reported side by side, not compared.
Unexpected acceptance, a rejection the reference does not share, or a missing
reference entry fails the differential gate. Reports distinguish decoded entries from agreed
rejections; agreement does not make a corrupt stock entry readable.

## Texture packages and split binders

`Binder::read_split(header, payload, decoder)` shares the binder file-table,
name, hash-table, decompression and coverage checks with the single-file reader.
It checks the BHF4 and BDF4 halves independently.

`tpf::Package::read(bytes)` parses PC TPF containers, retaining each texture's
DDS, name, format, dimensional kind, mip count, flags and optional metadata words.
Section padding must be zero; PC producers use both name-end-relative and
first-payload-relative data sizes. Unknown platforms and compressed PC texture
records return errors. UTF-16 and ASCII-compatible legacy names are supported.
`sekiro::tpf_image(&texture)` imports 2D DDS through `primase-dds`, preserving encoded
mips and colour-space tags. Cube/volume payloads are retained by the container
reader but explicitly rejected by this 2D import. Bindings assign semantic roles
and normal conventions; package parsing does not infer them from a name.

The TPF corpus gate reads its own oracle directory, `oracles.soulsformats.sekiro/tpf`,
declared as a live gated input in [tests/inputs](tests/inputs/README.md), with
`packages.jsonl`: one object per discovered package, with `id` (shard/hex
hash, followed by `/b<index>` or `/s<index>` for single/split binder children),
`area`, package size/SHA256, platform/flags/encoding, and a `textures` array.
Each texture records name/format/kind/mipmaps/flags, DDS size/SHA256 and, for area
2D images, width/height and decoded RGBA SHA256. Area DDS inputs reside at
`area-dds/<uppercase-sha256>.dds`; none belong in the repository.

The pixel gate additionally requires `oracles.scripts.sekiro/dds`, a folder holding
an explicitly trusted .NET assembly, `DdsWorker.dll` (BCnEncoder.Net 2.2.1), with the
libraries it loads, launched through `tools.dotnet` as `<dotnet> <folder>/DdsWorker.dll`. For each DDS path on stdin it
writes a binary response to stdout: status byte (0 success), little-endian u64
length, and that many RGBA8 bytes (UTF-8 error text for nonzero status). Diagnostics
must go to stderr. It uses the independent C# decoder with BC1 punch-through
alpha enabled; BC4 grayscale presentation is normalized to red-only by the gate.
Pillow 11 is queried through the shared texture-codec test oracle, run by `tools.python_pillow`. Production
D3D pixels must match bcdec exactly. C# and Pillow are independent value oracles:
each channel may differ by at most one level. The gate reports exact/within/failed
texture counts and channel-error histograms per format and oracle. It does not
select alternate production arithmetic to reproduce an oracle's convention.

## FLVER geometry

`flver::Model::read(bytes)` retains meshes in file coordinates, face-set flags,
vertex layouts, material texture bindings and repeated vertex attributes. Byte4C
UVs retain both their signed-short interpretation and their original four bytes
with the destination UV slot, so layer weights do not lose their provenance.
The supported container is little-endian Unicode FLVER2, versions `0x20013`,
`0x20014`, `0x20016` and `0x2001a`. Other variants return errors.
Elden Ring's files may be zero-padded after the data block to the next 2048-byte
boundary (other trailing bytes are refused). A mesh with neither face sets nor vertex
buffers is read as empty; its bounds are the exporter's sentinel (-inf, NaN) and are
read, not required finite. A face set's level of detail is bits 24 to 26 of its flags
(`FaceSet::kind`, `is_lod0`): Sekiro's face sets use levels 0 to 2, Elden Ring's also 3
and 4 (115 and 62 face sets in the install, both SoulsFormats censuses), the coarse
stand-ins its assets carry for far views; `lower-fromsoft-geometry` gives every level a
`core.lods` form. `flver::is_speedtree` recognises SpeedTree FLVERs (0x8000 in the high
half of the header word at 0x68). A SpeedTree layout member's first word is its stream
(low half) and a modifier (high half): 0x8000 marks a member of a stream the file does
not store, offsets count per stream (`Member::stored`, `Member::offset`). Elden Ring's
119 SpeedTree models all declare the same unstored stream, a `Float4` position and two
`Float4` UV members: the per-instance data the game supplies at draw time (translation
and scale, two basis vectors whose cross product is the third, and in the first's `w`
the blend between the stored coarser position and the finest). The data block holds
exactly the stored stream's buffers, the face indices and alignment. A vertex buffer's
stride is its stored members'; `Model::read` decodes them into `Vertex::speedtree` as the
input assembler gives them to a vertex program (`Half4`/`Half2` 0x2e/0x2d as half
floats, `UByte4Norm` 0x13 as x / 255, `Float4`/`Float2` as stored). In these files
(header word at 0x68 with 5 in its low half) a GX list terminator's length counts its
padding only, as SoulsFormatsNEXT reads it. SoulsFormatsNEXT ee1dd61 refuses every one
of the 119 ("Index was out of range"), so no independent decoder checks the members.

What the members are is the game's SpeedTree vertex programs' (`flver::speedtree_layout`):
`Half4` UV2's `xyz` is the position (its `w` and UV3's `zw` the coarser level's position,
which the programs blend toward by the instance's blend), `Half4`
UV3's `xy` the texture coordinate, the members at the program's last two `TEXCOORD` inputs the normal and the tangent
(`x * 2 - 1`), for all but three meshes the layout's two `UByte4Norm` members;
`Model::read` sets the vertex's position, normal, UV0 and
tangent from them, the tangent in the ordinary programs' convention (`T × N`, `w` 1:
an ordinary pixel program bends its normal by the normal map's `x` along `w (N × T)` and
`y` along `T`, a SpeedTree one by `x` along `T` and `y` along `-w (N × T)`). Each mesh
is one level of detail (its one face set's flags: 0 to 2, and 3 for the billboard
card), so the geometry lowers as an ordinary model's. The billboard cards (`Float4`
UV2, `Float2` UV3, shader `SpeedTree_billboard`) are turned to the camera by their
program, which also picks their atlas image by the view's angle: they have no position
of their own, `geometry::decode` refuses them, and their material is the `billboard` class
(`leave-out=billboard`). `tests/speedtree.rs` runs every SpeedTree mesh's own G-buffer
vertex program (lifted, on the executor) on each of its vertices with an identity
instance at the finest level and the wind off (the wind block's fourth row's `y` set to
1, with which every program outputs the stored position as it stands), and requires the decoded position and UV to be its
outputs exactly and the normal and tangent within 1e-6. The programs of the three
meshes of AEG801_258 (placed 194 times) drawn with `SpeedTree_5030f_w31a187` and
`SpeedTree_5030ff8_w31a187` read the inputs the eight-member layout holds (UV2 to UV4,
the normal at TEXCOORD5, the tangent at TEXCOORD6, the same lanes as
`SpeedTree_5010f_w312187`'s), but these meshes' layout has a fourth `Half4` (UV5) before
its `UByte4Norm` pair (UV6, UV7). Nothing in the layout marks it (its member words are
those of every other stored member, the indices run 2 to 7), and binding by index is what
reproduces every other mesh exactly, so the game gives those programs UV5 as the normal
and UV6 as the tangent: UV5 is all zeros in two meshes (the constant (-1, -1, -1) once
unpacked), and in the third `x` runs from 0.28 to 15.6 with `y` 1, not unit directions.
`Model::read` sets the layout's pair (`Model::speedtree_frame`, a unit normal and a
perpendicular tangent whose normal follows the faces); `lower-fromsoft-geometry`'s
`speedtree-frame=program` replaces it with the members the material's program reads
(`program_frame`: the last two `TEXCOORD` inputs of its `<shader>_Gbuf.vpo`;
`Model::set_speedtree_frame`), so these meshes are lit as the game lights them, and
`speedtree-frame=authored` keeps the layout's pair. The test takes each mesh's frame from
its own program the same way, compares every mesh, and lists the three. The
collision of the same assets checks the frame: the position as stored lies nearest its
own collision (101 of the 106 with collision; for the other five the best of the 48
signed axis permutations is within 0.02 m of it), mirrored in z farther (median 0.165 m against 0.064 m).

`geometry::decode(&model, mesh, face_set)` compacts one face set's vertices in first-use order, converts through the
core frame and normalizes stored normals; `Part::repair_winding` chooses the winding from the summed
geometric/stored-normal agreement, `Part::drop_degenerates` retains the vertices of faces that fail the area check,
and `Part::apply_uv_scale` applies a material's main-layer UV scale. `lower-fromsoft-geometry` makes each of these
choices explicit. None of them bakes layered materials or splits parts into baked weight levels.

The archive oracle directory (`oracles.soulsformats.sekiro/census`, `oracles.soulsformats.elden/census`; `https://github.com/Kalmarv/fromsoft-formats/blob/main/tests/flver_census.rs`
runs one body for each game) adds `flver/models.jsonl`, one record per FLVER2
found in the archive and its binders (a FLVER the reference refuses: `id`, `sha256` and `error`; ours must refuse
it too), `id` the shard/hex hash followed by
`/b<index>` or `/s<index>` for single/split binder members. Each record contains
the input SHA256, version and meshes. Each mesh records its material, vertex
count, vertex digest and face sets (flags, strip/culling booleans, index digest
and triangulated digest). Vertex digest input is little-endian:
position f32x3, normal f32x3, normal W i32, UV count u32 and f32x3 values,
tangent count u32 and f32x4 values, colour count u32 and RGBA f32x4 values,
repeated for every vertex. Indices are u32. Digests are uppercase SHA256.

Area geometry is checked against the SoulsFormats dump of every placed model
(`oracles.soulsformats.sekiro/m11_01`, `tests/rigid_census.rs` and `tests/sekiro_scene.rs`);
the earlier exporter's pre-bake geometry, once compared here, is retired with its input.

Packed normal/tangent bytes follow the consumer shader operation:
`(v - 127) * f32::from_bits(0x3c010204)`. G-buffer shader signatures and
instructions in four families (Multiple, MultipleGround, MetallicValue,
Character_AMSN) establish integer inputs and reciprocal multiplication; no
captured input-layout object is claimed. The independent reader divides by
127 instead. The census uniquely recovers each encoded byte from our exact
consumer value before projecting it to that reader's convention for hashing,
and counts the resulting differences (at most one ULP). Stored positions,
indices and other attributes remain exact. Area normal/UV comparisons allow
four ULP and report component histograms separately from exact topology.

## MSB placements

`msb::MapLayout::read(bytes)` reads the section chain, models, part names,
model references, stored transforms and MapOffset events. Unrelated gameplay
records remain opaque; their envelopes, rather than their schemas, are checked.
Names remain as stored, with part indices distinguishing repeated names.

The generations share the section chain and the record envelopes; what differs is a
`msb::Generation`, chosen by the file's section chain (its names in order), never by
the game a caller names. The number each section stores is a revision, kept in
`Section::version`, not a layout choice.

| | MSBS (Sekiro) | MSBE (Elden Ring) |
|---|---|---|
| sections | 8, ending with part poses and bone names (both empty) | 6, ending with parts |
| part header | 160 bytes, kind at 8, model at 16 | 192 bytes, kind at 12, model at 20 |
| placed geometry kinds | map piece 0, object 1 | map piece 0, asset 13 (and dummy asset 9) |
| entity block | 0x54 bytes, 16 zero at 0x44 | 0x40 bytes |
| point-light shadow byte | 0/1 | 0/0xff |
| MapOffset event | type 9 | none |
| spawn-point region | type 8 | type 8 (SoulsFormatsNEXT's `SpawnPoint`; the region header laid out alike) |

Parts already occupy the common world frame. Their local scale is followed by
Euler rotations X, Z, Y, then translation. `Part::transform` uses ordinary f64
trigonometry, rounds basis components to f32, and applies the core frame's Z
reflection for handedness. MapOffset places map-local resources separately;
it must not be added to part placements a second time.

The MSB reading is checked against an independent one: SoulsFormats' parts of the same area,
`oracles.soulsformats.sekiro/m11_01`'s `parts.jsonl` (every map piece and object part with its `name`, `kind`, model,
`position`, `rotation` in degrees and `scale`). Each part's stored values must match bit for bit (`msb_census.rs`); the
matrices derived from them are checked against the same dump by `sekiro_scene.rs`. Other part kinds and map offsets
have no independent reference yet: an open coverage gap (build one when a step 20 or in-game Sekiro difference points at this area).

Elden Ring's layouts are checked whole against SoulsFormatsNEXT ee1dd61's MSBE
(`oracles.soulsformats.elden/format`'s `msb.jsonl`, `tests/msb_census.rs`): one record per
layout the install holds, `path` (the archive path), `models` (`name`, `kind`) and
`parts`: `name`, `kind` (the reference's class name), `model`, `position`/`rotation`/
`scale` (three f32 bit patterns as hex strings), `entity_id`, `entity_group_ids`,
`cast`, `receive`, `only`, `point_light` (0 or -1), `static_cast`, `cascade3_cast` and
`hit_filter` (collision parts, else null), and `spawn_points` (every region of the
reference's `SpawnPoint` class in file order: `name`, `position` and `rotation` as f32 bit
patterns). Repeated names follow the reference's convention: later uses become
`"<name> {n}"`, in file order.

`btl::LightBank::read` reads versions 16 (Sekiro) and 18 (Elden Ring), which share the 232-byte light record, and
keeps the stored version; other versions are refused. Elden Ring's banks are checked against SoulsFormatsNEXT's BTL
(`btl.jsonl`, `https://github.com/Kalmarv/fromsoft-formats/blob/main/tests/btl_census.rs`): one record per bank the install holds, `path`, then either `error` (the
reference refuses it; ours must refuse it too) or `version` and `lights` (`name`, `type`, `diffuse`/`specular` RGB,
`diffuse_power`, `specular_power`, `cast_shadows`, `cone_angle`, `position`, `rotation`, `radius`; floats as bit
patterns).

Parts also carry their entity ID and eight entity group IDs as stored (sentinels -1 and 0 included); event scripts
address a part by any positive one of them (`Part::entity_ids`). `oracles.scripts.sekiro/state` is a folder from an
independent reader. Its `entities.jsonl` has one record per map file: `path` and `parts`, each with `index`, `name`,
`entity_id` and `entity_group_ids` (8 integers). The gate (`tests/msb_entities.rs`) requires the same part count and
names and exact values.

Each part's six MSB shadow controls are kept raw in `msb::ShadowFlags`.
`oracles.scripts.sekiro/shadow` is a folder holding one original `map.msb` and
an independent reader's `shadows.json` (`sha256`, ordered `parts` with `name`
and the six fields). The gate compares every part exactly. With
`installs.sekiro`, `tools.oodle6` and `oracles.baseline.sekiro/scene` (for its
region) it also reads area m11_01 and joins every selected placement by name.

Area assembly attaches a selected placement's raw controls as
`fromsoft.shadow_flags` and their common meaning as `core.shadows`. Cast and
shadow-only map to Off, On and Only; receive maps directly; explicit false
stays false. Shadow-only with casting disabled is contradictory: casting is
left unknown and counted. Point-light, static and cascade controls have no
common meaning and remain raw, with a named approximation. `Report::shadows`
counts selected placements, not the source map's parts. State conditions keep
or remove the placement with both sets of controls.

## Event scripts

`emevd::Script::read` reads a decompressed `.emevd` in Sekiro's layout (64-bit little-endian, version 0xCD): events,
their instructions (bank, id, argument bytes as stored, layer), parameter substitutions (which bytes of an event's
arguments fill which bytes of an instruction's), linked-file offsets and the string table. Every byte is accounted
for (tables, each instruction's arguments, strings; at most 15 zero bytes of alignment between them); a field
outside its known values, an overlap or an instruction or parameter no event owns is an error. Nothing is
interpreted. `oracles.scripts.sekiro/state`'s `emevd.jsonl` has one record per script: `path`, `sha256` (of the
decompressed file), `format`, `events` (each `id`, `rest`, `instructions` with `bank`, `id`, `args` as lowercase hex
and `layer` or null, `parameters` with `instruction`, `target`, `source`, `bytes`, `unk`), `linked_files` and
`strings` (hex). `tests/emevd_census.rs` reads every script the archive holds (the common scripts and every map
block, probed by path), requires the oracle to list the same set, and compares every value exactly.

## World states

`sekiro::Options::states` (default `None`: nothing changes) set to `Event` reads the block's event script
(`sekiro::states`). Each `2000:6` call of the part-state common functions 20004110 (map pieces), 20004112 (objects)
or 20004114 (collisions) with exactly seven arguments and no parameter substitution is a rule: the entity (a part's
entity ID or one of its groups) is disabled while any of its four (flag, state) pairs holds. `read_game_area` then makes a
bool `core.state_axis` per flag (`fromsoft.flag.<id>`, class `event`, default off) and gives each decided placement a
`core.condition`, the AND over its rules of "flag is not the state". World collision files and object collision
present only in some states are written as separate soups (`collision/game_state<n>.bin`, `water_state<n>.bin`,
`objects_state<n>.bin`), each with its condition and an identity transform, so no soup mixes them with what is
always present. Calls that cannot be decided statically, other common functions that touch parts (m11_00: 20004115,
20004120) and direct state instructions in story events are not interpreted: the parts they name keep their default
presence, even when a decided rule also names them, and `Report::states` counts them. A known call whose arguments
are not exactly seven whole words, or are filled by substitution, counts as undecidable. No independent reference
checks these rules yet (a stated coverage gap, CONTRIBUTING.md); the facts they read, MSB entity IDs and the
scripts' decode, are checked against `oracles.scripts.sekiro/state` (`tests/msb_entities.rs`,
`tests/emevd_census.rs`).

The area's placements are checked against a SoulsFormats dump of its MSB in `tests/sekiro_scene.rs` (below).

### Draw groups

`sekiro::Options::display_groups` (default `None`) set to `Axes` makes the parts' draw groups a presence axis
(`sekiro::groups`), for both games. A map piece, object or asset stores up to 256 draw groups; the community documents
that it is drawn while one of them is among the active display groups, which are the display groups of the collision the
player stands on. The read makes one bool `core.state_axis` per draw group of a kept placement
(`fromsoft.display_group.<n>`, class `display_group`, **no default**: which groups are active is the program's state) and
gives each placement the condition "one of its draw groups is displayed" (ANDed with the event-flag clause when `states`
are read). `resolve-state` then keeps what a program's groups draw: `assign=fromsoft.display_group.3:on;...`, or
`assign-class=display_group:on` for every group. A placement under no draw group at all (a zero array, which the stock
files store on 317,870 Elden Ring assets and 654 Sekiro parts) has no condition and is present in every state: that is a
choice, not a finding. A game run (Elden Ring 1.17.2, `m10_00`, loose layout) showed only that zeroing the draw groups of
every map piece and asset that had some hides the scenery. `Report::display_groups` counts the placements conditioned and
left present, those under groups none of the map's own collisions displays, how the layout's drawn parts meet its
collisions' display groups (`GroupCensus`), and each collision's display groups (the groups to assign for "the player
stands on this collision"). The running game does not draw a part by its draw groups against the collisions' display groups
alone, and a part's own display groups take part too (a layout with every part's draw groups zero and display groups set to a
group the floor collision holds draws the scenery): these axes carry only the draw groups, a documented limit. `tests/display_groups.rs` reads the Academy of Raya Lucaria with it.


## Material definitions and preview lowering

`mtd::MaterialDefinition::read` validates typed structure, markers, counts
and file bounds. Declared block lengths are diagnostic: stock values can
exceed their structurally consumed end, including the file end. The reader
retains every declared and consumed length without reading outside the file.
This structure-authoritative rule is not a claim about the game's parser control flow. Shift-JIS strings decode strictly; malformed text is
an error at its string offset. No replacement characters are introduced.

`materials::resolve` records what the source states, and chooses nothing.
Nonempty FLVER slot paths override fixed MTD paths; each binding says whether
the area's texture catalog holds its texture (the caller supplies availability).
Shader names and blend/alpha parameters state the class (water, decal, leaves,
alpha, cutout) or the skip class, and the alpha reference its cutoff.
`fromsoft.material` records these and all resolved bindings; the reader names a
source material by its MTD.

The preview a lowering draws is the passes' (`preview`): among available
albedos it chooses slot 8, then the lowest slot; normals prefer a matching
stem, then slots 3 and 0, then the lowest slot; metallic and emissive maps are
the first of their bindings. When shader-qualified names share a numeric slot,
bindings declared by the selected MTD take precedence over foreign-family
bindings; all bindings remain available to the later bake. The chosen albedo's
positive FLVER tiling scale is what `lower-fromsoft-geometry{uv-tiling=apply}`
applies. `lower-fromsoft-materials` names each source material it lowers after
its preview albedo (its MTD without one). Name-based collision surfaces
(`preview::named_surface`, which object collision uses) are a documented
approximation, not a claim about shader semantics.

`fromsoft.layered_material` additionally retains the complete MTD parameters,
texture declarations, selected static switch, UV indices and binding scales
for the later shader-aware bake. The preview does not execute layered shaders.
Metallic and emissive source slots survive even when the preview does not
consume them. `skip` names the source classes the game draws as something other
than a surface (sky and cloud cards, additive light volumes, shadow proxies); a
program chooses whether to leave them out (`lower-fromsoft-geometry{leave-out=}`). `sekiro::pbr::lower` takes metallic only from a traced source
constant or map; without either, a material is dielectric (metallic 0), not
glTF's default of 1, and is counted as an approximation. Material identity is chosen by the caller, independently of
any reference exporter's string formatting or key hashing.

`oracles.soulsformats.sekiro/materials` is SoulsFormats' reading of every MTD in
`/mtd/allmaterialbnd.mtdbnd.dcx`, in binder order, made by the dump harness's `mtd` mode (`https://github.com/Kalmarv/fromsoft-formats/blob/main/tests/mtd_census.rs`).
`mtd.jsonl` lists each one's binder name, decoded SHA256, shader, description,
typed parameters and every texture declaration; the gate compares them exactly,
f32 values by their bits. `mtd-boundaries.jsonl` lists each file's blocks as
`{start, kind, declared, consumed}` in the order they close. SoulsFormats' reader
does not keep block lengths, so the consumed length is its writer's: the harness
writes each file back, requires the rewrite to equal the file outside the
blocks' length fields, and takes the length its writer measures. The declared
length is the file's own. The gate compares every block and counts the files
whose declared lengths differ. It requires `installs.sekiro` and `tools.oodle6`;
absent inputs skip explicitly. Area material assignments and texture
availability have no independent reference.

## Material binaries (Elden Ring)

`matbin::Material::read` reads Elden Ring's material binaries (`MAB\0` version 2), which a FLVER material's
`.matxml` path names: the shader (`.spx`) and source paths, the key, typed parameters (bool, int, int2, float to
float5) and texture samplers (the shader's sampler name, the texture path, a key and two floats), as stored. A value's
extent is measured to the next stored item (a value no item follows is bounded by its type): float3 values are stored
with three floats or five (both kept), a value shorter than its type is kept raw (`Value::Truncated`), one zero byte may realign UTF-16 strings after an odd value,
and bytes after the last referenced item are kept as the unreferenced tail; any other gap is an error. Checked against
SoulsFormatsNEXT's MATBIN over every material binder of the install (`matbin.jsonl` of `oracles.soulsformats.elden/format`:
one record per member in member order, `binder` (archive path), `name`, `shader`, `source`, `key`, `params` (`name`,
`key`, `type`, `value` as strings: bools 0/1, ints decimal, floats as bit patterns; the reference keeps three floats of
a float3) and `samplers` (`type`, `path`, `key`, `unknown`); `https://github.com/Kalmarv/fromsoft-formats/blob/main/tests/matbin_census.rs`). The reference reads a short
value's declared size past the next item; the gate compares the stored prefix and counts such values. The area read
does not resolve materials against them yet.

## Shader bundles (Elden Ring)

Elden Ring's shader bundles are in four binders (`/shader/shaderbdle.shaderbdlebnd.dcx`, `shaderbdle_dlc01`,
`shaderbdle_dlc02`, `speedtree`; `Game::shader_binders`), 1,063 bundles, each name in one binder only. A bundle is laid
out as Sekiro's: `.vpo`, `.ppo`, `.hpo` and `.dpo` programs named by pass (`_Fwd`, `_FwdSdw`, `_Gbuf`, `_[A]` alpha
tested, `_<n>_` a static switch) and one `.metaparam`; the programs are Shader Model 6.0 DXIL containers that keep their
reflection names. `metaparam::Metaparam` reads Elden Ring's (SMD version 6, as Sekiro's) with the header word at 0x54
and a texture record's word at 0x20 kept as stored (nonzero in some of Elden Ring's files; the twelve bytes after it
are zero). Checked against SoulsFormatsNEXT ee1dd61's `METAPARAM` over every bundle of the four binders
(`metaparam.jsonl` of `oracles.soulsformats.elden/format`: one record per bundle, `binder`, `bundle`, `name`, the header
words `unk30`, `cbuffer_size`, `flags`, `unk40`, `gxmd_key`, `ca_block_size`, `unk50`, `ca_param_components`,
`shader_id`, then `textures`, `render_passes`, `ca_params` and `mtd_params` with floats as bit patterns;
`https://github.com/Kalmarv/fromsoft-formats/blob/main/tests/metaparam_census.rs`). Elden Ring stores colour MtdParams as type 13 where Sekiro stores 12.

## Area reading and its passes

`sekiro::read_game_area(&Install, map, ...)` reads any game's map through the same read (`AreaReader::of(install, ...)` is the
reader adapter's); an `Install` is the folder, its `Game` and its archive keys.
What differs between the games and is not stated by their files is `Game`'s: where a map keeps its files
(`map_folder`: Sekiro `/map/<map>`, Elden Ring `/map/<mAA>/<map>`), how a placed object's model is found
(`object_model`: Sekiro's `/obj/<model>.objbnd.dcx` with its HKX, Elden Ring's `/asset/aeg/<group>/<model>.geombnd.dcx`)
and source-identity prefix (`obj/`, `asset/`), the distant-backdrop series (Sekiro's `m9xxxxx`), and the larger maps
above a map (`parent_maps`: Elden Ring's open-world tiles `mAA_XX_ZZ_00` sit in 512 m `_01` and 1024 m `_02` tiles,
whose layouts place the small tile's big assets as `<tile>-<part>` in their own frame, the small tile's centre at
`(i - (p << L)) * 256 + 128 - (256 << L) / 2` metres there). `Options::parents` (default `None`, the map's own layout)
set to `Include` also reads those parts, moved into the map's frame by that offset. The map layout's generation says
the rest (part kinds: Sekiro's objects, Elden Ring's assets; spawn regions).

Elden Ring's read does not yet read world states (`states: event` is refused): it is in `Report::notices`. SpeedTree
models are placed like any other. `tests/elden_area.rs` accounts for every map
piece and asset part of the Academy of Raya Lucaria (m14_00_00_00) and of the Shaded Castle tile with its parents
against SoulsFormatsNEXT's layouts, translations exact.

`sekiro::read_game_area` reads a selected map into an empty `World`. It takes an
explicit `Install`, map name, trusted decoder and options; it never guesses paths, and a failed
read leaves the destination unchanged. The read selects source records and
converts frames; it does not lower:

- each placed model is a prototype holding its FLVER as stored (`fromsoft.flver`:
  skeleton, every mesh with all its face sets, original material indices and every
  vertex channel), its source identity (`fromsoft.prototype`, `far` for the
  distant-backdrop `m9xxxxx` series) and the material entity of each FLVER
  material (`fromsoft.material_slots`), each holding `fromsoft.material`;
- placements are `core.instance` with their MSB transform, source label, shadow
  controls and, with states read, conditions;
- every texture a kept material binds is kept as stored
  (`fromsoft.texture_payloads`), the bundle of every shader a kept material's
  definition names as an entity of its own (`fromsoft.shader_bundle`, named as
  the shader: every vertex, hull, domain and pixel program the install's
  `.shaderbdle` stores for it (in one of the game's bundle binders,
  `Game::shader_binders`: Sekiro's one, Elden Ring's base, two additions and
  SpeedTree, read in order while a wanted shader is missing), for every pass the
  game draws it with, as stored in the game's container format (`Game::shader_format`:
  Shader Model 5 DXBC in Sekiro, Shader Model 6 DXIL in Elden Ring; each
  container's shader kind is its extension's stage) and under the file name that
  says the pass, plus the shader's `.metaparam`:
  texture slots, the samplers each render pass binds per stage, the parameters
  and their constant-buffer offsets) linked from the material by
  `fromsoft.material_shader` (absent where the install holds no bundle of the
  shader: the `GXFlver_*` shaders; `Report::missing_shader_bundles`), and the
  G-buffer programs of its layered materials (`fromsoft.shader_programs`, for
  the layer bake);
- each high-resolution map collision file that MSB collision parts name, of every
  hit filter, is a `fromsoft.map_collision` entity: the naming parts (name, hit
  filter, clause with states read), every triangle in the world frame and its
  HitMtrl ID as the shape key binds it (-1: none); with `Options::f_collision`
  the file of the same model in the area's `f` collision binder (`f<area>.hkxbhd`:
  extern meshes, `hknpExternMeshShape` over `hkGeometry`, no HitMtrl) is read with
  it, as `f_triangles` in the same frame;
- each object prototype with collision carries every stored collision file with its
  resolution (`fromsoft.object_collision`: Sekiro's main HKX in the object's binder, high;
  Elden Ring's `aegnnn_mmm_h.hkx` and `aegnnn_mmm_l.hkx` in the asset's `_h`/`_l`
  `.geomhkxbnd` binders, high and low: every collidable asset has a low one, a subset a
  high one) and in each every body (name, stored motion type, shape class and its
  geometry as stored: a compressed mesh's triangles in the prototype's space and
  their HitMtrl IDs; a convex shape's core (vertices, planes, faces), convex
  radius, a capsule's segment, and the map from its own space into the
  prototype's; convex shapes store no material);
- with `Options::effect_lights`, the point and spot lights of effect files (FXR) are
  `fromsoft.effect_light` records where they stand: those of the effects the map's
  SFX regions name (at the region's pose), and those of the effects the kept objects
  (Sekiro: `ModelSfxParam` row `model number x 1000`) and assets (Elden Ring:
  `AssetModelSfxParam` row `(nnn*1000+mmm)*1000 + sfxParamRelativeId`) spawn at the
  FLVER dummy polygon each slot of the row names, with the effects those spawn
  (`SFXReference`) at the node that spawns them; each light's place is composed with
  the static node transforms above it (rotations Z, X, Y then the translation: the
  fxr project's order, sourced, not checked in the game). The effect is looked for in
  the area's `sfxbnd_<mNN>` binder (Elden Ring: and `sfxbnd_<map>`), then
  `commoneffects` (and `commoneffects_dlc02`). Values are as stored: colour floats
  and a keyframed multiplier as the first and the settled value, a spot's near, far
  and elliptic base. `Report::effect_lights` counts, per area, the regions and
  parts read, the lights the rows reach (`slot_lights`), those placed, those lost by
  reason (a model the archive lacks, a model of no vertex, a dummy polygon the model
  lacks) and the references that placed nothing by reason; Sekiro's rows at
  `num * 1000 + 1..=4` (which one an object uses is not stored in the object) are
  counted, not read;
- with `Options::effects`, the effects themselves are read where they stand, by the same routes (SFX regions;
  the effect rows of placed objects and assets): a `fromsoft.effect` on an entity placed at the region's pose or at
  the model's dummy polygon, the effect files as stored (`fromsoft.effect_file`, one per effect, with the effects
  they spawn) and the textures they name (`fromsoft.effect_resource` on a `core.texture` each, looked for in the
  area's binder and the common ones). `Report::effects` counts the regions, the effects placed by each route, the
  files and textures read and what no binder held, by reason. Models and animations the effects name are counted,
  not read. `read_effect` reads one effect by its number, at the origin (an effect viewer's world);
- with `CharacterOptions::clip_effects` (`CharacterReader::with_clip_effects`), each animation clip, as it loads, also reads
  the effects its events start: the events of types 96, 118 and 120, whose first field the stock files bear out as an effect
  number (types 110, 112, 116 and 119, which the community template also gives an `FFX ID`, hold small numbers or effects no
  binder has, and are counted), give `fromsoft.clip_effect_uses` on the clip (the event, the effect,
  the dummy polygon's reference id, whether it follows the dummy, whether the event keeps it going for its length, the
  event's times), and the effect files and textures come from the character's binders (`binders_for_clip`: Elden Ring's
  `sfxbnd_<cNNNN>` first, then `commoneffects` and `commoneffects_dlc02`; in Sekiro `commoneffects`, then every map binder,
  as its bosses start the effects of their map's binder), read once per reader (`EffectCache`). Events that name
  an effect another way (goods and magic rows, throw and foot indexes), and effects on a weapon's dummy polygon, are counted
  in `lost`. An event's number is the effect's number as the game takes it: one that is no file of the binders searched is
  counted in `lost` ("no effect file with this id"), not read as another (Elden Ring's player animations start `400` to
  `422` in 3,009 events, none of them a file; the engine's handler of event 96 passes the number on unchanged). The one number
  the engine changes is a blade's (event 118, Elden Ring): its handler adds the offset the character's data keeps for the hand
  to the event's number (`401001` + 10 is the file `401011`), and `CharacterOptions::trace_offsets` (`with_trace_offsets`)
  states those offsets per hand, 0 and 0 (no special effect on the weapon) by default. The weapons held are a stated input too
  (`CharacterOptions::equipped_weapons`, `with_equipped_weapons`: the part, `wp_a_<equipModelId>`, of each hand slot, read among
  the parts): a weapon read for a slot has its dummy polygons named as the events name them (`dummy 10300` for polygon 300 of
  the right hand's first weapon; `HandSlot`). Where the weapon stands on the character is not read yet: that placement has to
  come from the engine's own data, and the sockets stay on the weapon's skeleton until it does. An attack event (type 1)
  draws the trail of the first trace slot of the right hand's first weapon (`EquipParamWeapon`'s `traceSfxId<n>`), through
  the run of the weapon model's dummy polygons the slot's head and tail give as the game takes them (`trace_run`, from
  Elden Ring's executable: head to tail, or up from the head while the model has the next id where the tail is -1); a run
  of fewer than two is counted in `lost`, as the game draws no trail of it;
  `lower-fromsoft-effects` lowers the effects to unplaced systems and the uses to `core.clip_effects` on the clip,
  a spawn each, at the socket `dummy <reference id>` of the model; `Report::clip_uses` counts the uses;
- which clip an Elden Ring attack plays is `sekiro::attack_clip`'s choice, by the game's rule (2.7.2.0's executable): a
  `CustomManualSelectorGenerator` of the player's behaviour graph (`read_selector`, from `chr/<c>.behbnd`) of offset type
  13, 14 or 16 takes one hand's active weapon (16: the left where the arm style is `LeftBoth`, else the right), and plays
  the child named for the first of `spAtkcategory x 1000000 + animId`, `wepmotionCategory x 1000000 + animId` and the
  hand's fallback (`23000000`, `48000000` + animId) the character has, else `a000_<animId>`, else child 0. The player's
  state (`PlayerState`: each hand's slots, its active slot and the arm style) is read from a save
  (`fromsoft_formats::save`); which selector plays is the player's request, stated by name, and `check_request` refuses
  one the arm style contradicts (`AttackBoth` one-handed, `AttackRight` two-handed). Other offset types read other state
  and are refused by name;
- lights are `fromsoft.light` records where they stand (stored colours, powers,
  cone, radius and tangent); hang-edge segments and spawn points are the
  `fromsoft.area_extras` resource; what the read covered (game, area, region,
  margin, states) is `fromsoft.read_scope`;
- with `Options::draw_param = DrawParams::File(<stem>)`, the area's graphics
  environment is the `fromsoft.draw_param` resource: the draw-param file
  (`/param/drawparam/<mNN_NN>_<id>.gparam.dcx`, a GPARAM, `gparam::Gparam`; the
  stems the archive holds for the area's map are `sekiro::draw_params_available`)
  at the value sets the area's parts name. Both games keep a map's files under
  its first two blocks and a four-digit id (Sekiro `m11_01_0720`, Elden Ring
  `m10_00_0100`; its open-world tiles share `m60_00_*` and `m61_00_*`). The MSB
  does not name a file: a part's graphics config (`msb::PartGparam`, the part
  header's slot at +112: four value ids) names the value set inside the groups
  of the file in force (`LightSet ParamEditor`, `FogParamEditor`,
  `LightScattering : ParamEditor`, `Env Map:Editor`), and a collision part's
  scene config (+120: four signed bytes) names a file: the first byte is the
  hundreds digit of its id (the slot) and, in Elden Ring, the byte at +32 the last
  two (the sub id); the file of that id exists for all 1,304 Sekiro and 3,209 of
  the 3,266 Elden Ring collisions. `DrawParams::Collision(<part name>)`
  reads the file one collision names; the ids the area's collisions name are
  `Report::collision_ids` and the area's own events that switch a file (Sekiro's
  `2003:88`, Elden Ring's `2012:11`/`2012:12`) are `Report::switches`, not applied.
  How the engine picks one file from the collision under the player, the events and
  the weather is not established, so the file is the caller's. The id most of the area's kept placements and its
  collision parts name is read per group; a group no part names an id for is read
  at its one set, else at id 0, else not (counted). Every group and field of the
  file is read, in `gparam.rs`'s versions 3, 5 (Sekiro, Elden Ring) and 6; the
  file's comment tables, opaque block and extras are counted, not read. Elden
  Ring's `_weatherbase`, `_weatheroutdoor`, `_commonevent` layers are not read.
  What the read counts is in `Report::draw_param` and its notices.

The read is eager: it decodes every kept model,
texture, program and collision file before any part is planned, and through
`sekiro::convert::AreaReader` that read world stays resident for the whole
conversion, so a conversion in parts does not bound the source it holds by part
size. A part's load is its roots' closure with only the texture files and
programs its materials bind, sharing the read's buffers (`Arc<[u8]>`) rather
than copying them; the part-size counters (`PartSize.loaded/written`) count
components, not these resources, and are no measure of total memory. What the
read leaves out or cannot find is the reader's notices.

A region (world east/north rectangle) is an explicit partial read: a placement
is kept when its model's extent (every vertex a face set uses, static vertices
posed by their bones) reaches the region within `region_margin`; lights and spawn
points outside it are not read. Collision files are read whole whatever the
region. `Report::notices` returns what the read left out or could not find:
missing models (distinct models) and their placements, placements of models with
no used vertex, missing and unnamed collision files, EDGE kinds other than hang
edges, region exclusions, event states not requested, parts left at their
default presence, and the shadow controls' interpretation. Also counted: the water and
decal material slots kept (the shader families the class tables name: Sekiro's and
Elden Ring's `Water` shaders, `MeshDecal`), which `lower-fromsoft-materials` lowers
as a water usage and the decal draw layer; the placed objects, drawn at their model's
rest pose (their animations are not read); and Elden Ring's parts that store a grass
config (the grass is not read). `tests/material_class_census.rs` and
`tests/hit_class_census.rs` (install-gated) list every shader family by class and
every HitMtrl class by triangles, for the tables to be checked against the install.
Elden Ring's grass: a part's grass config (header +128, SoulsFormatsNEXT `GrassConfig`) is
six `GrassTypeParam` row ids; `Report::grass` resolves them against `regulation.bin`'s
grass param (`param::GrassParam`: model, simple model, flat and billboard textures, density,
spacing; Paramdex `GrassTypeParam.xml` as Smithbox ships it, rows of at least 276 bytes,
checked by `tests/grass_census.rs`) and the notices name the rows. No file holds a blade's
place (the game scatters by density), so no grass is placed. SpeedTree trees are ordinary
assets (`aeg801_*`, read with their LOD levels); their billboard cards, the far forms, have
no position of their own (their program places them by the camera and picks an atlas image
by the view angle) and core has no camera-facing primitive, so they stay left out
(`leave-out=billboard`).
`Options::object_animations` (off by default) reads the animation binder of each kept object
model (a `.anibnd` member of its objbnd/geombnd: 233 of Sekiro's 1395 objbnds hold one, none
has a binder beside it) into the world as a
Havok skeleton named `<model> havok skeleton` and `havok.animation` clips, counted in
`Report::object_animation` and the read's notices; they are not lowered onto the placements
yet (the placed objects keep their rest pose), and the clip entities of different models
share their file names (`a000_003000`), told apart by their `core.provenance`.
`https://github.com/Kalmarv/fromsoft-formats/blob/main/tests/object_animation_census.rs` (install-gated) prints what the object binders hold for
both games (Elden Ring's with its own keys), checking Elden Ring's assets for a binder beside
them too.
`tests/draw_param_census.rs` (install-gated) prints every draw-param field the chosen
maps store, group, key and value. Core states fog and exposure (`core.atmosphere`:
`Atmosphere`, `Fog`, `FogFalloff`, `Exposure`, `AutoExposure`, `ExposureMeter`, `VolumetricFog`); `lower-fromsoft-environment`
keeps the `VolumetricFog ParamEditor` fields the census shows in `Atmosphere::volumetric`
with the file's own names and values, unconverted (the densities are in a unit the group's
`GameScale` sets and no source establishes, so the pass counts them as not drawn). It
fills no distance `Fog`: its units are not established. Elden Ring's `Tone Map[Yebis]` group lowers to
`Atmosphere::auto_exposure` with the source meter, limits and adaptation; Sekiro's tone-map group remains
not lowered because its filter has not been traced.

A character's or part's files are kept beyond the model and its clips:

- The FLVER's dummy polygons are `fromsoft.dummies` on the prototype (`flver::Dummy`: position, forward and upward
  directions, the reference id, the parent and attach bones, colour bytes, two flags and two unexplained words, as
  stored) and `core.sockets` on the skeleton: a socket for each dummy that places, on its attach bone (else its parent
  bone), at its position taken in the parent bone's space (the model's where the parent is -1). Where
  `use_upward_vector` is set and the directions form a valid frame, normalized forward (-Z) and upward (+Y) also
  set its rotation in the attached joint's space; otherwise rotation is identity. Stored direction lengths stay in
  `fromsoft.dummies`. One
  that names a bone the model lacks, follows none, or stands at no finite place is left out and counted
  (`CharacterReport::dummies_unplaced`). Which reference id is which attach point is the game's, not the file's.
- The `.tae` event files of an animation binder (`tae::Tae`, version `0x1000D`, Sekiro's and Elden Ring's) are read: each
  animation's record is put on the animation's entity as `fromsoft.clip_events` (the events with their type, their start
  and end in seconds, the word after the type and their parameter bytes, and the record's mini header), found by the
  animation's number (`a000_003000` is the record numbered 3000, `a050_003000` the one numbered 50003000:
  `tae::clip_animation_id`). The player's binder holds one event file per animation group (`a50.tae` in binder slot
  5,000,050), storing numbers within the group: each file is numbered by its slot (`Tae::read_member`), and an import
  (`ImportOther`) is followed to the animation it names among the event files of the record's own binder
  (`tae::resolve`); one naming an animation those files do not hold leaves the clip without events, refused by name in
  the report. The report counts the records no animation matches and the animations no record matches
  (`AnimationReport::tae_unmatched`, `clips_without_events`); `tests/character_census.rs` prints the first numbers of
  each beside the first names so the rule can be read against the install. The event types are the file's numbers;
  their names are the game's templates, which the file does not carry. An object's binder (`o252050.tae`) is read the
  same way with `object_animations`.
- The cloth companion files (`.clm2`, `clm2::Clm2`: for each mesh of the FLVER a list of `[i16, i16]` entries whose
  meaning SoulsFormatsNEXT does not know) that go with a form are `fromsoft.cloth` on its prototype, as stored.
- A model's Havok cloth tagfile (`<model>_c.hkx`, a Sekiro `_l` part's `<model>_c_l.hkx`; every cloth tagfile of the
  part and character binders is named so) is `havok.cloth` on its prototype (`primase_havok::cloth`): its cloth data
  read whole and the skeleton their transform sets are, each bone joined to the part skeleton's joint of the same name
  (`CharacterReport::havok_cloth_data`, `havok_cloth_bones`, `havok_cloth_bones_unjoined`); a tagfile that does not
  read is named unread with its reason. Its display buffers (the meshes whose vertices the cloth sets) are bound to
  the FLVER meshes: a display buffer is the mesh whose node bone has its name, with its vertex count and its first
  face set's triangle count, the buffer's vertex `i` the mesh's vertex `i` (every display buffer of both installs is so
  bound; `CharacterReport::havok_cloth_displays`, and a buffer bound to none is a notice by name).
  `lower-fromsoft-geometry` then says, per primitive of a bound mesh, which display buffer vertex each of its vertices
  is (`havok.cloth_vertices`), for a program that runs the cloth and draws what it sets. `tests/part_cloth.rs` checks
  a Sekiro and an Elden Ring part.
- A character whose display masks need the NPC param also reads each row's descriptive fields from the same file
  (`param::NpcStats`: `behaviorVariationId`, `nameId`, `turnVellocity` (sic, degrees a second), `hitHeight`, `hitRadius`,
  `weight`, `hitYOffset`, `hp`, `mp`, `getSoul`, at the offsets `Game::npc_stats` states from Paramdex's
  `NpcParam.xml`) as the resource `fromsoft.npc_stats`; the row's other several hundred fields are not read. A layout
  the offsets are not for leaves them unread, said in `CharacterReport::npc_stats_unread`.
- A part's collision file (the form's `<form>_collidable.hkx`) is read as an object's is (`objects::bodies`, one high file)
  and kept as `fromsoft.object_collision` on the prototype (`CharacterReport::collision_files`, `collision_bodies`);
  `lower-fromsoft-part-collision{convex=...,leave-out-motion=...}` lowers the bodies of a prototype no placement uses
  (as `lower-fromsoft-object-collision` does the placed ones) into `core.collision` on the prototype itself, in its own space.
  A file that does not read is named in `unread` with its reason.
- `https://github.com/Kalmarv/fromsoft-formats/blob/main/tests/havok_census.rs` (install-gated) prints, for twenty characters of each game, the classes of every Havok member of
  their chrbnd, anibnd, behbnd and texbnd binders (`HAVOK`, `CLASS` with the stored field names of the cloth, ragdoll,
  rigid body, constraint and physics classes, `HAVOKERR`) and every binder member by kind, extension and whether a
  reader keeps it (`EXT`). `havok_formats::cloth::read` (cloth data, ragdoll data, constraint infos) is written from those lines and the census prints what it reads of every such file (`PHYSICS`, or `PHYSICSERR` with the reason); the character reader does not keep its output yet.
- `tests/character_census.rs` also prints `DUMMYDIR`: whether the dummies' stored forward and upward directions are unit
  vectors and perpendicular where the flag says upward is used. A direction is lowered only once that and the axes agree.
- Not read: the Havok physics and ragdoll files of a character binder beyond what `unread` names, and what the TAE
  event types mean.

Passes (`sekiro::passes::register`), each choosing explicitly:

- `drop-fromsoft-far` leaves out the distant-backdrop placements and prototypes.
- `fromsoft-apply-rigid-skinning` poses `fromsoft.flver` into `fromsoft.rigid_model`
  beside it (below).
- `lower-fromsoft-geometry` makes `core.mesh`: one primitive per mesh and face set,
  level 0 on the prototype and levels 1 and 2 as `core.lods` forms for
  `select-lod`; positions, unit normals, stored tangents and UV0 in the world frame,
  a layered material's Byte4C members as raw `Color(n)` streams listed by
  `fromsoft.layer_inputs`. `pose=rigid|stored` is required (which model is lowered);
  its other options default to the source as stored: `uv-tiling=keep|apply` (recorded per material as
  `fromsoft.uv_tiling`), `winding=stored|normals`, `degenerates=keep|drop`,
  `leave-out=sky+light-volume+shadow+billboard`, `blocker-cards=draw|record`,
  `speedtree-frame=program|authored` (a SpeedTree mesh's normal and tangent from the
  members its material's G-buffer vertex program reads, or its layout's pair). Motion-blur
  face sets, vertex colours and UV sets beyond UV0 are not lowered and are counted.
- `lower-fromsoft-effects{candela-per-power=...}` makes `core.particles` from the placed `fromsoft.effect` records and the
  effect files and textures the read kept: each node of the effect's tree becomes an emitter (one per config its states
  choose, the node's children hanging from the first), with the emitter action as its spawn, the shape and direction
  actions as where and which way particles start, the movement actions as their law, and the appearance as the draw
  (billboards and multi-texture billboards, point sprites, lines, trails, mesh particles, distortion, radial blur,
  lights, force fields, lens flares, water ripples; a GPU particle system as an ordinary emitter and quad). The effects an
  effect spawns (`SFXReference`) are systems of their own that its emitters name; a cycle is cut where it closes. A
  Hermite property is sampled into a line (counted); the fxr project's reading of the parameters is the meaning used
  (sourced, not checked in a running game); what core has no place for (the flicker of a light, sounds, the conditions
  between states, a node's response to forces) is counted by reason. `candela-per-power` scales the light draws as
  `lower-fromsoft-lights` scales its lights. The records stay.
- `lower-fromsoft-lights{candela-per-power=...,spot-blend=...}` makes `core.light` from the BTL
  records: colour the stored byte over 255 squared (the game's own transfer, read back from
  a running game: the lighting shader's diffuse colour of every light is `(byte / 255)^2 *
  power`, not the sRGB curve), intensity the diffuse power times the program's calibration
  (no default), range the stored radius (at least `min-range`), a spot's cone from the
  stored full angles in degrees (the vertical axis's `cone_angle`, the horizontal axis's
  `cone_width`, record offset 184, each taking the other's where it is 0; the game's spot
  light scales each axis by `cot(angle / 2)`, read back from five live spot records, so the
  stored value is the full angle of the cone's edge) as the circle of the same area, with an
  inner cone at `1 - spot-blend` (no default: the game's falloff is `1 - (tan a / tan edge)^2`,
  with no inner cone). A BTL light's rotation is stored in radians; a spot shines along the
  rotated local +Z of the game frame (the transform's local -Z in the mirrored world frame).
  A spot with no angle in either axis is left off, as the game leaves it; the record's
  exposure weight (offset 224, a term of the game's per-frame exposure; 0 on every Elden
  Ring light) is not lowered, counted. The records stay. It also lowers the `fromsoft.effect_light` records
  (read with `Options::effect_lights`) in the same unit: the game uploads a BTL light and an
  effect light as the same record, so an effect light's colour is its stored red, green and
  blue squared (every component of every light of both installs is a whole number of 255ths,
  the representation of BTL's bytes) and its brightness, the colour's alpha times the stored
  diffuse multiplier at the settled value of those properties (`brightness=initial`: the
  first keyframe), is a BTL power and takes `candela-per-power` and no other factor. A spot's
  multiplier is used as stored (the game's code has no tenth; the fxr library's tenth is its
  own convention; read from the code, not yet checked on a live spot, counted); a point's range
  is its radius, a spot's its far plane and its cone the wider half-angle of its elliptic base
  (axis: the effect's local +Z, inferred; the game's code builds the frustum from `2 * atan(radius
  / 2 / far)` per axis, not yet checked on a live effect spot). The relation is in `tests/engine_light_records.rs`
  against the records a running Sekiro uploaded (5 BTL lights and 1 steady effect light, to float
  rounding). What an effect light does over time, the flicker, fade-out and specular, is not
  lowered. Lights of a region that starts disabled and lights of a config the effect's first
  state does not choose are left out, counted by reason (`include-disabled=on` lowers them); a
  light dark at the lowered value is left out, counted.
- `lower-fromsoft-sky` lowers the sky a Sekiro read brought in (`Options::sky: Include`, `read/sky.rs`): the sky
  map `m89_00_00_99`'s domes (one per sky state), their rings of far mountains and the moon (the object `o005900`),
  and the area's own domes no layout places. The read makes the states the values of one global axis,
  `fromsoft.sky`, with no default. `Options::world_state` can assign its value from supplied source flags;
  otherwise a program assigns one or a viewer switches. The lowering
  gives each dome and ring the condition of its state and `core.sky_anchor` horizontal (the game moves the sky map's
  parts to the camera's x and z every frame); the pass anchors the moon towards the environment's sun at 25 units
  (the game's), drops a moon with no sun (counted), and puts the sky's materials on the sky layer in the frame's
  order (dome, moon, rings). Off unless asked: a target's pipeline that leaves out `sky` gets none of it.
  Elden Ring's read with the sky brings in the assets of the area's sky map (`m60_00_00_99` for an open-world tile, in
  the tile's frame; `mAA_BB_00_99` for a legacy map) that its weathers show: the values of `fromsoft.sky` are the
  weathers and fire-ash states of the sky map's `WeatherAssetReplaceParam` rows that apply at the area's creation
  limit (`MapDefaultInfoParam`), `weather-<id>` and `weather-<id>-ashen`, with no default (the weather lot is not
  read), and each asset is conditioned on the values that show it. An asset drawn only with sky programs (the
  material table's sky class: `M[Sky9]`, `M[Clouds5]`, `M[Clouds9]`, `M[Atmosphere][Clouds]..`) stands relative to its
  sky map's origin with `core.sky_anchor` horizontal, as a recorded Stormveil frame draws its cloud layer at the
  camera's x and z; any other (the Erdtree's light shafts) is placed where the layout places it, as that frame draws
  one. The pass puts the sky-program materials on the sky layer at its start, writing depth, as that frame draws the
  cloud layer (after the lit opaque surfaces, before the blended ones; that frame composites its volumetric fog over
  the sky, where Sekiro's composites it before its sky: a render graph states either by its order), and leaves the other materials in their own layer, counted. The sky
  spheres' own draws are not yet observed (inferred from the program family). The sky map's parts no row names (its
  far view of the land) are not read, counted.
- `lower-fromsoft-environment{lux-per-unit=...}` lowers `fromsoft.draw_param` to
  the core environment: the light set's first directional light (`Directional
  Light Angle0`, `DiffColor0`) a directional `core.light` named as the sun by
  `core.environment`: as a Sekiro frame shows them reaching the deferred sun
  program, the angle `(rx, ry)` is the direction towards the light
  `(sin ry cos rx, -sin rx, cos ry cos rx)` and the colour `(r, g, b, w)` the
  radiance `(r^2 w, g^2 w, b^2 w)` (Elden Ring lowered the same way, not yet
  checked on its frame), at the program's calibration (`lux-per-unit`, lux per
  unit of radiance, no default); the hemisphere colours (`Hemi Color Up`,
  `Down`, decoded as `(rgb * rgb) * w`) the gradient ambient, its equator their mean (counted), at the same
  calibration. A black or unusable light is no sun, counted. Every other field
  read is counted by group and why it is not lowered. The sun's specular colour is retained as `core.light`
  highlights; Elden Ring's auto exposure and the source-unit volumetric fog are retained in `core.atmosphere`.
  Secondary and local light-set lights, occlusion and unsupported tone effects remain not lowered. The record stays.
- `lower-fromsoft-collision{hit-filters=...}` lowers the map collision files that
  parts of the chosen hit filters name (required) into `core.collision` soups
  (`game`, `water`, `game_state<n>` with the file's condition), each triangle with
  its HitMtrl class's surface in the read's game (`known_surface`; Sekiro's and
  Elden Ring's HitMtrlParam rows are numbered differently, the pass reads the game
  from `fromsoft.read_scope`; `default` without one, counted, and a class without
  a surface counted by itself with its row's name, `class_name`); `water=` and
  `leave-out=` take HitMtrl classes (an ID modulo 100), `reach=N` crops N metres
  around the read region (every option required: empty classes for none, `reach=none`). The records it lowered are consumed; files only other
  filters' parts name stay as source. `f-meshes=on` (optional; absent is `off`, the
  triangles counted as left out) lowers the `f` meshes a read with `f_collision` brought
  in into soups of their own (`f`, `f_state<n>`), never into `game`, with the `default`
  surface (counted as an approximation: they store no HitMtrl); `tests/f_collision.rs`
  checks them against the `game` soup of the same area.
- `lower-fromsoft-object-collision{resolution=high|low|high-else-low,convex=tessellate|rounded:<metres>|leave-out}` places every
  placed object prototype's bodies of the files of the chosen resolution (a prototype
  without one is counted; extern shape instances are convex bodies of their own) with each placement into an `objects` soup
  (`objects_state<n>` for conditioned placements), at their stored pose whatever
  their motion type (counted). A soup cannot hold a rounded shape, so `convex`
  is required: `tessellate` gives a convex body the intersection of its planes
  offset by its radius (sharp where Havok rounds, a capsule a box around its
  segment, a sphere a cube; both counted), `rounded:<metres>` its rounded surface
  (the core swept by its radius) within that error (inside the shape, counted),
  `leave-out` drops it (counted). Every option is required: `leave-out=`,
  `leave-out-motion=` (stored motion types), `reach=N` and
  `convex-surface=default|render` (render: the surface of the prototype's largest
  colliding render area, measured on its stored level-0 face sets) are the
  program's choices.

  In a part of a conversion (`core.part_scope`) both collision passes lower only
  what the part's roots reach: a collision file or placement loaded as context is
  its own part's (`tests/collision_parts.rs` converts two cells, each with the
  other as its halo, and gets exactly the whole world's soups).
- `lower-fromsoft-extras` makes each hang segment a two-point `core.curve` of kind
  `ledge` and each spawn point a `core.marker` of kind `travel`, in source order,
  named as stored. Joining ledges (`primase_meshops::chain`), the surface under
  them (`primase_meshops::SurfaceIndex`), choosing a spawn and naming markers are
  a program's.
- `lower-fromsoft-map-records{leave-out=...}` lowers `fromsoft.map_records` (read with `Options::map_records`, off by
  default): each region of a type but the spawn point a `core.marker` of the type's word at its stored place
  (`msb::euler_transform`, the parts' Euler convention, a stated approximation; the shape is not drawn) with the stored
  record on the entity (`fromsoft.region`: type code and name, shape and dimensions, entity ID, the type's data as
  hexadecimal bytes); each enemy, player start, dummy and connect collision a marker of its kind word with
  `fromsoft.unplaced_part`; each event a named entity with `fromsoft.event` and no place. The type words are
  SoulsFormatsNEXT's `RegionType`/`EventType` names for each generation (`Generation::region_type`, `event_type`); a
  code it does not name is `type_<n>`. `leave-out` takes those words (empty for none). Nothing in a type's data is
  decoded.

The collision and geometry the earlier reader produced by itself are this explicit
composition: `lower-fromsoft-collision{hit-filters=8+27,water=22+36,leave-out=29+40,reach=50},
lower-fromsoft-object-collision{resolution=high,convex=tessellate,leave-out=22+29+36+40,leave-out-motion=2,convex-surface=render,reach=50},
drop-fromsoft-far, fromsoft-apply-rigid-skinning,
lower-fromsoft-geometry{pose=rigid,uv-tiling=apply,winding=normals,degenerates=drop,leave-out=sky+light-volume+shadow+billboard,blocker-cards=record,speedtree-frame=program},
select-lod{form=finest}`. On m11_01 it reproduces that reader's placements, meshes
(streams, indices, material identities), collision, lights, joined ledges and
travel markers exactly. The hit classes it leaves out, 29 and 40, are Sekiro's: 29 is "None" (its row is
byte-identical to row 0) and 40 "Cloud". In Elden Ring class 29 is real collision (236,710 triangles in 1,539
files), so an Elden Ring composition does not take this `leave-out`.

`tests/sekiro_scene.rs` checks reads against a SoulsFormats dump of the area
(`oracles.soulsformats.sekiro/m11_01`). The whole-area read, before any pass, places
every dumped map piece and object part except those of the models the dump could
not read either (missing from the read) and exactly the parts of the models the
dump holds no vertex of (empty), and nothing else. Slice A's read accounts
for every part (kept, outside, missing, empty); every placement is an MSB part
with its model, translation exact and basis within 1e-5; after the geometry
composition every
lowered vertex is a vertex of its FLVER in model space within 0.2 mm; four
material fixtures worked out by hand from the dump keep their class, base and
normal texture. `fromsoft_collision_scene.rs` (retired) compared the map collision,
lowered with the composition's options, with the scene our earlier C# exporter wrote (`oracles.baseline.sekiro/scene`, a regression
baseline, read through the ReSkate interchange tables): all non-roof triangles and
the water table agree exactly. The light, ledge and marker passes are checked by
hand-worked cases (`tests/extras_policy.rs`, primase-meshops' curve tests); the
earlier exporter's extras and object scenes they were once compared with are retired.

## Map coverage

What a Sekiro or Elden Ring map stores, and whether primase keeps it. Written from the code (`read/area.rs`,
`read/collision.rs`, `read/extras.rs`, `msb.rs`, `passes/`) and the lab's views; counts are given only where a census
has printed them, and every other count is a line of `tests/map_coverage_census.rs` (install-gated: `FILES` the area's
archive paths by pattern, `MEMBERS` the members of its binders, `PARTS`, `REGIONS` and `EVENTS` the MSB's types, `WORLD`
what the read keeps) for three areas of each game. Status: **read** (a reader keeps it and a pass or the lab shows it),
**read-header** (header and shape read, the type's own data kept as bytes), **dropped** (read, then not in the world
unless an option asks), **unread** (no reader opens it). A row's owner is the primase reader, then the lab's display.

| Type | Count | Status | Owner (reader / display) |
|---|---|---|---|
| Map pieces (MSB part kind 0, `<mapbnd>` FLVER) | `PARTS map_piece` | read | `read/area.rs`, `lower-fromsoft-geometry` / Objects, viewport |
| Objects (Sekiro kind 1) and assets (Elden Ring kind 13) | Sekiro 1395 objbnds, Elden Ring 12657 geombnds (object census) | read; animations `object_animations` only, not lowered onto placements | `read/area.rs`, `read/animation.rs` / Objects; clips listed under Clips |
| Enemies, player starts, dummy objects/assets/enemies, connect collisions (parts) | `PARTS` kinds | **dropped** without `Options::map_records`; with it, read as markers (kind = the part's word) with `fromsoft.unplaced_part` | `msb::MapLayout::unplaced_parts`, `lower-fromsoft-map-records` / Markers, Properties. An enemy's NPC row is read; its think/patrol/talk data is not |
| Map collision, high resolution (`h<area>.hkxbhd`) | `FILES h<area>` | read | `read/collision.rs`, `lower-fromsoft-collision` / Collision |
| Map collision, `f` binder (`f<area>.hkxbhd`: one extern mesh body per stored body) | Sekiro 1,145 files in 9 areas, Elden Ring 1,850 in 603 (`fromsoft-formats` `tests/extern_mesh_census.rs`) | read with `Options::f_collision` (off by default) as `MapCollision::f_triangles`, beside the `h` file of the same model | `read/collision.rs`, `lower-fromsoft-collision{f-meshes=on}` / Collision (group `f`) |
| Map collision, low resolution (`l<area>.hkxbhd`) | `FILES l<area>` | unread (no option selects it) | none. Decided by the `FILES`/`MEMBERS` lines: the same Havok layout as the high file, if its members are `.hkx` |
| Object and asset collision | Elden Ring: `object_animation_census`/collision census | read (high; Elden Ring's low where no high) | `read/objects.rs`, `lower-fromsoft-object-collision` / Collision |
| Part collision (a character form's `_collidable.hkx`) | character census | read | `read/character.rs`, `lower-fromsoft-part-collision` / Collision |
| Navmesh (`n<area>…nvmhktbnd`) and other navigation binders | `FILES` patterns with `nvm`, `MEMBERS` | unread | none. Decided by `MEMBERS` (the extensions inside) and a tagfile read of one member |
| Decals | material slots of class `decal` (`Report::material_classes`) | read as materials (`MeshDecal`), drawn on the decal layer, queue order a convention (approximation, noticed). Decal *placements* beyond the material slots: none known in the MSB | `lower-fromsoft-materials` / Shaded view |
| Water | slots of class `water`; collision class soup `water` | read as a material (the program's water colour and normal map; depth, refraction and flow not evaluated) and as a collision soup | `lower-fromsoft-materials`, `lower-fromsoft-collision` / Shaded, Collision |
| Lights, bank 0 (`<area>_0000.btl.dcx`) | `FILES _NNNN.btl` | read | `read/extras.rs`, `lower-fromsoft-lights` / Lights |
| Effects (every FXR action: emitters, particles, trails, distortion, blur, lights, forces): the same routes | `REGIONS` type 5, `PARTS`, `ModelSfxParam`/`AssetModelSfxParam`, the `sfx` binders | read with `Options::effects` (off by default) as `fromsoft.effect`, `fromsoft.effect_file`, `fromsoft.effect_resource`; models and animations named are counted, not read | `read/effects.rs`, `lower-fromsoft-effects` / Effects |
| Effect lights (FXR point and spot lights): SFX regions, and the effects placed objects and assets spawn through their model effect param row | `REGIONS` type 5, `PARTS`, `ModelSfxParam`/`AssetModelSfxParam`, the `sfx` binders | read with `Options::effect_lights` (off by default) as `fromsoft.effect_light`; light shafts (no instance) and every other effect action unread | `read/effect_lights.rs`, `lower-fromsoft-lights` / Lights. Per-area counts in `tests/effect_lights_census.rs` |
| Light banks other than 0 | `FILES _NNNN.btl` | unread (the reader opens `_0000` only) | none. `FILES` says how many banks exist per area |
| Light probes / global illumination data | Sekiro's `<area>_envmap_NN.tpf.dcx` GIIV packages and environment-map-effect-box regions | read with `Options::irradiance`, off by default; every package remains a source state, selected by `Options::world_state` when supplied | `sekiro::irradiance`, `lower-fromsoft-irradiance` / Irradiance volumes |
| Environment-map points and boxes (regions `environment_map_point`, `environment_map_effect_box`, `environment_map_output`) | `REGIONS` | read-header | `lower-fromsoft-map-records` / Markers. The part's `env_map` id and the draw-param `Env Map:Editor` group are read (`msb::PartGparam`) |
| Environment cubemap textures | `FILES` patterns of texture binders | unread unless a material binds them | none |
| Sound and effect regions (`sound`, `sfx`, `wind_sfx`, `wind_area`, `muffling_*`, `sound_region`, `sound_space_override`) | `REGIONS` | read-header: shape, place, entity ID read; sound id and effect id are in the type data, kept as bytes | `lower-fromsoft-map-records` / Markers. The `data_lengths` of a type decide whether a fixed record can be decoded |
| Weather and map-name regions (Elden Ring `weather_override`, `weather_create_asset_point`, `map_name_override`, `play_area`, `hitset`, …) | `REGIONS` | read-header | same |
| Spawn points (region 8) | `REGIONS spawn_point` | read | `read/extras.rs`, `lower-fromsoft-extras` (`travel` markers; the facing convention is not established) / Markers |
| Other regions (invasion points, messages, patrol routes, warp points, connections, auto-draw groups, …) | `REGIONS` | read-header | `lower-fromsoft-map-records` / Markers |
| Hang edges (`<area>.edgebhd`) | `FILES edgebhd` | read (hang edges; grapple and hug kinds counted, not read) | `read/extras.rs`, `lower-fromsoft-extras` / Curves |
| SpeedTree trees | assets `aeg801_*` | read as assets with their LOD levels; billboard cards left out (`leave-out=billboard`: the program places them from the camera, core has no camera-facing primitive) | `read/area.rs`, `lower-fromsoft-geometry` / Objects |
| Grass | `Report::grass_parts`, `Report::grass` | grass config ids and `GrassTypeParam` rows read (Elden Ring); no blade is placed (the game scatters by density) | `read/area.rs`, `param::GrassParam` / notices only |
| Far view (Sekiro `m9xxxxx` backdrops) | `Report::far_prototypes`, `far_placements` | read, then left out by `drop-fromsoft-far` unless the program keeps them | `read/area.rs`, `drop-fromsoft-far` / Objects |
| Far view (Elden Ring LOD tiles) | `Parents::Include` parts, `FILES` | the larger tiles' parts for the small tile are read (`Options::parents`); distant-terrain files beyond them: unread | `read/area.rs` / Objects. `FILES` lists the unread patterns |
| Events (`EVENT_PARAM_ST`) of every type (treasure, generator, obj_act, navmesh, patrol_info, …) | `EVENTS` | read-header: type, event ID, the part and region named, entity ID read; the type's data kept as bytes. `map_offset` (Sekiro) is decoded and used | `msb::MapLayout::map_events`, `lower-fromsoft-map-records` / Other world data rows, Properties (no place of their own) |
| Event scripts (`.emevd`) | one per area | Sekiro: read and interpreted for part presence (`states=event`); Elden Ring: unread (refused) | `emevd.rs`, `read/states.rs` / State axes |
| Sky (Sekiro's sky map `m89_00_00_99`, the moon `o005900`, the areas' own unplaced domes) | its layout | read with `Options::sky` (off by default): the states as the `fromsoft.sky` axis; `Options::world_state` selects from common event 100 with caller-supplied flags | `read/sky.rs`, `lower-fromsoft-sky` / Objects, State axes |
| Sky (Elden Ring's sky maps' weather assets: sky spheres, cloud layers, the burning Erdtree) | `WeatherAssetReplaceParam`, `MapDefaultInfoParam`, the sky map's layout | read with `Options::sky`: the weathers as the `fromsoft.sky` axis without a default, each asset conditioned on the weathers that show it; sky-program assets anchored to the camera's x and z and drawn on the sky layer writing depth (observed for a cloud layer, inferred for the spheres), others placed where the layout places them; the weather lot is unread | `read/sky.rs`, `lower-fromsoft-sky` / State axes, Draw order |
| Draw-param environment | `draw_param_census` | read for the file the caller names; the file choice from the weather and events, and Elden Ring's `_weatherbase`/`_weatheroutdoor`/`_commonevent` layers: unread | `read/draw_param.rs`, `lower-fromsoft-environment` / Environment, Properties |
| Textures (`.tpf`, Sekiro's `.tpfbhd`), shader bundles, materials | per area | read | `read/textures.rs`, `read/shaders.rs` / Shaded and Game shader views |

Open rows, in the order the census can close them: the `l<area>` low-resolution collision, the navmesh binders, light
banks other than 0, the light-probe pattern (if `FILES` names one), the decoding of the sound/effect/environment-map
regions' type data (`data_lengths`), the Elden Ring far tiles, the Elden Ring event scripts, and object clips lowered
onto placements.

## Shaded vs Game drift

What the Shaded path takes from a lowered material (`lower-fromsoft-materials`, `pbr.rs`, then `bake-fromsoft-materials` and
`bake-fromsoft-layers` where the lab runs them) against what the G-buffer program's outputs compute from the same
parameters and textures (`Game::gbuffer`, read as a `Surface` by `core/src/shader/surface.rs`). Written from the code;
nothing here was run. **Kind**: *missing* (the program has the term, the lowering none), *arithmetic* (both have it,
the formula differs), *model* (the two do not share a lighting model), *agree* (same formula, a test holds it), *noticed*
(an approximation notice names it). The lab's Shaded view runs the program bake first (`surface_bakes`), so the rows
"unbaked" below are what Shaded shows where the bake refuses a material (its refusal reason is the notice
`program bake refused; lowered material retained`) and what every non-lab consumer of the lowering gets.

| Family | Output | Shaded's source | The program's source | Kind | Status |
|---|---|---|---|---|---|
| Sekiro simple (`MetallicTex`, `MetallicValue`, `MetallicValue_MeshDecal`), unbaked | base colour | the preview's albedo slot (slot 8, else the lowest), sRGB | target 2 `rgb` (the program's blend of its slots and colour constants) | missing: material colour constants and the slots blended over it are not applied | **closed by the bake, which these programs never reached**: their one-set `TEXCOORD0` (two shorts) had no feed in the bake's vertex stage, so every one was refused "input TEXCOORD0 has no value to run on: Stream(TexCoord(0))"; `Feed::TexCoord0` (test `a_one_set_vertex_program_is_fed_the_grid_in_its_two_shorts`). The lowering itself still does not apply the constants: the program's arithmetic is run, not re-derived |
| same | normal | normal map as stored, `normals::SOURCE` (basis observed for the simple families by `consumer_basis`) | target 0 `xyz`, biased world normal (0.50048876) | agree on the basis; normal-intensity parameters are read by neither lowering path | agree; intensity unverified |
| same | roughness | `1 - 0.95 sqrt(gloss)`, gloss = the normal map's blue (`pbr::roughness`) | `Gloss(square_of(0.95))` of target 1 `x`, the same value | agree (`gloss_matches_deferred_ggx_alpha`, `surface.rs` tests); the per-light factor `k` is omitted | noticed |
| same | metallic / F0 | metal map R (or GXMD 237 for `MetallicValue*`), else 0 counted | no metallic output; target 3 `rgb` is F0 (specular workflow), stored as its square root | model: metallic workflow against specular workflow; F0 0.03 represented by glTF 0.04 (the 0.03 is traced for `M[Multiple]` only) | open, noticed |
| same | emission | none (`source emission without proven radiance`) | target 4 `rgb` | missing | closed by the same bake (`Emission` role decoded into the emissive texture); open for the unbaked lowering alone |
| same | occlusion | none | no output has an occlusion role in either game | agree | closed |
| same | tiling / UV | **was**: the FLVER's scale about the origin, the base map untiled when `uv-tiling=keep` | the `FC_TextureTilingScale` row: the FLVER's scale, else the MTD's stored one, else the uv group's, each axis about the texture's centre (`layer_bake::resolve::slot_tiling`) | arithmetic (offset `(1 - s) / 2`; stored and uv-group scales absent; base map) | **fixed**: `pbr.rs` takes every map's transform from the row; tests `maps_are_tiled_about_their_centre_by_the_flver_scale_else_the_stored_one`, `selected_normal_identity_controls_both_transforms`. The rule reproduces 301 of 440 captured Sekiro contents (its own doc), so a residual stays |
| same | alpha cutout | `Mask` at the bound program's `AlphaReference` (`alpha_cut`); a hard cutoff for dithered and transformed tests | the program's discard | agree; dithered and transformed counted (`DITHERED_ALPHA_CUT`, `TRANSFORMED_ALPHA_CUT`) | noticed |
| same | alpha blend | `Blend` for classes `alpha`, `additive`, `decal`, `water`; additive also as emission | the render state's blend: `g_BlendMode` 2 stated, 4 and the others unstated (`blend_unstated`) | model for additive and the unstated modes | open |
| Sekiro layered (`M[Multiple]`, `M[MultipleGround]`, `_Emissive`), baked | base, normal, gloss, emission | the program evaluated over UV0 (`bake-fromsoft-layers`) with the same decodings | same | agree; static flat tangent frame, white vertex colour, no per-light gloss factor, UNORM8 | noticed |
| same | F0 | **was**: metallic 0 with the notice "F0 0.03 mapped to 0.04" | G-buffer 3 `0x3e315cac` squared (0.03), traced per program | arithmetic | **fixed** for traced programs: `specularFactor` = F0 / 0.04 (`Reflectance::specular`; test `a_traced_dielectric_states_the_programs_f0_as_the_specular_factor`). Untraced programs (`M[Multiple]` switch 18, switches outside 0 to 33, a non-uniform output) keep metallic 0, counted |
| Sekiro other MTD shaders (`GXFlver_*`, `M[Translucency]`, `M[AMSN]`, character, fur, …), unbaked | roughness, metallic | roughness 1 (the default), metallic 0 (`unclassified shader PBR`, now naming the default) | target 1 `x` gloss, target 3 F0 | missing | open; noticed |
| Elden Ring material binaries, unbaked | base colour | the slot the program makes the albedo of (`coverage::Reader::base`); the slots blended over it left out; the lowest layer when no one is the base | target 2 `rgb` | missing (noticed per program) | open; the bake closes it |
| same | roughness, metallic | roughness 1, metallic 0 (every Elden shader is `unclassified`) | target 1 `x` metallic, `y` gloss (`Game::gbuffer`) | missing | open; noticed |
| same | normal | normal map as stored, basis assumed (`consumer_basis` refuses every Elden shader name: counted) | target 0 `xyz` | arithmetic unverified | open; noticed |
| same | tiling / UV | **was**: `Binding.scale`, which is the FLVER's and is 1 for every Elden map material (`sat_field_census`), so the sampler's stored scale (153 captured rows) and the uv group's (425) were dropped | the same row as Sekiro's (rule 84c6e0f1: FLVER, stored, uv group, centred) | arithmetic | **fixed** with the Sekiro row: the lowering reads the row through the shader bundle's metaparam |
| same | alpha cutout | the slot whose alpha reaches the discard, on the UV set the program samples it with; the vertex alpha where read | same | agree; the other terms of the discard are left out, one notice per term | noticed |
| same | emission, occlusion | none | targets 3 and 4 are read by no lighting program: no role | agree | closed |
| forward materials (`g_BlendMode` 2, or `g_GX_ForwardRendering` stated true: the skies, far mountains, clouds), both games | all | the lowered material (texture alpha) | the forward program's finished colour (role `Colour`), engine textures bound at zero | model: the bake needs an albedo output and refuses these | open |
| both games, baked by `bake-fromsoft-materials` | alpha | the previous base texture's alpha at UV0 times the base alpha | the program's discard and blend | arithmetic for a program whose alpha is not that texture's | noticed (`program still discards texels with alpha test disabled` refuses) |
| both games | vertex colour | Elden Ring material binaries: ignored unless the alpha test reads it; Sekiro: kept only for the layered programs that read it | read by the programs that use it (blend weights, occlusion) | missing for the others | **stated**: a pixel program with a `COLOR` varying is named per material (`pixel program reads a vertex colour ...`); the bake feeds a program's `COLOR0` white and counts `COLOR0 frozen to white` (it was refused). Core has only `VertexColor::Ignored`; there is no field for "multiplies the albedo by it", and the arithmetic of which output a colour reaches is not read |

Why `bake-fromsoft-materials` refuses a material, from the recorded run reports (`scratch/branch-triage/measure/*/report.json`
and `scratch/primase-reset/copy-cache-evidence/replies.json`: character parts and areas mixed, so the order, not the
counts, is what to take), most frequent first, and what each needs:

| Refusal | Cause | Status |
|---|---|---|
| `input COLOR0 has no value to run on ... no stream of the lowered geometry carries it` | `resolve_vertex` bound `COLOR0` to nothing | **fixed**: bound as `Color(0)`, fed white, counted (`COLOUR0_POLICY`) |
| `input TEXCOORD0 has no value to run on: Stream(TexCoord(0))` | one-set programs (Sekiro's non-layered SAT shaders) had no feed | **fixed**: `Feed::TexCoord0` |
| `input TANGENT4` / `TANGENT2` | tangent set number is the layout member's own semantic index: indices 0,1,4 remain Tangent(0), Tangent(1), Tangent(4), even if the program reads only 0 and 4 | every decoded member keeps its index through compaction; a missing/incomplete primitive set remains Unknown with its semantic index and reason |
| `the program reads COLOR1 lanes beyond x` | only `COLOR1.x` is observed (`COLOUR1_POLICY`) | open: needs a capture of `y`..`w` |
| `input TEXCOORD1 has lanes the bake cannot feed` (was `... a further coordinate set`) | a further packed set is bound as the next UV sets (`C[Fur]`'s `TEXCOORD1` is `TexCoord(2)`, which an Elden Ring mesh keeps); the bake feeds sets 0 and 1 only | open: the bake's feed of further sets |
| `no albedo output role` / `scene-dependent Colour` | forward programs write a finished colour | open (model, see forward row) |
| `program still discards texels with alpha test disabled` | a discard besides the alpha test (dither, clip) | open: `bind_output_uses` shows which lanes decide it |
| `frame texture SceneDepth cannot be baked as a material` | the program reads the scene's depth | open: not a material term |
| `alpha uses a secondary UV set` | the bake samples the previous base texture's alpha on UV0 only | open |
| `vertex program ...: t1/t7: no queries given` | a vertex program that samples a texture | open |

The two fixed rows do not make every Sekiro simple or Elden Ring material bake: a material still stops at the next row it
meets. Whether Elden Ring's materials now bake is for a rerun to say (`program bake refused` in its notices).

Not done: the tiling rule's 130 unreproduced Sekiro contents. The capture rows (`cbTextureTilingScale`, 483 contents of 58
programs) the rule's doc cites are not on disk under the names the task gave (`cache/sekiro/cb11-capture` holds the
texture-scroll buffer, one content, identity), and Elden Ring's `tiling-offline/RECORD.md` is undecided for the Elden branch
and says so. Classifying the 130 by what differs needs the capture file tied to the install by a census run.

Open rows after this round: the F0 of the simple shaders and the untraced layered programs; roughness and metallic of the
unclassified and Elden Ring shaders where the bake still refuses (the table above); Elden Ring's normal basis; additive and
unstated blend modes; forward materials; vertex colours' arithmetic; the tiling rule's residual; and the bake's static frame.

## Consumer normal-map frame

Representative G-buffer pixel shaders for 20 available scene shader families
read R/G as `2*v-1`, reconstruct positive `sqrt(1-min(dot(xy,xy),1))`, and
use red along `normalize(cross(N,T)*w)` and green along `normalize(T)`.
Blue carries other material data, not normal Z. The reader preserves texture
bytes and declares `BitangentTangent`, green positive along the stored tangent,
and Z to rebuild. Stored tangent xyz/w survives compaction. The LH-to-RH frame
conversion reflects xyz and negates w exactly once, in the geometry reader.

For writers that honor supplied tangents (glTF), run `normalize-normals`: it
swaps R/G and rebuilds Z against the stored frame, preserving alpha and all mips,
and does not flip w again. Curl/integrability verdicts describe authoring
patterns; they never select a per-texture sign exception.

For a target that derives its frame from texture coordinates (ReSkate's Studio
ignores supplied tangents), run `normalize-normals` then
`uv-frame-normals{bitangent=-v}` (toolkit/image). That pass measures each
triangle's stored frame against the frame its coordinates give, picks per
material the quarter turn or mirror most of its area takes, re-expresses the
normal map's red and green bytes exactly (a copy where materials sharing a map
take different ones), counts the area that takes another map or none (sheared
regions) and gives the meshes the derived tangents. For Sekiro's most common
stored frame (red along +u, green along +v) it keeps red and flips green, the
earlier skate rule; GTA's stored frames are its derived frames (worlds/gta
`tests/uv_frames.rs`), so the pass leaves its normal maps as they are.

`sekiro::normals::consumer_basis` lists the inspected families. Three legacy
families (`GXFlver_Col`, `GXFlver_ColDif`, `GXFlver_ColDifSpcBumpIbl`) are absent
from the inspected shaderbdle bundle (not proven absent from the install) and
use the same mapping as a named assumption, counted by `lower-fromsoft-materials`
with unclassified families. Water's static texture uses this basis, but its runtime
normal buffer remains an unsupported dynamic effect. Layer blending also remains
part of the later shader bake; this conversion only preserves the chosen static
texture contribution. Downstream consumers must preserve the emitted tangent
stream rather than regenerate a different UV-derived frame.

`GXFlver_ColDifSpcBumpEmiIbl` is present in the separate GX shader bundle. Its
G-buffer program samples t3 RG as `2*v-1`, multiplies red by the normalized
bitangent and green by the normalized tangent, rebuilds positive Z, and
normalizes the result. Sampled blue goes separately to o1.x, not to normal Z.
It uses the evidenced source basis and shared Z reconstruction.

`oracles.scripts.sekiro/normal-curl.json` supplies a JSON array of independently measured `name` and `verdict` values; the
gate reports all classifications and names every OpenGL verdict as a measurement, not a conversion exception. The
sampled source frames this section used to be checked against came from our earlier exporter's dump and are gone; the
basis is checked by the hand-worked probes above.

## Collision reader (in progress)

Raw map collision uses `havok-formats` for tagfiles, compendia and compressed
meshes. The FromSoft layer maps left-aligned shape keys through fsnp material
records. All-ones invalid keys are counted; missing materials remain -1 and are
counted separately. Contradictory bindings for a valid key fail.

Map collision decodes compressed-mesh bodies; a body of another shape class (m11_02
holds `hknpConvexShape` point hulls in files of other hit filters) is listed on
its file and reported, not read, and `lower-fromsoft-collision` refuses a chosen
file that has one. The area read keeps every named file whole; map offsets apply once, and the core
frame reflects the source's Z, so a stored triangle's order faces away (lowering
reverses it). Which hit filters are ground (the earlier exporter's: 8, world, and
27), which HitMtrl classes are water (22, 36) or left out (29, invisible
blockers; 40) and the crop are `lower-fromsoft-collision`'s options. HitMtrl IDs
map by their base value modulo 100 to core surface names where the class's surface
is known.

Elden Ring's collision is Havok 2018.1 (`havok-formats`'s `SDK_VERSIONS`). Its stored compressed meshes are
checked against HKLib ac40bc9 (`havok.jsonl` of `oracles.soulsformats.elden/format`, `https://github.com/Kalmarv/fromsoft-formats/blob/main/tests/havok_census.rs`): one record
per file, `file` (`<binder path>:<member>`), then `error` or `shapes`, each `kind`, `bits_per_key`, `sections`
(`codec` as six f32 bit patterns, `first_packed_vertex`, `packed_vertices`, `first_shared_vertex`,
`first_primitive`, `primitives`), `packed_vertices`, `shared_vertices`, `shared_vertices_index`, `primitives` (four
indices each), `triangle_to_shape_key` (FromSoft's custom-param shape, else null) and `extern_shapes` (each instance's
`transform`, its sixteen stored floats in order, `scale`, the instanced shape's `kind`, `convex_radius` and stored
`vertices`, as bit patterns). A file's shapes are compared as a set; the triangle decode built on them is not part of
this check, and the extern instances' transforms as the reader takes them are checked against the stored floats.

Object bodies (`collision::read_bodies`) are compressed meshes and the convex classes (box, polytope, capsule,
cylinder, plain convex, triangle, sphere). A compressed mesh's primitives whose second and fourth indices are equal are
custom primitives, not triangles: in every compressed mesh of both games' object and map collision, no triangle or
quad has them equal. Their first index names their data in the shared-vertex index. Data `[146, w, low, high]`
(indices `[k, k, 4, k]`) references the mesh's extern shape instance `low | high << 16`: a convex shape placed by the
instance's own transform, read as one more body of the same name and motion type (`BodyGeometry::Convex::instance`),
posed by that transform and then the body's; each instance is referenced exactly once (Elden Ring's assets: 262
instances in 22 files, every one equal to HKLib's reading). Data whose first word ends in byte 0x52 (`[0x2252, 8,
0x3d4d]`, in Sekiro's objects and one Elden Ring asset) is of a kind whose meaning is not known: counted, not read.
A mesh made of extern shapes only is those shapes. A shape key table of zero bits is accepted on a mesh of no
triangles (its rows identify nothing). `tests/asset_collision.rs` reads and tessellates every asset collision file
of the install (15,942).

The reader preserves raw authored roof collision. Roof smoothing, generated
opened-up surfaces, and object collision are not included in this step.
The whole-install collision census includes map h/l/f binders and all HKX members
of object binders, counting nonphysics animation/cloth variants separately.
Named character/ragdoll binders are counted but outside this reader's scope.
Container parsing does not imply geometry support for every observed shape.

## Rigid FLVER lowering

`fromsoft-apply-rigid-skinning` requires `fromsoft.flver` and provides
`fromsoft.rigid_model` beside it; the raw model stays. `lower-fromsoft-geometry`
lowers the posed model (or, with `pose=stored`, the raw one) and removes the posed
copy it consumed.

Static meshes use the implicit unit-weight binding in NORMAL.w. The pass applies
the bone hierarchy's scale, X/Z/Y rotation and translation to positions, the
inverse transpose to normals, and the linear transform to tangents. Reflections
reverse tangent handedness and face winding. Invalid hierarchies and singular
transforms are errors. Requested non-unit or multiple rigid influences are
refused and counted; no partial world mutation occurs on failure.

Dynamic meshes already store model-space bind-pose geometry and are animated at
runtime. They remain unchanged, counted separately, and are not submitted to
rigid lowering. This is a static bind-pose output, not an animation evaluator.
The rigid gate covers every model the area's MSB places (m11_01: 790 models,
9,284,997 stored vertices) against a SoulsFormats dump of the same FLVERs in
model space; positions agree within 0.2 mm and inverse-transpose normal
components within 2e-5 scaled by max(1, magnitude) (measured: 1.9 um and
3.5e-7). Placed collision is compared bidirectionally within 0.2 mm with
surface IDs enforced, independent of convex triangulation.

### Interior blocker cards

`lower-fromsoft-geometry{blocker-cards=record,speedtree-frame=program}` leaves out of the drawn geometry only meshes whose non-skipped resolved material uses a
`GXFlver_Col*` shader and an exact `m[0-9][0-9]_black_a` main albedo. Other meshes
in the same prototype remain renderable. Ordinary unlit ground fills and missing
albedos do not match. This is an explicit export policy: the source game draws
these black interior blockers; a program that omits them records them instead.
Their level-0 geometry, material, original FLVER mesh index and placed transforms
are kept in the `fromsoft.blocker_cards` resource. They never become collision geometry.

Blocker cards have no independent reference: the earlier cases came from our earlier exporter, and testing
against our own old code is not a check. An open coverage gap (build one when a step 20 or in-game Sekiro difference points at this area); the unit tests cover the rule.

### Bounded layered-material pilot

`layer_bake` binds the reflected SM5 resources for the Multiple material families.
It is a static preview policy: identity world transform, upward normal, stored
+X tangent, overhead camera, white instance colour, constant layer weights and
one UV set. It does not claim those constants or its base-mip bilinear wrap
sampler reproduce runtime derivatives or sampler descriptors. The sampler filters
as Direct3D 11 does, the texel coordinate truncated to 8 sub-texel bits
(`Subtexel::D3d11`; D3D11.3 functional specification 7.18.16.1), as the GPU and
WARP do. Albedo RGB is linearized before filtering; alpha and data channels
remain linear. Missing
bindings, zero-initialized buffer words, and frozen policies are counted.
Normal output inverts the binary G-buffer bias and retains the source tangent
basis; consumer lowering remains separate. The main albedo supplies alpha.

A bake first builds its `PipelineState` (`Prepared::pipeline_state`) and then
evaluates that state alone. The state holds:
- the grid, as size and period;
- the pixel program's container, constant-buffer words and texture slots (each
  a base mip's stored RGBA8 with its addressing and sRGB flag, or a constant);
- each pixel input's lanes (bits, or the grid's U or V);
- for a material with a vertex program, that program, its words, and the input
  and output registers of its four runs at the cell's corners;
- the albedo-alpha image with its scale.

Another executor given the same state can therefore check the execution: the
vertex runs, the pixel program over the grid, the interpolation and the
sampling. The values in the state stay this bake's stated policy.

`oracles.baseline.sekiro/layer-pilot` holds bake states and what Microsoft's
WARP rasterizer makes of them (`tests/layer_pilot.rs`):
- `cases.json`, written by the example `layer_pilot_states`: the first bake
  group of each kind in an area (shader family, with or without a vertex
  program, alpha-tested or not). Each case is written at two size caps and in
  three layer-weight variants: the defaults; mixed weights and a tint; every
  weight one with detail zero.
- Per case, cap and variant, a folder with the state as `PipelineState::encode`
  writes it. Beside it, `warp/` holds what the private WARP host's quad mode
  made of it. That mode runs the vertex program on the corners through stream
  output, and draws the pixel program over a quad fed with the state's input
  lanes. Textures are the state's own texels, sRGB views for the colour
  layers, with base-mip bilinear samplers. It writes:
  - `warp.json`: each stage's status;
  - `target<r>.f32`: each target's lanes;
  - `alpha.f32`;
  - `vertex-outputs.json`.

The test bakes our executor's outputs and WARP's through the same rules
(`PipelineState::bake`) and compares them:
- albedo and normal bytes within one level;
- the second output's x and the emission within one level of 1/255 or a
  thousandth;
- the albedo alpha within one level;
- the vertex outputs within four units in the last place.

The state's values are this bake's policy, so the gate checks execution,
interpolation and sampling, not runtime fidelity. The texels are D3D-convention
decodes (gated exactly against bcdec by the TPF pixel gate), so a native-BC
control would only repeat that gate.

Area grouping is an explicit static approximation: triangle-mean layer inputs,
snow weighted by the upward-facing normal, triangle area times placement count,
at most three weighted clusters, and weights quantized to 1/64. Groups below
two percent or within 0.08 on every component merge. Numeric lexicographic
ordering makes equal-weight groups deterministic without a text-format dependency.
Splitting preserves every vertex stream through the shared primitive compactor.
The bounded common-period policy searches up to sixteen UV units and records
when it falls back to the main texture's repeat. These policies are not runtime
shader behavior or format requirements.

The layer bake's images are checked against WARP running the game's layer shaders on the same states
(`oracles.baseline.sekiro/layer-pilot`, above). The area grouping and UV periods are policy and have no independent
reference: the earlier inventory and its control came from our earlier exporter's layer bake.

The opt-in `layer_area_pixels` tests additionally require
`<scope>-d3d-control/<record-index>-{albedo,normal}.rgba`, with indices in the
`counts.json` record order and RGBA8 at each record's `size`. The independent
producer receives D3D-decoded source pixels, as in the pilot. Four workers
prepare one material each; output sizes follow the largest sampled colour
layer, capped at 512. The gate reports unit and non-unit UV periods separately,
per-channel maximum and worst-image p99, and worker/wall time. No area output
is accepted merely because the fixtures exist: group identities, sizes and
periods must match before each image comparison.


### Material passes: layer bake and PBR lowering

The reader keeps material data: each FLVER material's `fromsoft.material`, every
texture a selected material binds (`fromsoft.texture_payloads`), the shader
bundles of its materials (`fromsoft.shader_bundle`) and the G-buffer programs of
its layered materials (`fromsoft.shader_programs`);
`lower-fromsoft-geometry` gives a layered part's raw FLVER Byte4C UV members as
`Color(n)` UNORM8 streams listed by `fromsoft.layer_inputs`. Two passes in
`sekiro::passes` lower them:

- `bake-fromsoft-layers{max-size=N,sample-time=S}` (`BakeFromSoftLayers`) bakes
  the static layered materials of placed prototypes with base-mip bilinear-wrap
  sampling at each material's reference size, capped at `max-size` (required: the
  program's choice; also the size of a group without a colour image). Five of the
  22 layered G-buffer programs of the inspected corpus read the global time
  (`FC_GlobalTime`, a texture-coordinate scroll of some layers): they are sampled
  at `sample-time` seconds (required), counted; the time of day stays zero. It groups per-triangle inputs into the gated
  weight levels and splits indexed parts by level through
  `World::rebuild_primitives`, preserving the other vertex streams and source
  material definitions and dividing main-layer UVs by the tiling the geometry
  pass applied (`fromsoft.uv_tiling`) and the bake period. A single weight level
  preserves the source vertex order. The source resources are put back also when
  it fails. Only the layer inputs of the primitives it baked are consumed, after
  it succeeded; an unplaced prototype's stay, and a level-of-detail form carrying
  inputs is refused before anything changes (`select-lod` first).
- `derive-fromsoft-detail` (`DeriveFromSoftDetail`), after
  `resolve-fromsoft-shaders`, gives each material of Sekiro's
  `Character_AMSN_[DetailBlend]` and of Elden Ring's plain and `[Rich]`
  `C[DetailBlend]` families (`[S2]` and `_SSS` are refused by name) whose
  program's equation proves on the
  material's own words a `pbr.detail` (`primase_core::detail::DetailedMaterial`)
  beside its baked `pbr.material`: the detail blend's chain of one to three
  tiled details, each slot the bound texture at the tiling the program samples
  it with (relative to a tiling `lower-fromsoft-geometry{uv-tiling=apply}`
  multiplied the coordinates by), and what the equation holds constant (the
  facing, the engine constants and textures that move a role). The equation is
  solved from runs of the program and proven on random samples and where its
  weights mix (`detail_derive::derive_equation`); a material that does not prove
  gets none, counted by reason, as is one no primitive draws. A slot whose
  addressing the source does not state repeats, counted by whether the
  material's mesh coordinates, through the slot's tiling, stay within 0 to 1
  (where the addressing decides only edge filtering) or reach past it (with the
  share past it, where clamping would hold the texture constant).
- `lower-fromsoft-materials{water-colour=R,G,B,A,layers=preserve|preview}`
  (`LowerFromSoftMaterials`) lowers every source material to core PBR, texture
  transforms relative to the applied tiling. A material without a base texture
  is drawn mid grey, or for water the required `water-colour` (sRGB bytes and a
  linear alpha byte: the destination's look, which the program states), counted.
  A layered material no pass baked needs the program's choice:
  `preserve` keeps it as source data (layer inputs included), counted; `preview`
  lowers it from its main-layer preview, counted, and drops the layer inputs of
  the primitives drawing a material it previewed (shader inputs, not vertex
  colours); a material it did not lower (a skipped class) keeps its inputs; no
  choice is an error. Normal maps of
  shaders whose basis is assumed or unclassified are counted. What a lowered
  material's source class states about its surfaces is set with it, counted:
  `water` (tessellated water shaders) as `core.material_usage` water, `leaves`
  (`M[Translucency]`) as foliage, `decal` (`MeshDecal`) as `core.draw_order`'s
  decal layer (queue 2500, after the cutouts: a convention, as Sekiro's own pass
  sequence is not read). The other classes state only their alpha mode.
  Sidedness is the source's, per face set: `lower-fromsoft-geometry` gives each
  primitive its face set's `CullBackfaces` byte (`fromsoft.face_cull`), and the
  lowering makes a material two-sided exactly where the face sets drawing it do
  not cull, splitting a material drawn both ways (a `<name>_two_sided` copy,
  counted); a drawing mesh without `fromsoft.face_cull` is an error. No class
  makes a material two-sided. On the scene region the flag is set on 2,503 of 2,598
  opaque, 51 of 261 cutout and 16 of 154 leaves LOD0 face sets, all 22 decal ones
  and none of the 3 water ones, and 16 source materials have LOD0 face sets of
  both kinds (the reader's decode; the BHD census checks it against SoulsFormats
  on every model). The bake's
  variants are per face-set cull too (`..._two_sided` for parts that do not cull).

- `drop-fromsoft-texture-transforms` (`DropFromSoftTextureTransforms::new()`, no
  options) takes the transform off every texture binding of a lowered FromSoft
  material (one with `fromsoft.material`), so each texture is sampled at the
  coordinates the geometry pass scaled by the albedo's tiling, as the earlier
  exporter sampled them; each non-identity transform left out is a counted
  approximation, by slot. For a target without texture transforms (ReSkate's
  dialect refuses them), after `lower-fromsoft-materials`.

The earlier skate route's materials are `bake-fromsoft-layers{max-size=512,sample-time=0}` then
`lower-fromsoft-materials{layers=preview,water-colour=48,98,122,190}`, after the legacy geometry above.

Generated texture names begin with `bake_`, including colour, normal, ORM and
emission, so downstream web caps remain a separate policy. Generated names follow
the source, never entity order: a bake group's index is its place in the order of
bake-input identities, its parts are in prototype source-id order (a repeated id
is refused), and a source material's level variant is
`bake_<group>_<level>_source_<digest>`, the digest the first 8 bytes of the
SHA-256 of the material's identity record (the JSON the reader dedupes materials
by); the lowering's packed ORM textures are `<normal>_orm_<digest>` the same way.
Variants of equal content keep their own names and entities: in the scene slice
(`baked_area`), 243 of the 680 variants share their content with another, in 98
sets, each within one group and level, whose sources differ only in their raw GX
records, which a variant keeps as provenance. No pass merges them; the trigger
for a merge pass (one that merges materials of equal lowered content and counts
the GX provenance it drops) is a texture or material budget hit on the Sekiro
route. Core PBR bindings are
authoritative. Gloss becomes roughness only in the PBR helper; emission uses its
normalised sRGB texture and explicit HDR strength. Source materials without a bake
are lowered by the lowering pass. The bake's pass log counts unique source
materials separately from weight levels, including channel presence/absence and
non-unit periods, and names frozen inputs and quantisation; the lowering's log
retains the source lowering's losses.

The `baked_area` installed-data test exercises the bake pass using the existing
Sekiro directory, decoder and scene-reference inputs. Its inventory and binding
assertions do not replace the independent per-image and written-scene gates.

Baked reflectance lowering captures output3 internally without changing public
`Prepared::bake` results or allocating another RGB plane for public callers.
For the traced Multiple, MultipleGround and Multiple_Emissive switches 0..33,
except variable Multiple18, fully written RGB must equal the exact source
encoded dielectric literal at every texel. These materials use metallic factor
0 and count the source F0 .03 to target .04 approximation. Missing, partial,
nonfinite, variable and unsupported values have no metallic evidence: they also
use factor 0, counted as unsupported reflectance and as the unevidenced-metallic
approximation; ORM blue 255 remains a neutral multiplier. Counts are per
newly referenced output material, excluding unused bake levels and repeated
primitive uses. The optional `oracles.scripts.sekiro/reflectance.json` gate checks the
picked jar's original shader literal and DDS inputs, capture-path neutrality,
and unchanged other lowered channels; final old/new export parity is separate.


`AreaBake::from_world` groups the inputs of a lowered area, the inventory the
bake pass bakes, without decoding or baking pixels.

Static clustering uses the material identity with only raw GX projected out.
The interpreter does not consume GX, so retaining extra source bytes must not
change the weight clustering pool or image results. Full source identities,
including exact GX payloads, remain on each referenced output material; equal
bake inputs share image entities and weight centers, not source provenance.
The bake log's `source materials baked` counts those full source identities,
while `bake groups` and the gloss/emission presence counters count bake-input
groups. `bake levels` counts baked images' weight levels, not per-source
material entities.
UV scale, alpha policy and every other material field remain in the group key.

## Example

```rust
use primase_core::pass::PassRegistry;

fn main() {
    let mut passes = PassRegistry::new();
    primase_fromsoft::sekiro::passes::register(&mut passes);
    println!("{}", passes.describe("lower-fromsoft-sky").expect("registered pass"));
}
```

This example needs no game files.

## Environment and source state

`Options::world_state` accepts an explicitly sourced flag set for Sekiro. The installed common event 100 is evaluated
in script order, its enabled map groups are joined to the actual sky domes, and the existing sky and irradiance
axes receive the selected values. Reports retain the script hash, input flags and selected actions; unsupported
instructions and undecidable branches are errors. This does not guess the player's flags or select Elden Ring's weather.

`Options::irradiance` reads Sekiro's irradiance regions and coefficient packages;
`lower-fromsoft-irradiance` lowers the spatial first-order data of the regions the game draws diffusely: a box's
type data word 0x24 states its terms (0 diffuse and specular, 1 diffuse only, 2 specular only), and a box of any other
value is refused by name. Low-resolution texture twins are opt-in source
forms (`core.texture_forms`); choosing an image is explicit. `fromsoft-formats` exposes Elden Ring's switch
parameters separately; the world reader does not select an image by distance.
`sample-fromsoft-draw-params` follows the source time axis and blends slots at the caller's hour.
`lower-fromsoft-environment` lowers Elden Ring's tone-map exposure to `core.atmosphere`'s `AutoExposure`, including
its meter and adaptation; source-unit volumetric fog remains descriptive.
