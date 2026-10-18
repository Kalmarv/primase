# primase-fromsoft

Read-only Sekiro archive access: raw RSA BHD5 envelopes, bucket indexes,
AES-128-ECB entry ranges, DCX DFLT/KRAK envelopes, and BND4 file tables.

`Archive::open(root)` indexes the five archive pairs. `read_hash(hash, decoder)`
and `read(path, decoder)` return decrypted, decoded bytes. `read_stored(hash)`
returns decrypted stored bytes including padding. `Header::read` and
`unwrap_rsa` expose the lower-level index operations; `Entry::decrypt` applies
only its listed ranges. SHA metadata is parsed and retained, not authenticated.

`Decoder { oodle, max_output }` takes an optional borrowed, caller-loaded Oodle
library and an explicit decompressed-byte ceiling. KRAK needs the native decoder;
DFLT does not. `Binder::read(bytes, &decoder)` returns IDs, names, flags, and
member bytes, including decoded nested DCX. Names are UTF-16 or ASCII; non-ASCII
legacy Shift-JIS names are rejected. Unsupported layouts and unaccounted bytes
are errors. BDT padding for non-DCX entries is retained for reference-reader
parity: a zero BHD logical size does not imply an empty entry.

`NameDictionary::read(path)` reads a caller-supplied UTF-8 path list. Hash
collisions retain all distinct normalized candidates. No dictionary, native
library, or game assets are distributed with this crate.

## Gates

Synthetic tests cover valid and malformed BHD5/DCX/BND4, RSA block geometry,
a NIST AES vector, encrypted ranges, nested envelopes, hash tables, truncations,
and single-byte mutations. Real gates skip when their environment variables
are unset. Use `cargo test -p primase-fromsoft -- --nocapture`.

- `PRIMASE_SEKIRO_DIR`: directory containing the five BHD/BDT pairs.
- `PRIMASE_OODLE6`: explicitly trusted version-6 native library.
- `PRIMASE_FROMSOFT_NAMES`: optional path dictionary for the named-count gate.
- `PRIMASE_FROMSOFT_BHD_ORACLE`: directory of independent reference C# decoder
  output, generated from the same installation. Its files are:
  - `Data1.plain` through `Data5.plain`: complete RSA-unwrapped headers.
  - `entries.tsv`: shard, eight-digit hex hash, container label, decoded length,
    uppercase SHA-256, tab-separated.
  - `binders.tsv`: shard, hash, zero-based member index, signed ID, member name,
    decoded length, uppercase SHA-256.
  - `errors.tsv`: shard, hash, parser name, decimal failure offset, normalized
    error detail. Empty when the reference rejects nothing. Populate only from
    verified reference failures; this is never a list of ignored entries.
  - `samples.tsv`: hash and archive path, tab-separated; corresponding exact
    decoded bytes in `samples/<hash>.bin`. Include varied entries and every
    indexed path for a selected map area.

The census attempts every indexed entry. Accepted results must match reference
lengths and digests; samples compare actual bytes. Rejections count as agreement
only when both readers reject at the same parsing boundary with equivalent
errors. Unexpected acceptance, a missing reference entry, or a different error
fails the differential gate. Reports distinguish decoded entries from agreed
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

The TPF corpus gate extends the archive oracle directory with
`tpf/packages.jsonl`: one object per discovered package, with `id` (shard/hex
hash, followed by `/b<index>` or `/s<index>` for single/split binder children),
`area`, package size/SHA256, platform/flags/encoding, and a `textures` array.
Each texture records name/format/kind/mipmaps/flags, DDS size/SHA256 and, for area
2D images, width/height and decoded RGBA SHA256. Area DDS inputs reside at
`tpf/area-dds/<uppercase-sha256>.dds`; none belong in the repository.

The pixel gate additionally requires `PRIMASE_FROMSOFT_DDS_ORACLE`, an explicitly
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

`Model::geometry(materials)` selects the first LOD0 face set per mesh, compacts
vertices in first-use order, converts through the core frame, normalizes stored
normals, and chooses winding from the summed geometric/stored-normal agreement.
Caller-supplied material decisions control mesh exclusion and main-layer UV
scale. It retains vertices referenced by faces that later fail the area check.
It does not bake layered materials or split parts into baked weight levels.

