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
Normal green direction is an explicit import policy, not inferred from curl.
Layered materials retain bindings for a later bake; collision/Havok, ledges,
lights and shader baking are not yet imported.

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
