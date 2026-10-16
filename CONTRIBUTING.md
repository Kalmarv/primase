# Contributing to primase

How the workspace is laid out and the rules every crate follows. The design is `docs/DESIGN.md`.

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
cli/                  primase: thin compositions of readers + passes + writers
```

**Each new game is its own crate. Patterns that recur across games move into core or the toolkit:** the second time a
game crate needs something another game already has (a codec, a container walk, a vertex decode, a transform
convention, a hash), it moves to the toolkit with a gate and both games call it. A game crate should end up mostly
composition: its format specifics plus calls into the toolkit. Likewise a CLI that needs more than piping is missing a
crate function.

Dependencies point down only: `cli -> worlds/formats -> toolkit -> core`. A world or format crate never depends on
another world crate; what two of them share belongs lower.

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
A resolved input path that does not exist also keeps the existing `SKIPPED`/`None` behavior. A named configuration
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

| variable | what |
|---|---|
| `PRIMASE_SCENES_GTA` | a folder of GTA V skate-scene folders (every folder under it holding a `scene.json`) |
| `PRIMASE_GTA_DIR` | a GTA V install folder (Enhanced or Legacy; the folder holding the executable and the `.rpf` archives) |
| `PRIMASE_GTA_KEYS` | a folder of the user's own GTA V archive key material in the community layout (`gtav_ng_key.dat`, `gtav_ng_decrypt_tables.dat`, `gtav_hash_lut.dat`, optional `gtav_aes_key.dat`); checked against published digests, never committed |
| `PRIMASE_GTA_LISTING` | optional: an independent reader's listing of every archive of that install (JSON lines; schema in `worlds/gta/tests/census.rs`) |
| `PRIMASE_GTA_ACTIVE` | optional: an independent reader's live-file tables of that install (JSON; schema in `worlds/gta/tests/archive_set.rs`) |
| `PRIMASE_GTA_FILTER` | optional: only archives whose path contains this text |
| `PRIMASE_SCENES_SEKIRO` | a folder of Sekiro skate-scene folders (every folder under it holding a `scene.json`) |
| `PRIMASE_SCENE_FILTER` | optional: only scene folders whose path contains this text |
| `PRIMASE_RESKATE_NORMAL_REF` | Reference map folders containing normal PNGs; `worlds/reskate/tests/map_gate.rs::real_normal_writer_images`. |
| `PRIMASE_RESKATE_NORMAL_SCENES` | Matching scene folders for the normal-writer image differential; same schema as `PRIMASE_RESKATE_SCENES`. |
| `PRIMASE_RESKATE_REF` | a folder of map folders written by the reference C# map writer, one per area (each holding a `map.json`) |
| `PRIMASE_RESKATE_SCENES` | the skate-scene folders those maps were built from, one per area, named like the map folders |
| `PRIMASE_RESKATE_CAPS` | optional: the per-texture size caps file the reference build used (`--tex-caps`) |
| `PRIMASE_RESKATE_PATCH` | a Patch folder written by ReSkate Studio's compile (its `layout.toc`, `Win32` TOCs and CAS archives); `worlds/reskate/tests/postcompile_fb.rs` |
| `PRIMASE_RESKATE_FB_ORACLE` | an independent reader's summary of that Patch (JSON; schema in `worlds/reskate/tests/postcompile_fb.rs`) |
| `PRIMASE_SKATE_DATA` | the skate. install's `Data` folder (its `layout.toc`, TOCs and CAS archives), read only |
| `PRIMASE_RESKATE_VBLEND_SPEC` | the vertex-blend gate's spec for `PRIMASE_RESKATE_PATCH`: keys to retarget and every rewritten section's expected bytes (JSON; schema in `worlds/reskate/tests/postcompile_vblend.rs`) |
| `PRIMASE_RESKATE_VBLEND_CHECK` | optional: an independent checker of the vertex-blend output, run as `python -I CHECK ORIG OUT SPEC DATA [REF]`, last line `PASS` (protocol in `worlds/reskate/tests/postcompile_vblend.rs`) |
| `PRIMASE_RESKATE_VBLEND_REF` | optional: the same Patch blended by a reference patcher whose output was checked in game, for the checker's comparison |
| `PRIMASE_UNITY_DATA` | a Unity player's data folder (`<Game>_Data`, holding `globalgamemanagers` and the `levelN` / `*.assets` files) |
| `PRIMASE_UNITY_TPK` | a type tree package (TPK, uncompressed or LZ4) with the class layouts for that player's Unity version |
| `PRIMASE_UNITY_ORACLE` | optional: an independent reader's decoded sample objects of that folder (JSON; schema in `worlds/unity/tests/oracle.rs`) |
| `PRIMASE_UNITY_SCAN` | optional: an independent reader's scan of a level of that folder (JSON; schema in `worlds/unity/tests/hierarchy.rs`) |
| `PRIMASE_UNITY_MESHES` | optional: an independent reader's decoded meshes of that folder (JSON; schema in `worlds/unity/tests/mesh.rs`) |
| `PRIMASE_UNITY_TEXTURES` | optional: an independent reader's decoded textures of that folder (a folder of PNGs and `index.json`; schema in `worlds/unity/tests/texture.rs`) |
| `PRIMASE_UNITY_MATERIALS` | optional: an independent reader's materials of that folder (JSON; schema in `worlds/unity/tests/material.rs`) |
| `PRIMASE_UNITY_PORT`, `PRIMASE_UNITY_PORT_MATERIALS` | optional: another exporter's output folder for that game's `level1` and its materials joined to their objects (JSON; schema in `worlds/unity/tests/lower.rs`) |
| `PRIMASE_UNITY_PORT_TERRAIN` | optional: that exporter's terrain pieces unpacked (raw arrays, `index.json`, its layer means; schema in `worlds/unity/tests/terrain.rs`) |
| `PRIMASE_OUT` | optional: where real-data tests write their outputs (default: the OS temporary directory) |
| `PRIMASE_ALLOW_LIVE` | optional: `1` lets a real-data gate report input folders that changed during the run and continue |
| `PRIMASE_MAPS_GTA` | a root containing exported map folders with `textures/` PNG images |
| `PRIMASE_MESH_CASES` | independent mesh-operation oracle bundle (binary contract in toolkit/meshops/README.md) |
| `PRIMASE_SCENE_SKATE` | the reference skate-scene validator (`scene-skate.dll`, run with `dotnet`, or its executable) |
| `PRIMASE_GLTF_VALIDATOR` | the Khronos glTF-Validator executable (`gltf_validator`, 2.0.0-dev.3.10 release build) |
| `PRIMASE_OODLE5` | explicit trusted Oodle version-5 game DLL path |
| `PRIMASE_OODLE6` | explicit trusted Oodle version-6 game DLL path |
| `PRIMASE_OODLE9` | explicit trusted Oodle version-9 game DLL path |
| `PRIMASE_DCX` | folder recursively containing loose KRAK DCX files |
| `PRIMASE_CAS_SAMPLES` | directory of indexed individual `.casblock` frames |
| `PRIMASE_SEKIRO_DIR` | install folder containing Data1 through Data5 BHD/BDT pairs |
| `PRIMASE_FROMSOFT_RIGID_ORACLE` | independent rigid vertex fixtures: models.jsonl and per-model binary mesh/vertex counts followed by position and normal f32 triples; worlds/fromsoft/tests/rigid_census.rs |
| `PRIMASE_FROMSOFT_OBJECT_ORACLE` | independently decoded main-HKX body fixtures for reference bd77fcb; worlds/fromsoft/tests/object_collision.rs |
| `PRIMASE_FROMSOFT_OBJECT_SCENE` | reference scene folder with scene.json, export_report.json and collision/objects.bin from corrected convex decoding; worlds/fromsoft/tests/object_scene.rs |
| `PRIMASE_FROMSOFT_OUTPUT` | fresh output folder for written scene and object-collision gates; worlds/fromsoft/README.md |
| `PRIMASE_FROMSOFT_EXTRAS_ORACLE` | independent stored EDGE/BTL records and loose fixtures; worlds/fromsoft/tests/extras_census.rs |
| `PRIMASE_FROMSOFT_EXTRAS_SCENE` | reference scene folder with lights/grind tables and markers; worlds/fromsoft/tests/extras_scene.rs |
| `PRIMASE_FROMSOFT_EXTRAS_OUTPUT` | fresh extras scene output folder; worlds/fromsoft/tests/extras_scene.rs |
| `PRIMASE_FROMSOFT_NAMES` | caller-provided UTF-8 Sekiro path dictionary |
| `PRIMASE_FROMSOFT_NORMAL_SCENE` | reference normal PNGs and scene.json for offline cached-DDS comparison; worlds/fromsoft/tests/normal_images.rs |
| `PRIMASE_FROMSOFT_DDS_ORACLE` | trusted C# DDS oracle assembly; protocol in worlds/fromsoft/README.md |
| `PRIMASE_FROMSOFT_MODEL_ORACLE` | independent pre-bake area geometry outputs; schema in worlds/fromsoft/README.md |
| `PRIMASE_FROMSOFT_MSB_ORACLE` | independent map models, parts and transforms; schema in worlds/fromsoft/README.md |
| `PRIMASE_FROMSOFT_STATE_ORACLE` | independent MSB part entity IDs and groups (`entities.jsonl`) and event-script decode (`emevd.jsonl`); schema in worlds/fromsoft/README.md |
| `PRIMASE_FROMSOFT_STATE_SCENE` | a scene of one whole block exported with its world states (`extensions.sekiro.world_state`) by an independent implementation; worlds/fromsoft/tests/state_membership.rs |
| `PRIMASE_FROMSOFT_BLOCKER_ORACLE` | blocker-card reference cases.json; schema in worlds/fromsoft/README.md |
| `PRIMASE_FROMSOFT_MATERIAL_ORACLE` | independent MTD fields, area material assignments and per-model texture availability for object collision; schema in worlds/fromsoft/README.md |
| `PRIMASE_FROMSOFT_PBR_ORACLE` | source DDS in `census/dds` plus independent `texture-statistics.json` channel extrema and means; `worlds/fromsoft/tests/pbr_census.rs` |
| `PRIMASE_FROMSOFT_LAYER_PILOT` | bounded layered-shader pilot inputs and reference pixels; schema in worlds/fromsoft/README.md |
| `PRIMASE_FROMSOFT_SCENE` | reference area skate-scene folder containing placement sources and tables |
| `PRIMASE_FROMSOFT_BHD_ORACLE` | reference archive outputs; schema in worlds/fromsoft/README.md |
| `PRIMASE_HAVOK_CORPUS` | loose Havok 2016.2 tagfiles, matching compendium, and optional independent `.hkx.soup` oracle files; schema in toolkit/havok/README.md |
| `PRIMASE_FROMSOFT_UV_FRAME_CENSUS` | independent TSV with normal_texture, area_m2, main_class, main_share and comma-separated class/fraction values; reports rounded area fractions without choosing conversion exceptions |
| `PRIMASE_CODEC_CORPUS` | TSV: codec, compressed path, oracle decoded path (tab-separated) |
| `PRIMASE_DXBC_SHADERS` | shader binaries, independent disassembly and optional raw-register value fixtures; schema in toolkit/dxbc/README.md |

Add a row when you add a variable. Test outputs go to a temporary directory, never into the source tree.

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