The archive oracle directory adds `flver/models.jsonl`, identified with the same
binder paths as the TPF census. Each record contains the input SHA256, version,
layout declarations and meshes. Each mesh records its material, vertex count,
vertex digest, layout indices and face sets (flags, strip/culling booleans,
index digest and triangulated digest). Vertex digest input is little-endian:
position f32x3, normal f32x3, normal W i32, UV count u32 and f32x3 values,
tangent count u32 and f32x4 values, colour count u32 and RGBA f32x4 values,
repeated for every vertex. Indices are u32. Digests are uppercase SHA256.

Area geometry is checked against the SoulsFormats dump of every placed model
(`PRIMASE_FROMSOFT_RIGID_ORACLE`, `tests/rigid_census.rs` and `tests/sekiro_scene.rs`);
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

`msb::MapLayout::read(bytes)` reads Sekiro's section chain, models, part names,
model references, stored transforms and MapOffset events. Unrelated gameplay
records remain opaque; their envelopes, rather than their schemas, are checked.
Names remain as stored, with part indices distinguishing repeated names.

Parts already occupy the common world frame. Their local scale is followed by
Euler rotations X, Z, Y, then translation. `Part::transform` uses ordinary f64
trigonometry, rounds basis components to f32, and applies the core frame's Z
reflection for handedness. MapOffset places map-local resources separately;
it must not be added to part placements a second time.

`PRIMASE_FROMSOFT_MSB_ORACLE` supplies `maps.jsonl` from an independent reader.
Each record has `path`, decoded-file `sha256`, an ordered `models` name array,
`parts` and `offsets`. Parts contain `name`, `kind`, nullable model name,
`position`, `rotation` in degrees, `scale` and `transform`. Offsets contain
`name`, `position`, `degrees` and `transform`. Transforms are 12 f32 values,
four columns of three components, in the right-handed output frame.
Stored values and model references must match exactly. Derived bases allow
absolute error 1e-6 after division by max(1, largest absolute local scale);
translations remain exact. Gates count exact and tolerance matches separately.

Parts also carry their entity ID and eight entity group IDs as stored (sentinels -1 and 0 included); event scripts
address a part by any positive one of them (`Part::entity_ids`). `PRIMASE_FROMSOFT_STATE_ORACLE` is a folder from an
independent reader. Its `entities.jsonl` has one record per map file: `path` and `parts`, each with `index`, `name`,
`entity_id` and `entity_group_ids` (8 integers). The gate (`tests/msb_entities.rs`) requires the same part count and
names and exact values.

