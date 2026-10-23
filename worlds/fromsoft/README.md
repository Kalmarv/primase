# primase-fromsoft

Read-only FromSoftware archive access (Sekiro, Elden Ring): raw RSA BHD5 envelopes, bucket indexes, AES-128-ECB
entry ranges, DCX DFLT/KRAK envelopes, and BND4 file tables.

What differs between the games and is not stated by their files is data in one descriptor, `Game` (`game.rs`), with
each game's table in its own module (`sekiro`, `elden`): its archive set (`ArchiveSet`: the BHD5/BDT pairs, whether
each is required, the path hash, the BHD5 file-record layout and where its keys come from). Formats are read by what
their bytes state.

| | Sekiro | Elden Ring |
|---|---|---|
| pairs (shard) | Data1..Data5 (1..5) | Data0..Data3 (0..3), DLC (4, optional), sd/sd (5), sd/sd_dlc02 (6, optional): UXM's archive list order |
| path hash | base 37, u32 (`path_hash`) | base 133, u64 (`path_hash64`) |
| file record | u32 hash, u32 padded size, u64 offset, SHA and AES offsets, u64 logical size | u64 hash, u32 padded and u32 logical size, u64 offset, SHA and AES offsets |
| keys | published, built in | supplied by the caller (`Keys::from_folder`: one PKCS#1 PEM per pair, `Data0.pem`, `sd.pem`, ...) |

`Archive::open(root)` indexes Sekiro's five pairs with the built-in keys; `Archive::open_game(root, game, &keys)`
indexes any game's (an optional pair the install lacks is listed in `absent`). Entry hashes are u64 throughout
(Sekiro's zero-extended). `read_hash(hash, decoder)` and `read(path, decoder)` return decrypted, decoded bytes.
`read_stored(hash)` returns decrypted stored bytes including padding. `Header::read(plain, &record)` and
`unwrap_rsa` expose the lower-level index operations; `Entry::decrypt` applies only its listed ranges. SHA metadata
is parsed and retained, not authenticated.

`Decoder { oodle, max_output }` takes an optional borrowed, caller-loaded Oodle
library and an explicit decompressed-byte ceiling. KRAK needs the native decoder;
DFLT does not. `Binder::read(bytes, &decoder)` returns IDs, names, flags, and
member bytes, including decoded nested DCX. Names are UTF-16 or ASCII; non-ASCII
legacy Shift-JIS names are rejected. Unsupported layouts and unaccounted bytes
are errors. BDT padding for non-DCX entries is retained for reference-reader
parity: a zero BHD logical size does not imply an empty entry.

`NameDictionary::read(path)` reads a caller-supplied UTF-8 Sekiro path list, `read_for(path, game)` any game's,
keyed by its path hash. Hash collisions retain all distinct normalized candidates. No dictionary, native library,
key or game asset is distributed with this crate.

## Gates

Synthetic tests cover valid and malformed BHD5/DCX/BND4, RSA block geometry,
a NIST AES vector, encrypted ranges, nested envelopes, hash tables, truncations,
and single-byte mutations. Real gates skip when their environment variables
are unset. Use `cargo test -p primase-fromsoft -- --nocapture`.

- `installs.sekiro`: directory containing the five BHD/BDT pairs.
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
`tpf::Texture::image()` imports 2D DDS through `primase-dds`, preserving encoded
mips and colour-space tags. Cube/volume payloads are retained by the container
reader but explicitly rejected by this 2D import. Bindings assign semantic roles
and normal conventions; package parsing does not infer them from a name.

The TPF corpus gate reads its own oracle directory, `oracles.soulsformats.sekiro/tpf`,
which is not built yet (a stated coverage gap; see CONTRIBUTING.md), with
`packages.jsonl`: one object per discovered package, with `id` (shard/hex
hash, followed by `/b<index>` or `/s<index>` for single/split binder children),
`area`, package size/SHA256, platform/flags/encoding, and a `textures` array.
Each texture records name/format/kind/mipmaps/flags, DDS size/SHA256 and, for area
2D images, width/height and decoded RGBA SHA256. Area DDS inputs reside at
`area-dds/<uppercase-sha256>.dds`; none belong in the repository.

The pixel gate additionally requires `oracles.scripts.sekiro/dds`, an explicitly
trusted .NET assembly launched with `dotnet`. For each DDS path on stdin it
writes a binary response to stdout: status byte (0 success), little-endian u64
length, and that many RGBA8 bytes (UTF-8 error text for nonzero status). Diagnostics
must go to stderr. It uses the independent C# decoder with BC1 punch-through
alpha enabled; BC4 grayscale presentation is normalized to red-only by the gate.
Pillow 11 is queried through the shared texture-codec test oracle. Production
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
half of the header word at 0x68), whose multi-stream layouts neither this reader nor
SoulsFormatsNEXT decodes.

`Model::geometry(materials)` selects the first LOD0 face set per mesh, compacts
vertices in first-use order, converts through the core frame, normalizes stored
normals, and chooses winding from the summed geometric/stored-normal agreement.
Caller-supplied material decisions control mesh exclusion and main-layer UV
scale. It retains vertices referenced by faces that later fail the area check.
It does not bake layered materials or split parts into baked weight levels.

The archive oracle directory (`oracles.soulsformats.sekiro/census`, `oracles.soulsformats.elden/census`; `tests/flver_census.rs`
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
| spawn-point region | type 8 | not read yet (`spawn_points` refuses) |

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
`hit_filter` (collision parts, else null). Repeated names follow the reference's
convention: later uses become `"<name> {n}"`, in file order.

`btl::LightBank::read` reads versions 16 (Sekiro) and 18 (Elden Ring), which share the 232-byte light record, and
keeps the stored version; other versions are refused. Elden Ring's banks are checked against SoulsFormatsNEXT's BTL
(`btl.jsonl`, `tests/btl_census.rs`): one record per bank the install holds, `path`, then either `error` (the
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
entity ID or one of its groups) is disabled while any of its four (flag, state) pairs holds. `read_area` then makes a
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

`oracles.soulsformats.sekiro/materials` supplies two independent outputs:
`mtd-boundaries.jsonl` lists independently audited block starts, kinds, declared
and consumed lengths for every file. The gate counts length mismatches.
`mtd.jsonl` lists binder names, decoded SHA256, shader, description, typed
parameters and every texture declaration; `models.jsonl` lists model keys,
texture availability and per-FLVER-material assignments. The first gate compares
stored fields exactly, including f32 bits. The second isolates selection policy
using the oracle's availability input and compares assignments, then checks
the material references on each part in `oracles.baseline.sekiro/scene`. It is not a
gate of archive texture discovery, which belongs to area assembly. Both require
`installs.sekiro` and `tools.oodle6`; absent inputs skip explicitly.

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
a float3) and `samplers` (`type`, `path`, `key`, `unknown`); `tests/matbin_census.rs`). The reference reads a short
value's declared size past the next item; the gate compares the stored prefix and counts such values. The area read
does not resolve materials against them yet.

