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
block); oracle outputs committed as fixtures are generated from synthetic inputs. Tests that need real data read it
from paths in environment variables and are **skipped** (pass, printing `SKIPPED`) when the variable is unset, via
`primase_core::gate::real_data("VAR")`. Code never has a default path. Variables:

| variable | what |
|---|---|
| `PRIMASE_SCENES_GTA` | a folder of GTA V skate-scene folders (every folder under it holding a `scene.json`) |
| `PRIMASE_SCENES_SEKIRO` | a folder of Sekiro skate-scene folders (every folder under it holding a `scene.json`) |
| `PRIMASE_SCENE_FILTER` | optional: only scene folders whose path contains this text |
| `PRIMASE_RESKATE_REF` | a folder of map folders written by the reference C# map writer, one per area (each holding a `map.json`) |
| `PRIMASE_RESKATE_SCENES` | the skate-scene folders those maps were built from, one per area, named like the map folders |
| `PRIMASE_RESKATE_CAPS` | optional: the per-texture size caps file the reference build used (`--tex-caps`) |
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
| `PRIMASE_FROMSOFT_NAMES` | caller-provided UTF-8 Sekiro path dictionary |
| `PRIMASE_FROMSOFT_DDS_ORACLE` | trusted C# DDS oracle assembly; protocol in worlds/fromsoft/README.md |
| `PRIMASE_FROMSOFT_MODEL_ORACLE` | independent pre-bake area geometry outputs; schema in worlds/fromsoft/README.md |
| `PRIMASE_FROMSOFT_MSB_ORACLE` | independent map models, parts and transforms; schema in worlds/fromsoft/README.md |
| `PRIMASE_FROMSOFT_SCENE` | reference area skate-scene folder containing placement sources and tables |
| `PRIMASE_FROMSOFT_BHD_ORACLE` | reference archive outputs; schema in worlds/fromsoft/README.md |
| `PRIMASE_CODEC_CORPUS` | TSV: codec, compressed path, oracle decoded path (tab-separated) |

Add a row when you add a variable. Test outputs go to a temporary directory, never into the source tree.

## Dependencies

Small, focused, maintained crates that do one job, each behind our own trait so swapping it is local, each held to the
same gates as our code. Pinned in the root `Cargo.toml` `[workspace.dependencies]` (exact version, default features
off) and used with `workspace = true`. **No umbrella crates** (`image`, `image_dds`, engine frameworks like
`bevy_ecs`). In use or approved: bcdec_rs, intel_tex_2 / texpresso, ddsfile, png, lz4_flex, flate2, binrw, bytemuck,
half, meshopt, pyo3, csbindgen, libloading (a game's own Oodle DLL, loaded at run time from the install, never
bundled), oozextract (only as a gate's second decoder), thiserror, serde/serde_json, indexmap, sha2, aes (AES-128 block operations), num-bigint (raw RSA public operations), criterion
(benches). Propose anything else in the pull request, with what it does and why our own would be worse.

Licence: GPL-3.0-or-later for the whole workspace; dependencies must be GPL-3-compatible.

## Checks before a merge

Toolchain: pinned in `rust-toolchain.toml` (latest stable). `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets` with no warnings, `cargo test --workspace`. Commit messages say what changed
and how it was verified.