Each part's six MSB shadow controls are kept raw in `msb::ShadowFlags`.
`PRIMASE_FROMSOFT_SHADOW_ORACLE` is a folder holding one original `map.msb` and
an independent reader's `shadows.json` (`sha256`, ordered `parts` with `name`
and the six fields). The gate compares every part exactly. With
`PRIMASE_SEKIRO_DIR`, `PRIMASE_OODLE6` and `PRIMASE_FROMSOFT_SCENE` (for its
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
interpreted. `PRIMASE_FROMSOFT_STATE_ORACLE`'s `emevd.jsonl` has one record per script: `path`, `sha256` (of the
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
are not exactly seven whole words, or are filled by substitution, counts as undecidable. `PRIMASE_FROMSOFT_STATE_SCENE` is a scene of one whole block exported by
an independent implementation with `extensions.sekiro.world_state` (`parts`: `source`, `kind`, `states` per flag
`off`, `on` or `never`); `tests/state_membership.rs` requires the same membership for every part it lists that our
rules decide, counts and prints the parts we leave at their default because something uninterpreted also names them,
and fails on a part decided by only one side otherwise.

The area's placements are checked against a SoulsFormats dump of its MSB in `tests/sekiro_scene.rs` (below).

## Material definitions and preview lowering

`mtd::MaterialDefinition::read` validates typed structure, markers, counts
and file bounds. Declared block lengths are diagnostic: stock values can
exceed their structurally consumed end, including the file end. The reader
retains every declared and consumed length without reading outside the file.
This structure-authoritative rule is not a claim about the game's parser control flow. Shift-JIS strings decode strictly; malformed text is
an error at its string offset. No replacement characters are introduced.

`materials::resolve` applies an explicit main-layer preview policy. Nonempty
FLVER slot paths override fixed MTD paths. Among available albedos it chooses
slot 8, then the lowest slot; normals prefer a matching stem, then slots 3 and
0, then the lowest slot. When shader-qualified names share a numeric slot,
bindings declared by the selected MTD take precedence over foreign-family
bindings; all bindings remain available to the later bake. The caller supplies
texture availability. Shader names
and blend/alpha parameters select preview classes; name-based collision surface
choices are a documented approximation, not a claim about shader semantics.
The selected albedo's positive FLVER tiling scale is kept for geometry lowering.

`fromsoft.material` records these choices and all resolved bindings.
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

`PRIMASE_FROMSOFT_MATERIAL_ORACLE` supplies two independent outputs:
`mtd-boundaries.jsonl` lists independently audited block starts, kinds, declared
and consumed lengths for every file. The gate counts length mismatches.
`mtd.jsonl` lists binder names, decoded SHA256, shader, description, typed
parameters and every texture declaration; `models.jsonl` lists model keys,
texture availability and per-FLVER-material assignments. The first gate compares
stored fields exactly, including f32 bits. The second isolates selection policy
using the oracle's availability input and compares assignments, then checks
the material references on each part in `PRIMASE_FROMSOFT_SCENE`. It is not a
gate of archive texture discovery, which belongs to area assembly. Both require
`PRIMASE_SEKIRO_DIR` and `PRIMASE_OODLE6`; absent inputs skip explicitly.

## Area reading and its passes

`sekiro::read_area` reads a selected map into an empty `World`. It takes an
install root, a trusted decoder and options; it never guesses paths, and a failed
read leaves the destination unchanged. Reading lowers nothing:

- each placed model is a prototype holding its FLVER as stored (`fromsoft.flver`:
  skeleton, every mesh with all its face sets, original material indices and every
  vertex channel), its source identity (`fromsoft.prototype`, `far` for the
  distant-backdrop `m9xxxxx` series) and the material entity of each FLVER
  material (`fromsoft.material_slots`), each holding `fromsoft.material`;
- placements are `core.instance` with their MSB transform, source label, shadow
  controls and, with states read, conditions;
- every texture a kept material binds is kept as stored
  (`fromsoft.texture_payloads`), and the G-buffer programs of its layered
  materials (`fromsoft.shader_programs`);
- map and object collision are `core.collision` soups with `fromsoft.collision`
  (source group, water); lights are `fromsoft.light` records where they stand
  (stored colours, powers, cone, radius and tangent); hang-edge segments and
  spawn points are the `fromsoft.area_extras` resource.

A region (source east/north rectangle) is an explicit partial read: a placement
is kept when its model's extent (every vertex a face set uses, static vertices
posed by their bones) reaches the region within `region_margin`; collision
triangles whose centre lies more than 50 m outside it, and lights and spawn points
outside it, are not read. `Report::notices` returns what the read left out or could
not find. Object collision takes a convex body's material from the model's largest
colliding render surface, measured on the stored level-0 face sets.

Passes (`sekiro::passes::register`), each choosing explicitly:

- `drop-fromsoft-far` leaves out the distant-backdrop placements and prototypes.
- `fromsoft-apply-rigid-skinning` poses `fromsoft.flver` into `fromsoft.rigid_model`
  beside it (below).
- `lower-fromsoft-geometry` makes `core.mesh`: one primitive per mesh and face set,
  level 0 on the prototype and levels 1 and 2 as `core.lods` forms for
  `select-lod`; positions, unit normals, stored tangents and UV0 in the world frame,
  a layered material's Byte4C members as raw `Color(n)` streams listed by
  `fromsoft.layer_inputs`. Its options default to the faithful reading:
  `pose=rigid|stored`, `uv-tiling=keep|apply` (recorded per material as
  `fromsoft.uv_tiling`), `winding=stored|normals`, `degenerates=keep|drop`,
  `leave-out=sky+light-volume+shadow`, `blocker-cards=draw|record`. Motion-blur
  face sets, vertex colours and UV sets beyond UV0 are not lowered and are counted.
- `lower-fromsoft-lights{candela-per-power=...}` makes `core.light` from the BTL
  records: colour bytes read as sRGB, intensity by the program's calibration (no
  default), range the stored radius (at least `min-range`), a spot's cone from the
  stored angle with an inner cone at `1 - spot-blend` (0.15). The records stay.
- `lower-fromsoft-extras` makes each hang segment a two-point `core.curve` of kind
  `ledge` and each spawn point a `core.marker` of kind `travel`, in source order,
  named as stored. Joining ledges (`primase_meshops::chain`), the surface under
  them (`primase_meshops::SurfaceIndex`), choosing a spawn and naming markers are
  a program's.

The geometry the earlier reader produced by itself is this explicit composition:
`drop-fromsoft-far, fromsoft-apply-rigid-skinning,
lower-fromsoft-geometry{uv-tiling=apply,winding=normals,degenerates=drop,leave-out=sky+light-volume+shadow,blocker-cards=record},
select-lod{form=finest}`. On m11_01 it reproduces that reader's placements, meshes
(streams, indices, material identities), collision, lights, joined ledges and
travel markers exactly.