## Area reading and its passes

`sekiro::read_game_area(&Install, map, ...)` reads any game's map through the same read (`read_area` is Sekiro's,
`AreaReader::of(install, ...)` the reader adapter's); an `Install` is the folder, its `Game` and its archive keys.
What differs between the games and is not stated by their files is `Game`'s: where a map keeps its files
(`map_folder`: Sekiro `/map/<map>`, Elden Ring `/map/<mAA>/<map>`), how a placed object's model is found
(`object_model`: Sekiro's `/obj/<model>.objbnd.dcx` with its HKX, Elden Ring's `/asset/aeg/<group>/<model>.geombnd.dcx`)
and source-identity prefix (`obj/`, `asset/`), the distant-backdrop series (Sekiro's `m9xxxxx`), and the larger maps
above a map (`parent_maps`: Elden Ring's open-world tiles `mAA_XX_ZZ_00` sit in 512 m `_01` and 1024 m `_02` tiles,
whose layouts place the small tile's big assets as `<tile>-<part>` in their own frame, the small tile's centre at
`(i - (p << L)) * 256 + 128 - (256 << L) / 2` metres there). `Options::parents` (default `None`, the map's own layout)
set to `Include` also reads those parts, moved into the map's frame by that offset. The map layout's generation says
the rest (part kinds: Sekiro's objects, Elden Ring's assets; spawn regions).

Elden Ring's read does not yet resolve material binaries (its materials keep their FLVER bindings, no texture is read),
read asset collision binders, world states (`states: event` is refused) or spawn points, or decode SpeedTree models
(their placements are left out, by model): each is in `Report::notices`. `tests/elden_area.rs` accounts for every map
piece and asset part of the Academy of Raya Lucaria (m14_00_00_00) and of the Shaded Castle tile with its parents
against SoulsFormatsNEXT's layouts, translations exact.

`sekiro::read_area` reads a selected map into an empty `World`. It takes an
install root, a trusted decoder and options; it never guesses paths, and a failed
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
  `.shaderbdle` stores for it, for every pass the game draws it with, as DXBC
  and under the file name that says the pass, plus the shader's `.metaparam`:
  texture slots, the samplers each render pass binds per stage, the parameters
  and their constant-buffer offsets) linked from the material by
  `fromsoft.material_shader` (absent where the install holds no bundle of the
  shader: the `GXFlver_*` shaders; `Report::missing_shader_bundles`), and the
  G-buffer programs of its layered materials (`fromsoft.shader_programs`, for
  the layer bake);
- each high-resolution map collision file that MSB collision parts name, of every
  hit filter, is a `fromsoft.map_collision` entity: the naming parts (name, hit
  filter, clause with states read), every triangle in the world frame and its
  HitMtrl ID as the shape key binds it (-1: none);
- each object prototype with collision carries every stored collision file with its
  resolution (`fromsoft.object_collision`: Sekiro's main HKX in the object's binder, high;
  Elden Ring's `aegnnn_mmm_h.hkx` and `aegnnn_mmm_l.hkx` in the asset's `_h`/`_l`
  `.geomhkxbnd` binders, high and low: every collidable asset has a low one, a subset a
  high one) and in each every body (name, stored motion type, shape class and its
  geometry as stored: a compressed mesh's triangles in the prototype's space and
  their HitMtrl IDs; a convex shape's core (vertices, planes, faces), convex
  radius, a capsule's segment, and the map from its own space into the
  prototype's; convex shapes store no material);
- lights are `fromsoft.light` records where they stand (stored colours, powers,
  cone, radius and tangent); hang-edge segments and spawn points are the
  `fromsoft.area_extras` resource; what the read covered (area, region, margin,
  states) is `fromsoft.read_scope`.

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
default presence, and the shadow controls' interpretation.

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
  `leave-out=sky+light-volume+shadow`, `blocker-cards=draw|record`. Motion-blur
  face sets, vertex colours and UV sets beyond UV0 are not lowered and are counted.
- `lower-fromsoft-lights{candela-per-power=...,spot-blend=...}` makes `core.light` from the BTL
  records: colour bytes read as sRGB, intensity by the program's calibration (no
  default), range the stored radius (at least `min-range`), a spot's cone from the
  stored angle with an inner cone at `1 - spot-blend` (no default: the cone falloff
  is not established). The records stay.
- `lower-fromsoft-collision{hit-filters=...}` lowers the map collision files that
  parts of the chosen hit filters name (required) into `core.collision` soups
  (`game`, `water`, `game_state<n>` with the file's condition), each triangle with
  its HitMtrl class's surface (`default` without one, counted); `water=` and
  `leave-out=` take HitMtrl classes (an ID modulo 100), `reach=N` crops N metres
  around the read region (every option required: empty classes for none, `reach=none`). The records it lowered are consumed; files only other
  filters' parts name stay as source.
- `lower-fromsoft-object-collision{resolution=high|low|high-else-low,convex=tessellate|leave-out}` places every
  placed object prototype's bodies of the files of the chosen resolution (a prototype
  without one is counted; extern shape instances are convex bodies of their own) with each placement into an `objects` soup
  (`objects_state<n>` for conditioned placements), at their stored pose whatever
  their motion type (counted). A soup cannot hold a rounded shape, so `convex`
  is required: `tessellate` gives a convex body the intersection of its planes
  offset by its radius (sharp where Havok rounds, a capsule a box around its
  segment; both counted), `leave-out` drops it (counted). Every option is required: `leave-out=`,
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

The collision and geometry the earlier reader produced by itself are this explicit
composition: `lower-fromsoft-collision{hit-filters=8+27,water=22+36,leave-out=29+40,reach=50},
lower-fromsoft-object-collision{resolution=high,convex=tessellate,leave-out=22+29+36+40,leave-out-motion=2,convex-surface=render,reach=50},
drop-fromsoft-far, fromsoft-apply-rigid-skinning,
lower-fromsoft-geometry{pose=rigid,uv-tiling=apply,winding=normals,degenerates=drop,leave-out=sky+light-volume+shadow,blocker-cards=record},
select-lod{form=finest}`. On m11_01 it reproduces that reader's placements, meshes
(streams, indices, material identities), collision, lights, joined ledges and
travel markers exactly.

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

Raw map collision uses `primase-havok` for tagfiles, compendia and compressed
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

Elden Ring's collision is Havok 2018.1 (`primase-havok`'s `SDK_VERSIONS`). Its stored compressed meshes are
checked against HKLib ac40bc9 (`havok.jsonl` of `oracles.soulsformats.elden/format`, `tests/havok_census.rs`): one record
per file, `file` (`<binder path>:<member>`), then `error` or `shapes`, each `kind`, `bits_per_key`, `sections`
(`codec` as six f32 bit patterns, `first_packed_vertex`, `packed_vertices`, `first_shared_vertex`,
`first_primitive`, `primitives`), `packed_vertices`, `shared_vertices`, `shared_vertices_index`, `primitives` (four
indices each), `triangle_to_shape_key` (FromSoft's custom-param shape, else null) and `extern_shapes` (each instance's
`transform`, its sixteen stored floats in order, `scale`, the instanced shape's `kind`, `convex_radius` and stored
`vertices`, as bit patterns). A file's shapes are compared as a set; the triangle decode built on them is not part of
this check, and the extern instances' transforms as the reader takes them are checked against the stored floats.

