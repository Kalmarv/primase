# Contributing to primase

How the workspace is laid out and the rules every crate follows. The design is `docs/DESIGN.md`.

## Roles: read this first

There is one way through primase: **source -> reader -> passes -> writer**. Every piece of code is one of the roles
below, and each rule is checked by the normal gates (`cargo clippy ... -D warnings`, `cargo test --workspace`).

| role | may | may not | checked by |
|---|---|---|---|
| **reader** | read the source it is given; attach core components and its own dialect (`gta.*`, `fromsoft.*`); keep the source as authored (bone-space geometry with its skeleton and pose, every level of detail, every state); count what it tolerated; load the meshes and textures a world references, when asked (its load step) | lower, bake, approximate, apply poses, pick levels of detail or states, select what a target draws, shape for a target (another dialect's components, target units), read options from the environment | I/O only in modules that opt in as `reader I/O` (clippy); the namespaces a reader declares, checked by `primase convert` for registered readers |
| **pass** | transform the world it is given: lower, bake, approximate, select, split, merge; declare what it requires, provides, removes and modifies; name and count every approximation | touch files, the environment or processes; run outside the pass manager | clippy's file/environment ban; every pass registered by name and every public lowering run by a pass (`cli/tests/architecture.rs`) |
| **writer** | serialize what its target declares, laying it out as its encoding needs (files, tiles, splits by encoded size) without changing the world; refuse the rest by name | lower, approximate, or select and merge what the world means for its target (a pass does that first), derive data a pass should own, read other files, change the world | writer entry points take `&World` (`cli/tests/architecture.rs`); I/O only in modules that opt in as `writer I/O` |
| **core** | world facts, the store, frames, the pass machinery and its generic passes, gates | game or target names, renderer tuning, file access, dependencies on other crates | crate dependencies, game names, clippy's file ban (`cli/tests/architecture.rs`) |
| **toolkit** | shared mechanisms that games supply data to | game branches, depending on formats or worlds | crate dependencies, game names |
| **formats** | file formats that are not one game's: their readers, writers and passes | depending on worlds or on formats other than `formats/gltf` | crate dependencies, game names |
| **test-input registry** (`toolkit/testdata`) | name and describe the real-data inputs tests read, games and third-party tools included; resolve them | game logic, depending on workspace crates, being a normal dependency of any crate | crate dependencies (not excusable in `architecture.toml`); `real_data_variables_are_declared` |
| **cli, pipelines, examples, scripts** | compose readers, passes, writers and `convert`, in any combination | data logic of their own: reimplementing a step, querying or editing world components, reading a format, running a pass past the manager | `cli/tests/architecture.rs` (`entry_points`) |

Known exceptions are listed in `architecture.toml`, each with its reason and what removes it. The test fails on an
exception that no longer occurs, so fixing a violation means deleting its entry; adding one needs a reviewer's
agreement.

## Drift: shapes to refuse in review

Changes drift towards one program's situation in recognisable shapes. Writers check their change against these, and
reviewers refuse them.

- **A reader that bakes, lowers or approximates.** The reader keeps the source's data; a named pass changes it
  (Roles). Checked by the namespaces a reader declares.
- **A flag or boolean added for one target's need.** If the fact is in the source (draw order, what draws over what,
  a surface's material), the reader reads it into a generic core component every target can use. A target-shaped
  switch doesn't land.
- **A hand-kept name list where the data already says it.** Read the field (a sampler state, a shader class, a
  surface id). A name list is acceptable only as a named, documented table from source names to core vocabulary, in
  a pass, counting what it doesn't map.
- **Game or target semantics in core or the toolkit.** Core holds generic facts; game names and target rules live
  in that game's or target's crate. A core change carries evidence from two sources, or a written reason why one is
  enough.
- **A contract a pass may skip.** Counts, per-primitive data (`World::rebuild_primitives`), declarations (the pass
  manager's checks). If a contract can be skipped, make it a compile or run error, not a convention.
- **A home-made oracle.** Compare against the game, a third-party tool or hand-worked fixtures. A second
  implementation of our own isn't evidence. A baseline of our own output detects changes; it doesn't check
  correctness.
- **A program-specific shortcut in the library.** A path, option or code branch that exists for one driver, scene
  or machine stays in that driver: the library is composable by any program.

## Priorities: correct first, then fast

1. Every piece lands with its gates passing: cross-checks against a second decoder or fixtures, the census over a
   game's real data, byte-identical round trips. No gates, no merge.
2. Performance comes after, measured with benches (`criterion`, in the crate's `benches/`). An optimisation merges only
   if every gate still passes **byte for byte**.
3. Never trade correctness for speed: no "fast path" that skips a check, no tolerance widened to make a faster
   version pass.

## Layout and the crate rule

```
core/                 primase-core: world store, core components, TexFormat, frame, pass manager, gates
toolkit/<name>/       primase-<name>: patterns games repeat (texcodec, dds, compress, hash, image, vertex, ...)
worlds/<game>/        primase-<game>: one crate per game or engine (sekiro, gta, fh6, unity, reskate, ...)
formats/<name>/       primase-<name>: file formats that are not one game's (skate-scene, gltf, usd, web, ...)
pipelines/<name>/     primase-<name>: reusable compositions of readers, passes and writers (no data logic)
cli/                  primase: thin compositions of readers + passes + writers
```

**Each new game is its own crate. Patterns that recur across games move into core or the toolkit:** the second time a
game crate needs something another game already has (a codec, a container walk, a vertex decode, a transform
convention, a hash), it moves to the toolkit with a gate and both games call it. A game crate should end up mostly
composition: its format specifics plus calls into the toolkit. Likewise a CLI that needs more than piping is missing a
crate function.

Dependencies point down only: `cli -> pipelines -> worlds/formats -> toolkit -> core`. A world crate depends only on
the toolkit and core: never on a format or another world. A format crate depends on the toolkit, core and
`formats/gltf` (the container other formats embed) only. What two of them share belongs lower. Dev-dependencies may
add format crates (round-trip gates) and `toolkit/testdata`, which is never a normal dependency.

Dialect components (`fromsoft.*`, `gta.*`, `skate.*`, ...) are owned by one crate: the game's world crate, or for
`skate.*` (until `worlds/reskate` exists) `formats/skate-scene`. `pbr.*` lives in core.

### Where code goes

1. **Three layers.** `core` is tiny and slow to change: the world, core components, frames, the pass manager,
   gates. The **toolkit** is many small, independent crates where most reuse lands: codecs, containers, vertex
   layouts, mesh operations, hashes. Game and format crates sit on top. "Should this be shared?" usually means a
   toolkit crate, not core.
2. **Share mechanisms; games supply data.** Prefer one mechanism driven by per-game tables over per-game code: one
   vertex decoder driven by layout descriptors (each game maps its layout format onto them), one `TexFormat` that
   games map their format ids onto, one index + entries + decompression container walker with per-game headers and
   hash functions.
3. **Move it down at the second copy; refine its interface at the third.** No abstraction is designed ahead of two
   real users.
4. **Watch for the opposite failure.** Shared code that grows game flags or `if game == ...` branches is two things
   under one name: split it, or push it back down into the game crates.
5. **Measure it.** For each new game reader, record its game-specific line count against its toolkit calls. A large
   game-specific share points at a missing toolkit piece.

## API conventions

- **Components**: plain data structs implementing `primase_core::Component` with a namespaced, stable `NAME`
  (`core.mesh`, `skate.material`, `fromsoft.layered_material`) and a `VERSION` bumped when the meaning or layout
  changes. No behaviour or I/O handles in components. Entity references are `Entity` fields.
- **Readers** take a source (path, bytes) and attach what they know to a `World`. **Writers** take `&World` and
  declare the components they accept. **Passes** declare what they require, provide, remove and modify.
- **Strict readers**: walk to the exact end of a blob; anything unknown is an error, never silently skipped. A reader
  that must tolerate something reports it (counts) instead of dropping it quietly.
- **Errors**: each crate has its own `Error` enum (`thiserror`) and `Result<T>` alias; bad input is an `Err` with the
  offset/item that broke, never a panic. Panics are for bugs (broken invariants).
- **Determinism**: same inputs, same bytes. No `HashMap`/`HashSet` iteration on any path that reaches output (use
  `BTreeMap`, `IndexMap`, or sort); no thread-timing-dependent order; output names come from content. Parallel code
  must produce the same bytes as serial code (a test proves it).
- **Numbers**: geometry and transforms are f32 where the formats are f32 (byte-identical round trips); compute in f64
  where precision matters and say where results are rounded. Non-finite values are errors, never silently written.
- **Frame**: the world is metres, right-handed, +Y up, -Z north (glTF). Readers convert in, writers convert out, with
  `primase_core::frame`.
- **`unsafe`** is denied workspace-wide. A crate that needs it (loading a game's Oodle DLL) allows it in one module
  with a `// SAFETY:` comment on each block.
- Doc comments say what a thing means and its units/conventions; comments in code say why, not what.

## Gates

- Every decoder has a second, independent decoder or fixtures to check it against (`primase_core::gate::GateReport`).
  The test's doc comment names the oracle.
- **Independent means not ours.** Compare against something we did not write: the game or consumer itself, or an
  existing third-party tool or library (format libraries, the Khronos glTF validator, shader compilers, UnityPy,
  community readers). Where none exists, use small hand-worked fixtures. A second implementation written for this
  project is not independent of the first; do not add one, and when an existing home-made oracle's test is next
  touched, replace it with a third-party check or fixtures.
- Every reader has a census over all of a game's real data of its kind, reporting failures by count.
- Writers round-trip: write, read back, compare.
- Known test values (hash outputs, decoded pixels of a known block) are unit tests.

### What "correct" means

Correct means correct to the **format's spec and to what its consumer reads** (the game, the GPU/D3D, the target
tool), cross-checked by an independent decoder. It does not mean reproducing an existing tool's quirks.

- **Reference.** The spec and the consumer decide. Existing tools (other converters, community libraries, runtimes)
  are oracles for *values*, not for formats: measure and name where they deviate, never copy the deviation.
- **Exact** where the spec defines an exact result: lossless codecs (LZ4, zlib, Oodle), container and header layouts,
  BCn decoding per the D3D spec, binary tables, mesh topology and stored (non-derived) attributes.
- **Equal by meaning, with a stated tolerance,** where a reference tool deviates or a format allows incidental
  variation: JSON compared parsed (same keys and structure, array order kept, numbers at the precision the consumer
  reads: f32 fields as f32, integers exact); PNGs by header, palette, transparency and decoded pixels; BCn conventions
  that differ from the spec (about one grey level, each convention named); derived floats such as normals and
  tangents (a few ulp); compressed byte streams. Every known difference is named and counted in the gate's report.
- **Deterministic.** Our own output stays the same bytes for the same input, for our reproducibility.
- **No complexity for quirks**, unless the quirk has an effect in the consumer. Code that only reproduces another
  program's formatting or arithmetic quirks doesn't belong in the library. When a game or target tool actually
  depends on an old tool's behaviour (a value it reads, a layout it requires), we match it, and the gate says why,
  with the in-game or consumer evidence. When in doubt, the consumer decides (the target tool's own import or an
  in-game check), not the old tool's bytes.

### Fixtures and real data

**No game files in the repository.** Fixtures are synthetic or generated by our own code (or a tiny hand-written
block); oracle outputs committed as fixtures are generated from synthetic inputs. Tests that need real data resolve
it through `primase_testdata::real_data("VAR")`. Code never has a default data path.

Set `PRIMASE_DATA` to the absolute path of a local TOML file containing a flat map of the names below to strings.
An empty `PRIMASE_DATA` is a configuration error; unset it to disable the file.
Copy [primase-data.example.toml](primase-data.example.toml) to an ignored `primase-data.toml` (or keep the private file
outside the checkout). The example contains placeholder paths only. Never commit a populated configuration.

```toml
PRIMASE_SEKIRO_DIR = 'D:/placeholder/games/Sekiro'
PRIMASE_UNITY_DATA = 'D:/placeholder/games/Example/Example_Data'
PRIMASE_UNITY_TPK = 'D:/placeholder/fixtures/typetrees.tpk'
PRIMASE_SCENE_FILTER = 'example-scene'
PRIMASE_ALLOW_LIVE = '0'
```

Resolution is the named environment variable if set (even if empty), then its entry in the `PRIMASE_DATA` file,
then no value. There is no implicit file search. An environment override does not read the file. Partial files are
normal: absent names print `SKIPPED` for path-based gates, including the configured file name, and return `None`.
A name that is set but whose path does not exist fails the gate: a configured input that is gone is never a skip.
With `PRIMASE_REQUIRE_REAL_DATA = "1"` (environment or the `PRIMASE_DATA` file) an unset name fails too; merge gates
for a crate run its real-data tests with it, so a skipped real-data test counts as a failure. A named configuration
file that cannot be read, is invalid TOML, or contains any non-string value is a configuration error that fails the
gate with the requested name and file path. Use TOML literal strings (single quotes) for Windows backslashes.

Values are literal: no shell/environment expansion and no relative-path rewriting. Relative paths use the test
process's working directory, so absolute paths are recommended for both `PRIMASE_DATA` and input/output locations.
The resolver lives in `toolkit/testdata` (`primase-testdata`), which must only appear in consumers'
`[dev-dependencies]`; normal core and production builds have no TOML dependency or feature flag.
Use `primase_testdata::real_data_value("VAR")` for filters, flags and output paths that need not exist; it follows the same precedence
and fails on broken configuration. `try_real_data_value` exposes a structured error to fallible callers. All values,
including flags such as `"1"`, must be strings. The file is read afresh when needed, without mutating the environment.
Ignored private names are `primase-data.toml` and `primase-data.*.toml`; only the placeholder example is tracked.

Variables (the individual environment overrides remain supported):

| variable | kind | what |
|---|---|---|
| `PRIMASE_SCENES_GTA` | input | a folder of GTA V skate-scene folders (every folder under it holding a `scene.json`) |
| `PRIMASE_GTA_DIR` | input | a GTA V install folder (Enhanced or Legacy; the folder holding the executable and the `.rpf` archives) |
| `PRIMASE_GTA_KEYS` | input | a folder of the user's own GTA V archive key material in the community layout (`gtav_ng_key.dat`, `gtav_ng_decrypt_tables.dat`, `gtav_hash_lut.dat`, optional `gtav_aes_key.dat`); checked against published digests, never committed |
| `PRIMASE_GTA_LISTING` | input | optional: an independent reader's listing of every archive of that install (JSON lines; schema in `worlds/gta/tests/census.rs`) |
| `PRIMASE_GTA_ACTIVE` | input | optional: an independent reader's live-file tables of that install (JSON; schema in `worlds/gta/tests/archive_set.rs`) |
| `PRIMASE_GTA_RESOURCES` | input | optional: a folder of an independent reader's decodings of chosen drawables and texture dictionaries of that install (JSON; schema in `worlds/gta/tests/resource_oracle.rs`) |
| `PRIMASE_GTA_PARITY` | input | optional: a folder holding `dump.jsonl`, an independent reader's decoding of every file the gate regions' placements reach (the CodeWalker harness's `batch` mode over the list `parity_list` writes; schema in `worlds/gta/tests/parity.rs`) |
| `PRIMASE_GTA_GTXD` | input | optional: an independent reader's texture dictionary parents of that install with their source files (JSON; schema in `worlds/gta/tests/gtxd.rs`) |
| `PRIMASE_GTA_FILTER` | filter | optional: only archives whose path contains this text |
| `PRIMASE_SCENES_SEKIRO` | input | a folder of Sekiro skate-scene folders (every folder under it holding a `scene.json`) |
| `PRIMASE_SCENE_FILTER` | filter | optional: only scene folders whose path contains this text |
| `PRIMASE_RESKATE_PATCH` | input | a Patch folder written by ReSkate Studio's compile (its `layout.toc`, `Win32` TOCs and CAS archives); `worlds/reskate/tests/postcompile_fb.rs` |
| `PRIMASE_SKATE_DATA` | input | the skate. install's `Data` folder (its `layout.toc`, TOCs and CAS archives), read only |
| `PRIMASE_RESKATE_VBLEND_SPEC` | input | the vertex-blend gate's spec for `PRIMASE_RESKATE_PATCH`: keys to retarget and every rewritten section's expected bytes (JSON; schema in `worlds/reskate/tests/postcompile_vblend.rs`) |
| `PRIMASE_RESKATE_VBLEND_CHECK` | input | optional: an independent checker of the vertex-blend output, run as `python -I CHECK ORIG OUT SPEC DATA [REF]`, last line `PASS` (protocol in `worlds/reskate/tests/postcompile_vblend.rs`) |
| `PRIMASE_RESKATE_VBLEND_REF` | input | optional: the same Patch blended by a reference patcher whose output was checked in game, for the checker's comparison |
| `PRIMASE_RESKATE_BLEND_MAP` | input | a map folder the ReSkate writer built with `vertex_blend` materials (holding `vertex_blend.json`); `worlds/reskate/tests/postcompile_vbrecord.rs` |
| `PRIMASE_RESKATE_BLEND_PATCH` | input | ReSkate Studio's Patch of that map, compiled by a build that carries the vertex-blend weight (validated: 20261012) |
| `PRIMASE_RESKATE_BLEND_SPEC` | input | optional: an independent resolution of that record against that Patch, in the vertex-blend spec format plus `"materials": {name: key}`; with `PRIMASE_RESKATE_VBLEND_CHECK` the checker runs on the stage's output |
| `PRIMASE_UNITY_DATA` | input | a Unity player's data folder (`<Game>_Data`, holding `globalgamemanagers` and the `levelN` / `*.assets` files) |
| `PRIMASE_UNITY_TPK` | input | a type tree package (TPK, uncompressed or LZ4) with the class layouts for that player's Unity version |
| `PRIMASE_UNITY_ORACLE` | input | optional: an independent reader's decoded sample objects of that folder (JSON; schema in `worlds/unity/tests/oracle.rs`) |
| `PRIMASE_UNITY_SCAN` | input | optional: an independent reader's scan of a level of that folder (JSON; schema in `worlds/unity/tests/hierarchy.rs`) |
| `PRIMASE_UNITY_MESHES` | input | optional: an independent reader's decoded meshes of that folder (JSON; schema in `worlds/unity/tests/mesh.rs`) |
| `PRIMASE_UNITY_TEXTURES` | input | optional: an independent reader's decoded textures of that folder (a folder of PNGs and `index.json`; schema in `worlds/unity/tests/texture.rs`) |
| `PRIMASE_UNITY_MATERIALS` | input | optional: an independent reader's materials of that folder (JSON; schema in `worlds/unity/tests/material.rs`) |
| `PRIMASE_UNITY_PORT` | input | optional: another exporter's output folder for that game's `level1` and its materials joined to their objects (JSON; schema in `worlds/unity/tests/lower.rs`) |
| `PRIMASE_UNITY_PORT_MATERIALS` | input | optional: another exporter's output folder for that game's `level1` and its materials joined to their objects (JSON; schema in `worlds/unity/tests/lower.rs`) |
| `PRIMASE_UNITY_PORT_TERRAIN` | input | optional: that exporter's terrain pieces unpacked (raw arrays, `index.json`, its layer means; schema in `worlds/unity/tests/terrain.rs`) |
| `PRIMASE_OUT` | output | optional: where real-data tests write their outputs (default: the OS temporary directory) |
| `PRIMASE_ALLOW_LIVE` | switch | optional: `1` lets a real-data gate report input folders that changed during the run and continue |
| `PRIMASE_MAPS_GTA` | input | a root containing exported map folders with `textures/` PNG images |
| `PRIMASE_SCENE_SKATE` | input | the reference skate-scene validator (`scene-skate.dll`, run with `dotnet`, or its executable) |
| `PRIMASE_GLTF_VALIDATOR` | input | the Khronos glTF-Validator executable (`gltf_validator`, 2.0.0-dev.3.10 release build) |
| `PRIMASE_OODLE5` | input | explicit trusted Oodle version-5 game DLL path |
| `PRIMASE_OODLE6` | input | explicit trusted Oodle version-6 game DLL path |
| `PRIMASE_OODLE9` | input | explicit trusted Oodle version-9 game DLL path |
| `PRIMASE_DCX` | input | folder recursively containing loose KRAK DCX files |
| `PRIMASE_CAS_SAMPLES` | input | directory of indexed individual `.casblock` frames |
| `PRIMASE_SEKIRO_DIR` | input | install folder containing Data1 through Data5 BHD/BDT pairs |
| `PRIMASE_FROMSOFT_RIGID_ORACLE` | input | independent rigid vertex fixtures: models.jsonl and per-model binary mesh/vertex counts followed by position and normal f32 triples; worlds/fromsoft/tests/rigid_census.rs |
| `PRIMASE_FROMSOFT_OBJECT_ORACLE` | input | independently decoded main-HKX body fixtures for reference bd77fcb; worlds/fromsoft/tests/object_collision.rs |
| `PRIMASE_FROMSOFT_OBJECT_SCENE` | input | reference scene folder with scene.json, export_report.json and collision/objects.bin from corrected convex decoding; worlds/fromsoft/tests/object_scene.rs |
| `PRIMASE_FROMSOFT_OUTPUT` | output | fresh output folder for written scene and object-collision gates; worlds/fromsoft/README.md |
| `PRIMASE_FROMSOFT_EXTRAS_ORACLE` | input | independent stored EDGE/BTL records and loose fixtures; worlds/fromsoft/tests/extras_census.rs |
| `PRIMASE_FROMSOFT_EXTRAS_SCENE` | input | reference scene folder with lights/grind tables and markers; worlds/fromsoft/tests/extras_scene.rs |
| `PRIMASE_FROMSOFT_EXTRAS_OUTPUT` | output | fresh extras scene output folder; worlds/fromsoft/tests/extras_scene.rs |
| `PRIMASE_FROMSOFT_NAMES` | input | caller-provided UTF-8 Sekiro path dictionary |
| `PRIMASE_FROMSOFT_NORMAL_SCENE` | input | reference normal PNGs and scene.json for offline cached-DDS comparison; worlds/fromsoft/tests/normal_images.rs |
| `PRIMASE_FROMSOFT_DDS_ORACLE` | input | trusted C# DDS oracle assembly; protocol in worlds/fromsoft/README.md |
| `PRIMASE_FROMSOFT_MODEL_ORACLE` | input | independent pre-bake area geometry outputs; schema in worlds/fromsoft/README.md |
| `PRIMASE_FROMSOFT_MSB_ORACLE` | input | independent map models, parts and transforms; schema in worlds/fromsoft/README.md |
| `PRIMASE_FROMSOFT_SHADOW_ORACLE` | input | original decompressed `map.msb` and independent `shadows.json` (source SHA-256 and ordered part names plus six raw shadow fields); `worlds/fromsoft/tests/shadow_census.rs` |
| `PRIMASE_FROMSOFT_STATE_ORACLE` | input | independent MSB part entity IDs and groups (`entities.jsonl`) and event-script decode (`emevd.jsonl`); schema in worlds/fromsoft/README.md |
| `PRIMASE_FROMSOFT_STATE_SCENE` | input | a scene of one whole block exported with its world states (`extensions.sekiro.world_state`) by an independent implementation; worlds/fromsoft/tests/state_membership.rs |
| `PRIMASE_FROMSOFT_BLOCKER_ORACLE` | input | blocker-card reference cases.json; schema in worlds/fromsoft/README.md |
| `PRIMASE_FROMSOFT_MATERIAL_ORACLE` | input | independent MTD fields, area material assignments and per-model texture availability for object collision; schema in worlds/fromsoft/README.md |
| `PRIMASE_FROMSOFT_PBR_ORACLE` | input | source DDS in `census/dds` plus independent `texture-statistics.json` channel extrema and means; `worlds/fromsoft/tests/pbr_census.rs` |
| `PRIMASE_FROMSOFT_LAYER_PILOT` | input | bounded layered-shader pilot inputs and reference pixels; schema in worlds/fromsoft/README.md |
| `PRIMASE_FROMSOFT_SCENE` | input | reference area skate-scene folder containing placement sources and tables |
| `PRIMASE_FROMSOFT_BHD_ORACLE` | input | reference archive outputs; schema in worlds/fromsoft/README.md |
| `PRIMASE_HAVOK_CORPUS` | input | loose Havok 2016.2 tagfiles, matching compendium, and optional independent `.hkx.soup` oracle files; schema in toolkit/havok/README.md |
| `PRIMASE_FROSTBITE_EBX` | input | a folder of EBX documents as the game wrote them, mirroring their asset paths (searched recursively); every field's value must survive a rewrite, and the byte layout is compared with the reference writer; toolkit/frostbite/tests/ebx.rs |
| `PRIMASE_RESKATE_CLI` | input | the ReSkate CLI executable, used as an independent EBX reader and reference writer (`ebx-values`, `ebx-rewrite-diff`); toolkit/frostbite/tests/ebx.rs |
| `PRIMASE_FROMSOFT_UV_FRAME_CENSUS` | input | independent TSV with normal_texture, area_m2, main_class, main_share and comma-separated class/fraction values; reports rounded area fractions without choosing conversion exceptions |
| `PRIMASE_CODEC_CORPUS` | input | TSV: codec, compressed path, oracle decoded path (tab-separated) |
| `PRIMASE_DXBC_SHADERS` | input | shader binaries, independent disassembly and optional raw-register value fixtures; schema in toolkit/dxbc/README.md |
| `PRIMASE_SKATE_MASK_ORACLE` | input | PNGs and independent mask pixel hashes in `cases.json`; schema below |
| `PRIMASE_DATA` | config | the absolute path of a local TOML file mapping these names to string values (see above) |
| `PRIMASE_REQUIRE_REAL_DATA` | switch | optional: `1` makes every unset input fail its test; `gate` only the inputs the landing gate must have |
| `PRIMASE_CACHE` | runtime | optional: the disk cache folder of KTX2 encodings that `scene_to_tiles` (and a command with the same `--cache` option) uses when no `--cache` is given; read by commands, not tests |
| `PRIMASE_DXC` | input | Microsoft's `dxc` shader compiler executable, for the DXIL evaluator's comparison with WARP; `toolkit/dxil/tests/warp.rs` |
| `PRIMASE_WARP` | input | the WARP oracle program that runs a shader on Microsoft's WARP rasterizer; `toolkit/dxil/tests/common/warp.rs` |
| `PRIMASE_WARP_SCRATCH` | output | a folder for the WARP comparison's case files and results |
| `PRIMASE_DXIL_FUZZ_RUNS` | switch | optional: how many mutated fixtures the DXIL fuzz test runs (default 3000) |
| `PRIMASE_FROMSOFT_FRAMES` | input | a folder holding `frames.jsonl`: SoulsFormats' reading of a few Sekiro models as stored (per mesh its LOD0 face set, per vertex position, normal, tangents and UV0); `worlds/fromsoft/tests/handedness.rs` |
| `PRIMASE_FROMSOFT_NORMAL_FRAMES` | input | a folder holding `frames.jsonl`: independently stored tangent frames (`n`, `t`, `w` per sample key) of Sekiro normal-mapped surfaces; `worlds/fromsoft/tests/normal_basis.rs` |
| `PRIMASE_FROMSOFT_NORMAL_CURL` | input | a JSON list of independent curl measurements of Sekiro normal maps (`name`, `verdict` per map); `worlds/fromsoft/tests/normal_basis.rs` |
| `PRIMASE_FROMSOFT_WRITTEN_SCENE` | input | a scene folder written by the FromSoft reader for the same area as `PRIMASE_FROMSOFT_SCENE`, compared with it; `worlds/fromsoft/tests/collision_scene.rs`, `sekiro_scene.rs` |
| `PRIMASE_GTA_FRAMES` | input | one ydr's high-detail geometries as an independent reader (CodeWalker) reads them, as stored in GTA's frame: one JSON file `{path, value: {models: [{geometries: [{positions, normals, tangents (xyzw), texcoords, indices}]}]}}`; the frame change, triangle fronts and bitangent sign are checked against it (needs `PRIMASE_GTA_DIR`, `PRIMASE_GTA_KEYS`); `worlds/gta/tests/handedness.rs` |
| `PRIMASE_GTA_BOUNDS` | input | props' embedded collision bounds (a ydr's own, a yft's first physics LOD's) as an independent reader (CodeWalker) reads them: a folder of JSON files, one per prop, `{path, leaves: [{type, flags, polygons: [{type, material, vertices (placed)}] or corners (8, placed)}]}`; `lower-gta-embedded-collision`'s triangles are checked against them (needs `PRIMASE_GTA_DIR`, `PRIMASE_GTA_KEYS`); `worlds/gta/tests/embedded_bounds.rs` |
| `PRIMASE_GTA_REGION` | filter | optional: only this named gate region in `worlds/gta/tests/region_pipeline.rs` |
| `PRIMASE_HAVOK_OBJECT_CORPUS` | input | a folder of Havok tagfiles holding object convex bodies, checked against their stored half-spaces; `toolkit/havok/tests/convex_corpus.rs` |
| `PRIMASE_UNITY_SUN` | input | an independent reader's (UnityPy) record of the scan level's RenderSettings sun as stored, the render-pipeline asset classes, the shining directional lights and the main-light choice; `worlds/unity/tests/light.rs` |
| `PRIMASE_UNITY_SHADERS` | input | an independent reader's (UnityPy) shader render state of every material (subshader tags; per pass LightMode, blend factors, zwrite and cull with their bound properties); `worlds/unity/tests/material.rs` |
| `PRIMASE_UNITY_PROGRAMS` | input | an independent reader's (UnityPy) compiled fragment programs of every shader (per pass the player variants, keywords, program type and code hash, and the constant buffers, textures and samplers); `worlds/unity/tests/program.rs` |
| `PRIMASE_UNITY_FRAMES` | input | an independent reader's (UnityPy) decoding of a few meshes in Unity's own frame (vertices, normals, tangents, UV0, submesh triangles); `worlds/unity/tests/handedness.rs` |

Add a row when you add a variable. Test outputs go to a temporary directory, never into the source tree.

`PRIMASE_SKATE_MASK_ORACLE` supplies `cases.json` for the target mask census. Its `cases` array contains
`source` (PNG path relative to the fixture root, or absolute), `source_sha256`, `metallic`, `roughness`,
`occlusion` (explicit packed R binding), `strength`, `width`, `height` and `mask_sha256` (independently decoded
and mapped row-major RGBA8 base pixels). Source images and independent expected values stay outside the repository.

## Dependencies

Small, focused, maintained crates that do one job, each behind our own trait so swapping it is local, each held to the
same gates as our code. Pinned in the root `Cargo.toml` `[workspace.dependencies]` (exact version, default features
off) and used with `workspace = true`. **No umbrella crates** (`image`, `image_dds`, engine frameworks like
`bevy_ecs`). In use or approved: bcdec_rs, intel_tex_2 / texpresso, ddsfile, png, lz4_flex, flate2, binrw, bytemuck,
half, meshopt, pyo3, csbindgen, libloading (a game's own Oodle DLL, loaded at run time from the install, never
bundled), oozextract (only as a gate's second decoder), thiserror, serde/serde_json, indexmap, sha2, aes (AES-128 block operations), num-bigint (raw RSA public operations), encoding_rs (strict Shift-JIS), sha1 (key material checks), quick-xml (pull parser for game XML data files), criterion
(benches), toml (standards-compliant local gate configuration; a handwritten subset would mishandle escaping,
duplicate keys and typed values). Propose anything else in the pull request, with what it does and why our own would be worse.

Licence: GPL-3.0-or-later for the whole workspace; dependencies must be GPL-3-compatible.

## Checks before a merge

Toolchain: pinned in `rust-toolchain.toml` (latest stable). `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets` with no warnings, `cargo test --workspace`. Commit messages say what changed
and how it was verified.
