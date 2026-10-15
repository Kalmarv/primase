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

`PRIMASE_FROMSOFT_MODEL_ORACLE` supplies independently exported, pre-bake area
geometry. Its `models.jsonl` records model key (`area/model` or `obj/model`),
missing-model status, per-material exclusion and UV scale, and ordered parts.
Each part names its binary file and SHA256. A binary part starts with u32 vertex
and index counts, followed by positions f32x3, normals f32x3, UVs f32x2, and u32
indices, all little-endian. Generate these directly from the reference model
builder before any layered-material bake. Baked scene comparisons must report
part splitting, UV rescaling and material replacement as separate differences.

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

`PRIMASE_FROMSOFT_SCENE` supplies a reference skate-scene folder. Its source
labels connect `tables/placements.bin` to MSB parts. The gate checks prototype
references, exact translations and bases under the same stated tolerance.
Both gates also require `PRIMASE_SEKIRO_DIR` and `PRIMASE_OODLE6`.

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
`Material::skate_material` resolves colour and normal roles independently into
texture entities, with deterministic fallback colours for missing base data.
Metallic and emissive source slots survive even when the target preview does
not consume them. Material identity is chosen by the caller, independently of
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

## Area assembly and scene proof

`sekiro::read_area` assembles a selected map into an empty `World`, preserving
unbaked geometry, materials, source labels and explicit texture conventions.
It takes an install root, a trusted decoder and options; it never guesses paths.
A failed read leaves the destination unchanged. Region selection uses source
map east/north coordinates and transformed prototype bounds with an explicit
margin. Far map pieces are excluded by default. Missing model IDs are reported.

`sekiro::prepare_scene_assets` writes prototype GLBs and PNG images through
the toolkit into an empty asset directory, attaching file components required
by `primase_skate_scene::write`. The scene output directory is separate.
Normal maps retain the source convention until explicit consumer lowering:
glTF uses the stored tangents and swapped R/G; the skate path uses the
UV-derived frame, keeps R unswapped and inverts G. Both rebuild positive Z.
Raw map/object collision, ledges, lights and travel/spawn markers are assembled.
Layered shader baking, roof smoothing and opened-up surfaces remain separate passes.

Extras use named lowering policies. Ledges join endpoints within one centimetre
in Euclidean distance, retain chains at least one metre long, and select wood
or concrete from nearby game-map collision. The reference's quantized joining
leaves two additional near-closed castle curves open; retained points and all
326 surface assignments match. Lights use exact sRGB conversion; the intensity
multiplier of 100 and local +Z spot axis remain unverified conversion policies.
The castle comparison counts 70 channels differing from power-2.2 conversion
(maximum 0.008515), with 34 lights, 326 curves and 11 markers. Marker positions
are exact; yaw is compared within 0.0051 degrees of the rounded reference.

The installed-area integration gate uses `PRIMASE_SEKIRO_DIR`, `PRIMASE_OODLE6`,
`PRIMASE_FROMSOFT_SCENE`, `PRIMASE_SCENE_SKATE` (independent validator executable
or DLL), and `PRIMASE_FROMSOFT_OUTPUT` (a fresh output directory). Outputs remain
for review. It checks the written folder, not only the in-memory model:
prototype selection, topology and positions exactly; normalized normal
components within absolute 1e-6; UV differences only when explained by the
selected material's tiling, within four f32 epsilon scaled by magnitude;
material texture bindings and classes exactly; placement translations exactly
and bases within 1e-6 per unit scale. It counts tolerance matches separately.

`written_manifest_values` additionally audits a retained output folder supplied
by `PRIMASE_FROMSOFT_WRITTEN_SCENE` against `PRIMASE_FROMSOFT_SCENE`: every
per-part material field, cutoff at consumer f32 precision, texture identities,
source labels and region. Material deduplication retains full source bindings,
so material counts can exceed the reference's preview-only identities. Texture
pixel parity with the reference's normal-processing heuristic is not asserted;
source decoding is covered separately by the texture gates.

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
from the installed shader bundle and use the same mapping as a named assumption,
counted per emitted part in `Report::normal_basis_assumptions`. Unclassified
families are errors. Water's static texture uses this basis, but its runtime
normal buffer remains an unsupported dynamic effect. Layer blending also remains
part of the later shader bake; this conversion only preserves the chosen static
texture contribution. Downstream consumers must preserve the emitted tangent
stream rather than regenerate a different UV-derived frame.

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

`fromsoft-apply-rigid-skinning` requires and consumes `fromsoft.flver`, and
provides `fromsoft.rigid_model`. Raw FLVER parsing and raw geometry remain
available unchanged. Area assembly lowers before constructing render geometry
and selecting an object's dominant render-area collision material.

Static meshes use the implicit unit-weight binding in NORMAL.w. The pass applies
the bone hierarchy's scale, X/Z/Y rotation and translation to positions, the
inverse transpose to normals, and the linear transform to tangents. Reflections
reverse tangent handedness and face winding. Invalid hierarchies and singular
transforms are errors. Requested non-unit or multiple rigid influences are
refused and counted; no partial world mutation occurs on failure.

Dynamic meshes already store model-space bind-pose geometry and are animated at
runtime. They remain unchanged, counted separately, and are not submitted to
rigid lowering. This is a static bind-pose output, not an animation evaluator.
The full-castle gate covers 749 placed prototypes, with 8,660,257 stored vertices;
positions agree within 0.2 mm and inverse-transpose normal components within
2e-5 scaled by max(1, magnitude). Placed collision is compared bidirectionally
within 0.2 mm with surface IDs enforced, independent of convex triangulation.

### Interior blocker cards

The area reader excludes only meshes whose non-skipped resolved material uses a
`GXFlver_Col*` shader and an exact `m[0-9][0-9]_black_a` main albedo. Other meshes
in the same prototype remain renderable. Ordinary unlit ground fills and missing
albedos do not match. This is an explicit export policy: the source game draws
these black interior blockers, but exported worlds omit them from render geometry.
Their local geometry, material, original FLVER mesh index and placed transforms
remain in the `fromsoft.blocker_cards` resource. Scene output also records them in
`extensions.sekiro.blocker_cards`, with the rule, `drawn: false`, placement and
triangle counts, and `records`. They never become collision geometry.

`PRIMASE_FROMSOFT_BLOCKER_ORACLE` points to a directory with `cases.json`, an array
of objects containing `name`, `area`, `region` (null or source rectangle),
`placements` (source labels), `prototypes` (identity strings), `mesh_identities`
(array of `[prototype, original_flver_mesh_index]`), `meshes` and `triangles`.
The gate checks the shared geometry-selection stage used by the area reader.
Cases with `assemble: true` also run full area assembly, compare its retained
records and verify no matching material is attached to render geometry. This
allows inventories without choosing a normal basis for unrelated shaders. Exported part indices must be mapped
back to original FLVER mesh indices when layer splitting changes them.