Object bodies (`collision::read_bodies`) are compressed meshes and the convex classes (box, polytope, capsule,
cylinder, plain convex, triangle). A compressed mesh's primitives whose second and fourth indices are equal are
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

`lower-fromsoft-geometry{blocker-cards=record}` leaves out of the drawn geometry only meshes whose non-skipped resolved material uses a
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
sampler reproduce runtime derivatives or sampler descriptors. Albedo RGB is
linearized before filtering; alpha and data channels remain linear. Missing
bindings, zero-initialized buffer words, and frozen policies are counted.
Normal output inverts the binary G-buffer bias and retains the source tangent
basis; consumer lowering remains separate. The main albedo supplies alpha.

The optional `oracles.baseline.sekiro/layer-pilot` fixture directory contains:

- `cases.json`: array of `{id, model, mtd, shader, switch}`.
- `<id>.mtd` and `<id>.ppo`: original material and pixel-shader bytes.
- `<id>.json`: `bindings` entries `{kind,path,scale:[u,v]}` and `images`
  keyed by texture stem, each `{width,height,file,dds}`. `file` is RGBA8
  decoded by the independent value oracle; `dds` is the original texture.
- `reference.json`: array `{id,cap,variant,size,period:[u,v]}` with optional
  timing metadata. Variants are identity, mixed weights/tint, and all weights
  one with detail zero (defined in `tests/layer_pilot.rs`).