`tests/sekiro_scene.rs` reads slice A with that composition and checks it against
a SoulsFormats dump of the area (`PRIMASE_FROMSOFT_RIGID_ORACLE`): every placement
is an MSB part with its model, translation exact and basis within 1e-5; every
lowered vertex is a vertex of its FLVER in model space within 0.2 mm; four
material fixtures worked out by hand from the dump keep their class, base and
normal texture. `cli/tests/fromsoft_collision_scene.rs` compares the read collision
with the scene our earlier C# exporter wrote (`PRIMASE_FROMSOFT_SCENE`, a regression
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

Call `sekiro::normals::lower(world, Consumer::Gltf)` for consumers that honor
supplied tangents: the shared image pass swaps R/G and rebuilds Z, preserving
alpha and all mips. It does not flip w again. Curl/integrability verdicts describe
authoring patterns; they never select a per-texture sign exception.

`Consumer::Skate` keeps the source R/G order, flips green and rebuilds Z.
A consumer test found green inverted relative to Studio's UV-derived frame;
Studio ignores supplied tangents. This rule applies to every texture with no
curl-based exceptions. The stored frame is replaced by the UV-derived frame as
a counted approximation: measured sheared/other regions do not become exact
merely by choosing a green sign. Both lowerings preserve alpha and all mips,
update scene normal declarations and discard stale file provenance.

`sekiro::normals::consumer_basis` lists the inspected families. Three legacy
families (`GXFlver_Col`, `GXFlver_ColDif`, `GXFlver_ColDifSpcBumpIbl`) are absent
from the inspected shaderbdle bundle (not proven absent from the install) and
use the same mapping as a named assumption,
counted per emitted part in `Report::normal_basis_assumptions`. Unclassified
families are errors. Water's static texture uses this basis, but its runtime
normal buffer remains an unsupported dynamic effect. Layer blending also remains
part of the later shader bake; this conversion only preserves the chosen static
texture contribution. Downstream consumers must preserve the emitted tangent
stream rather than regenerate a different UV-derived frame.

`GXFlver_ColDifSpcBumpEmiIbl` is present in the separate GX shader bundle. Its
G-buffer program samples t3 RG as `2*v-1`, multiplies red by the normalized
bitangent and green by the normalized tangent, rebuilds positive Z, and
normalizes the result. Sampled blue goes separately to o1.x, not to normal Z.
It uses the evidenced source basis and shared Z reconstruction.

`PRIMASE_FROMSOFT_NORMAL_FRAMES` supplies `frames.jsonl` with independently sampled
unit source normal `n`, tangent `t`, handedness `w`, and a model `key`. Two off-axis
normal probes per frame compare the transformed consumer formula with our glTF
basis at absolute component tolerance 1e-6. `PRIMASE_FROMSOFT_NORMAL_CURL` supplies
a JSON array of independently measured `name` and `verdict` values; the gate
reports all classifications and names every OpenGL verdict as a measurement,
not a conversion exception. These gates require no installed archive reads.

## Collision reader (in progress)

Raw map collision uses `primase-havok` for tagfiles, compendia and compressed
meshes. The FromSoft layer maps left-aligned shape keys through fsnp material
records. All-ones invalid keys are counted; missing materials remain -1 and are
counted separately. Contradictory bindings for a valid key fail.

Area assembly selects high-resolution map collision placed with hit filters 8
(world) and 27 (water). This is a named rideable-map lowering policy. HitMtrl
IDs map by their base value modulo 100 to the standard skate surfaces. Blockers
and clouds are excluded; water gets a separate water-mode soup. Selection uses
triangle centroids within the requested region plus 50 metres. Map offsets
apply once, and the core source frame reflects Z and reverses winding.

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

`PRIMASE_FROMSOFT_BLOCKER_ORACLE` points to a directory with `cases.json`, an array
of objects containing `name`, `area`, `region` (null or source rectangle),
`placements` (source labels), `prototypes` (identity strings), `mesh_identities`
(array of `[prototype, original_flver_mesh_index]`), `meshes` and `triangles`.
The gate reads each case's area and region, lowers it with the explicit legacy
composition above, compares the recorded cards and verifies no matching material
is drawn. Exported part indices must be mapped back to original FLVER mesh indices
when layer splitting changes them.

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

The optional `PRIMASE_FROMSOFT_LAYER_PILOT` fixture directory contains:

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