- `<id>-<cap>-<variant>-{albedo,normal}.rgba`: independent bake output.

The pilot reports max/p99 per channel, time, decoded image bytes, and executed
instructions/samples per pixel. It compares identical decoded inputs separately
from D3D-decoded inputs. Arithmetic with identical inputs has a one-level bound.

The pilot also requires `d3d-control/<id>-<cap>-<variant>-{albedo,normal}.rgba`:
run the same independent producer with only its decoded texture pixels replaced
by the D3D decoder's RGBA8 images. This control isolates input convention effects
from shader arithmetic. For D3D inputs this control is the gate oracle: the GPU
uses the D3D BC convention. The original decoder differs from that convention;
the shader amplifies its input differences to measured albedo maxima [2,3,4,0]
and normal maxima [2,2,1,0]. Those original-oracle differences remain reported
diagnostics. The same-input arithmetic bound remains one level.

Area grouping is an explicit static approximation: triangle-mean layer inputs,
snow weighted by the upward-facing normal, triangle area times placement count,
at most three weighted clusters, and weights quantized to 1/64. Groups below
two percent or within 0.08 on every component merge. Numeric lexicographic
ordering makes equal-weight groups deterministic without a text-format dependency.
Splitting preserves every vertex stream through the shared primitive compactor.
The bounded common-period policy searches up to sixteen UV units and records
when it falls back to the main texture's repeat. These policies are not runtime
shader behavior or format requirements.

The layer bake's area grouping, UV periods and images have no independent reference: the earlier inventory and
its control came from our earlier exporter's layer bake. An open coverage gap (build one when a step 20 or in-game Sekiro difference points at this area; the reference
would be the game's layer shaders run on WARP, `oracles.baseline.sekiro/layer-pilot`).

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
- `lower-fromsoft-materials{layers=preserve|preview}` (`LowerFromSoftMaterials`)
  lowers every source material to core PBR, texture transforms relative to the
  applied tiling. A layered material no pass baked needs the program's choice:
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
`lower-fromsoft-materials{layers=preview}`, after the legacy geometry above.

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
