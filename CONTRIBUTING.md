# Contributing to primase

What primase is, the role of every piece, and the rules every crate follows. This page comes first; where any other
document, rule, `architecture.toml` entry or older decision disagrees with it, this page wins. The design notes are
`docs/DESIGN.md`.

## The model

primase is a library. Readers bring a game's data in, passes transform it, writers write it out, and a **program**
composes them and makes the choices.

- **Readers** read supported source information faithfully, into shared core pieces or clearly namespaced game data
  (`gta.*`, `fromsoft.*`). They don't bake and don't choose destination policy. Coverage grows over time: readers
  read what they can read. They keep the supported meaning, not necessarily the original encoded bytes; an old
  compressor's quirks are not reproduced to match its output.
- **Core and toolkit.** Core holds shared meanings and representations; toolkit holds reusable mechanisms (a codec
  used by several games is a toolkit crate, because it isn't one game's). New readers and writers should widen the
  useful combinations without a bespoke integration for every pair.
- **Passes** transform data, and the program selects them. Shared operations handle texture and geometry work.
  Destination passes prepare data for one writer: ReSkate's preparation and its prepared data live in the ReSkate
  crate next to its writer, not in core, and run before writing. Choices such as quality, region, spawn or whether to
  add a floor are explicit settings or parameters of the passes a program picks.
- **Programs** live in the consumer's repository (a game's conversion program), import primase and compose its
  operations, through the chunked conversion helper (`convert`) or directly. They own the recipe and the choices, and
  may hold custom steps; useful pieces move into primase later. primase has no library tier for complete recipes.
- **Writers** write the prepared core and namespaced data they understand, whichever reader it came from. They write
  what they can write: an unsupported feature is omitted and reported, not a failure. They never adapt: baking,
  resizing, generating collision or moving placements is a pass. Encoding prepared data in their format is writing
  (vertex packing, file-sized splits, index layout, texture encoding). Invalid data that prevents valid output is an
  error; an unsupported feature is not.
- **Further stages** a program runs after writing (a Studio compile, stitching) stay visible stages; they don't
  justify hidden adaptation in a writer.
- **When things move into core.** A standard, broadly useful concept can enter core with one current reader. More
  readers or writers needing the same meaning is another reason. Shared meanings go to core, mechanisms to toolkit,
  reusable destination preparation to destination passes.
- **Reports** return what the program needs to see: known coverage limits, approximations, deliberate selections and
  exclusions, unsupported output and failures, with each part's approximation details kept. The program decides how to
  show or act on them. Nothing implies that omitted data was written or that an approximation was exact. A report
  can't promise to find source features a reader doesn't recognise.
- **Verification** tests the behaviour a change affects, widening when shared-code impact is uncertain. Missing inputs
  are reported; not every decoder or game conversion runs for every change.

Clarifications:

- **Choices.** Choosing an operation ("resize this texture") is the program's. Choosing a sensible implementation of it
  (filter, compression effort, precision) is the library's, with documented defaults. Decisions that materially change
  what is produced (dropping geometry, lowering resolution, picking a world state) are explicit.
- **Writing.** Writers may encode and arrange prepared data their own way. Byte differences from an old tool are not a
  loss; keeping the meaning still needs checking. Changes to content, quality, appearance or gameplay are passes.
- **Missing support.** Visible reporting is enough for the program to react (a map with no collision, say). There is no
  mandatory requirements framework.
- **Chunking.** Deciding how to divide content (regions, tiles, output groups) is a pass's job; loading and processing
  the pieces incrementally is the reader's and the conversion machinery's. The world can describe objects whose heavy
  data isn't loaded yet. Operations that need neighbouring geometry or world-wide information get it rather than treat
  a chunk as an isolated world, and processing in chunks preserves the intended result.

Also always: the repository holds no game files and nothing about anyone's machine; output is deterministic;
dependencies are small and pinned; correctness comes from independent sources (the game, a third-party tool, a
hand-worked fixture), never from a second implementation of ours; make it work, then correct, then fast.

## Roles

Every piece of code has one of these roles. Each rule is checked by the normal gates (`cargo clippy ... -D warnings`,
`cargo test --workspace`) where a check can see it; a lexical check is a guardrail, not proof.

| role | may | may not | checked by |
|---|---|---|---|
| **reader** | read the source it is given; attach core components and its own namespace (`gta.*`, `fromsoft.*`); keep the source as authored (bone-space geometry with its skeleton and pose, every level of detail, every state); report what it tolerated or can't read; load the heavy data (meshes, textures) of the parts it is asked for | lower, bake, approximate, apply poses, pick levels of detail or states, select what a destination draws, shape for a destination (another namespace's components, destination units), read options from the environment | I/O only in modules that opt in as `reader I/O` (clippy); the namespaces a reader declares, checked by `primase convert` for registered readers |
| **pass** | transform the world it is given: lower, bake, approximate, select, split, merge, prepare for a destination; declare what it requires, provides, removes and modifies; report every approximation and exclusion by name | touch files, the environment or processes; run outside the pass manager | clippy's file/environment ban; every pass registered by name and every public lowering run by a pass (`cli/tests/architecture.rs`) |
| **writer** | encode the prepared data its format supports, laying it out as its encoding needs (files, tiles, splits by encoded size) without changing what it means; omit and report what it doesn't support; refuse invalid prepared data with the reason | lower, approximate, resize, select, merge, generate or move content (a pass does that first), read other files, change the world | writer entry points take `&World` (`cli/tests/architecture.rs`); I/O only in modules that opt in as `writer I/O` |
| **core** | shared meanings, the store, frames, the pass machinery and its generic passes, gates | game or destination names, renderer tuning, file access, dependencies on other crates | crate dependencies, game names, clippy's file ban (`cli/tests/architecture.rs`) |
| **toolkit** | shared mechanisms that games supply data to | game branches, depending on formats or worlds | crate dependencies, game names |
| **formats** | file formats that are not one game's: their readers, writers and passes | depending on worlds or on formats other than `formats/gltf` | crate dependencies, game names |
| **test-input registry** (`toolkit/testdata`) | name and describe the real-data inputs tests read, games and third-party tools included; resolve them | game logic, depending on workspace crates, being a normal dependency of any crate | crate dependencies (not excusable in `architecture.toml`); `real_data_variables_are_declared` |
| **cli, examples** | compose readers, passes, writers and `convert` for a person trying the library | data logic of their own (reimplementing a step, querying or editing world components, reading a format, running a pass past the manager); hiding a recipe's choices | `cli/tests/architecture.rs` (`entry_points`) |

Programs are not in this repository: they live with their consumers and hold the recipe, the settings, custom steps and
the stages after writing.

Readers read, writers write, and what breaks a rule today is listed in `architecture.toml` as a break to remove,
each with its reason and what removes it (`removed_when`). The test fails on a violation not listed, on a break that
no longer occurs (delete its entry), and on any entry it hasn't seen before (`breaks_only_go`, by identity): the list
only goes down, and nothing is added to make a check pass. Disk access is a rule by crate role, not a list: reader and
writer I/O in `formats` and `worlds`, toolkit I/O in the toolkit, test I/O only in test code, each module opting in
with its role (`allow(clippy::disallowed_*, reason = "<role>: ...")`).

## Drift: shapes to refuse in review

- **A reader that bakes, lowers, selects or approximates.** The reader keeps the source's data; a pass the program
  selects changes it.
- **A writer that adapts.** Resizing, budgets, floors, lifts, collision generation, a fallback spawn: each is a pass
  the program selects, not a side effect of writing.
- **A default that is a choice.** A reader or library operation that silently picks a quality, region, LOD, state or
  fallback for its caller. The program states it; the library documents implementation defaults (filters, effort)
  only.
- **A flag or boolean added for one destination's need.** If the fact is in the source (draw order, what draws over
  what, a surface's material), the reader reads it into a shared core meaning every destination can use.
- **A hand-kept name list where the data already says it.** Read the field (a sampler state, a shader class, a
  surface id). A name list is acceptable only as a named, documented table from source names to shared vocabulary,
  in the game's reader or passes, reporting what it doesn't map.
- **Game or destination semantics in core or the toolkit.** Core holds shared meanings; a game's meaning lives in its
  reader and passes, a destination's preparation in the destination's crate.
- **A contract a pass may skip.** Reports, per-primitive data (`World::rebuild_primitives`), declarations (the pass
  manager's checks). If a contract can be skipped, make it a compile or run error, not a convention.
- **A home-made oracle.** Compare against the game, a third-party tool or hand-worked fixtures. A second
  implementation of our own isn't evidence. A baseline of our own output detects change; it doesn't check
  correctness.
- **A lost report.** An approximation, exclusion or omission that a pass or writer knew about and the program can't
  see.

## Priorities: work, then correct, then fast

1. Every piece lands with the checks its change affects passing: independent cross-checks or hand-worked fixtures for
   what it reads or writes, compared by meaning.
2. Performance comes after, measured with benches (`criterion`, in the crate's `benches/`). An optimisation lands only
   if the meaning of every affected output is unchanged.
3. Never trade correctness for speed: no "fast path" that skips a check, no tolerance widened to make a faster
   version pass.

## Layout

```
core/                 primase-core: world store, core components, TexFormat, frame, pass manager, conversion helper
toolkit/<name>/       primase-<name>: shared mechanisms (texcodec, dds, compress, glb, hash, image, meshops, ...)
worlds/<name>/        primase-<name>: one crate per game (or game family), engine family or destination (fromsoft,
                      gta, unity, frostbite, monogame, reskate, ...): its reader, its passes, and for a destination its
                      preparation passes and writer
formats/<name>/       primase-<name>: file formats that are not one game's or destination's (gltf, web, ...)
cli/                  primase: a small command line over the library, for trying it; not where recipes live
```

Dependencies point down only: `cli -> worlds/formats -> toolkit -> core`. A world crate depends on the toolkit and
core, never on a format. Each says what it is in its manifest (`[package.metadata.primase] kind`): a `game` crate (a
game or a game family) or a `destination` crate may depend on its engine family's `engine` crate (`engine = "<package>"`),
never the other way round, and an engine crate never depends on a game, a destination or a recipe; no other world
depends on another. A format crate depends on the toolkit, core and `formats/gltf` (the scene writer other formats
embed; the GLB codec itself is `toolkit/glb`) only. What two of them share belongs lower. Dev-dependencies may add format crates (round-trip gates) and
`toolkit/testdata`, which is never a normal dependency.

Names are owned: core, toolkit and formats name no game, engine or destination; a world crate uses only its own
(`[package.metadata.primase] names`): a game crate its games and engine, an engine crate its engine, a destination
crate its destination and engine. A source crate naming a destination, or any crate naming another game, is a break.
Vendor formats used across engines (Havok, DXBC, DXIL) are toolkit mechanisms.

Namespaced components (`fromsoft.*`, `gta.*`, `skate.*`, ...) are owned by one crate: the game's or destination's
crate (`skate.*`: `worlds/reskate`, with the skate-scene interchange it carries). `pbr.*` lives in core.

### Worlds crates: one role vocabulary

Every worlds crate is arranged by the same roles, so no world drifts to a layout of its own; a crate has only the
roles it has (no empty folders):

```
src/read/          the reader: decoding the source, the convert::Reader adapter and its load step
src/passes/        the passes: selections, lowerings, preparation (no I/O; they take the world and their settings)
src/write/         the writer and its encoding: it writes the prepared data it is given and reopens no source
src/components/    the World data the crate owns (its namespaced components and resources)
src/<stage>/       a later stage with its own artifacts, in its own folder (reskate's postcompile/), declared in the
                   manifest: [package.metadata.primase] stages = ["postcompile"]
src/<format>/      a named format module for records, tables and byte codecs that more than one role uses (reskate's
                   interchange/ schema, with read/interchange and write/interchange as its I/O entry points)
```

- **Several games in one crate** use game-first subtrees with the same names: shared engine work at the crate's roles,
  each game's under `<game>/read`, `<game>/passes`, `<game>/components` (fromsoft: `sekiro/`, later `elden/`). One
  game isn't split between both conventions, and a game folder isn't added merely because several games use an engine.
- **Format records aren't components**: an encoded record and a World component are different representations. No
  catch-all `shared/`, no duplicate structs for symmetry, no splitting a tiny pure codec into three files.
- **A mixed module is split by responsibility**, not moved whole (query helpers apart from encoding, reading apart from
  lowering).
- **Public entry points stay**: a crate's useful public paths are kept through explicit re-exports when it moves, so
  consumers don't churn; internals aren't made public to reach siblings.

Loaded prepared data is authoritative: a writer never prefers a source file over what it was given, and keeping the
original encoded bytes for an unchanged pass-through is the reader's job, with an explicit unchanged relationship.

`architecture.rs` (`worlds_layout`) is a guardrail, not a proof that readers read and writers write; the behavioural
tests are what show that. It checks:

- reader I/O only under `read/`, writer I/O only under `write/`, a stage's under its own folder;
- an I/O allowance covers no module beneath it (none on a `mod` declaration, no inner one in a file that declares
  modules), and a module's place is its file (no `#[path]`, no inline role modules);
- passes, components and the other modules (format modules, pure codecs, helpers) name no I/O path but their own: no
  module that opts in to I/O or is a `read`/`write`/stage module, and no re-export of one, however the path is written
  (`use` trees, `crate`/`super`/`self`, aliases). A helper that reached I/O would be refused itself, so a pass can't
  reach I/O through it. The crate root may re-export for consumers.

It works by name: a function value, a trait object or a macro that reaches I/O is not seen. A module not yet arranged
carries a migration status in `architecture.toml` (`[[layout]]`, one per module, counted like a break): a new
violating module fails, and a listed one that follows the layout must be deleted, so the list only shrinks. Crates move
one at a time: ReSkate first, before step 20; then GTA, Unity, Frostbite and FromSoft, each before the final
review.

### Where code goes

1. **A game's meaning** (what its data is, its shader classes, which of its records are ledges or spawns) goes to its
   reader, or to its passes when turning it into shared core data is a choice or an approximation.
2. **A shared meaning** goes to core. A standard, broadly useful concept can enter core with one current reader; a
   one-off convenience doesn't.
3. **A mechanism** (codec, container walk, vertex decode, mesh or image operation, hash) goes to the toolkit, so games
   and destinations supply data to one mechanism instead of each growing their own. Code moves down when a second
   copy appears; don't design an abstraction ahead of real use.
4. **Destination preparation** goes to the destination's crate as passes, next to its writer.
5. **A recipe and its choices** go to the program, in its consumer's repository.
6. **Watch for the opposite failure.** Shared code that grows game flags or `if game == ...` branches is two things
   under one name: split it, or push it back down.

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
- **Numbers**: geometry and transforms are f32 where the formats are f32 (lossless round trips); compute in f64
  where precision matters and say where results are rounded. Non-finite values are errors, never silently written.
- **Frame**: the world is metres, right-handed, +Y up, -Z north (glTF). Readers convert in, writers convert out, with
  `primase_core::frame`.
- **`unsafe`** is denied workspace-wide. A crate that needs it (loading a game's Oodle DLL) allows it in one module
  with a `// SAFETY:` comment on each block.
- Doc comments say what a thing means and its units/conventions; comments in code say why, not what.

## Gates

- Test what the change affects. A decoder is checked against an independent decoder or fixtures
  (`primase_core::gate::GateReport`); the test's doc comment names the oracle.
- **Independent means not ours.** Compare against something we did not write: the game or consumer itself, or an
  existing third-party tool or library (format libraries, the Khronos glTF validator, shader compilers, UnityPy,
  community readers). Where none exists, use small hand-worked fixtures. A second implementation written for this
  project is not independent of the first; do not add one, and when an existing home-made oracle's test is next
  touched, replace it with a third-party check or fixtures.
- A census over a game's real data is a tool for a reader change that needs it, not a default for every change.
- Writers round-trip where they can: write, read back, compare by meaning; and their tests show what they omit is
  reported.
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
- **Deterministic.** Our own output stays the same bytes for the same input, for our reproducibility. Matching an
  old tool's bytes is not a goal.
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
With `PRIMASE_REQUIRE_REAL_DATA = "1"` (environment or the `PRIMASE_DATA` file) an unset name fails too: use it when
running the real-data tests a change affects, so a skipped affected test counts as a failure, not a pass. A named configuration
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

Variables (the individual environment overrides remain supported). `status`: `live`, the obligation stands and the
input it names is acceptable; `blocked`, the obligation stands but no acceptable input exists yet (the existing data
is not independent of what it checks, or its producer is gone), so its tests fail when required and never pass by
skipping; `retired`, its owner retired the obligation, it is never a landing-gate input, and the tests still reading
it drop it in their owner's step. `source` says where the data comes from (the game itself, a third-party reader and
its version, another exporter, our own earlier output) and so how far it is independent: a regression baseline of
our own output is labelled as one, never as an oracle. A name called independent must be produced by something other
than the code it checks.

| variable | kind | status | what | source |
|---|---|---|---|---|
| `PRIMASE_SCENES_GTA` | input | retired | a folder of GTA V skate-scene folders (every folder under it holding a `scene.json`) | an earlier exporter's scenes, since deleted; retired by its owners (no replacement wanted): its census readers drop it in their own steps |
| `PRIMASE_GTA_DIR` | input | live | a GTA V install folder (Enhanced or Legacy; the folder holding the executable and the `.rpf` archives) | the user's own install |
| `PRIMASE_GTA_KEYS` | input | live | a folder of the user's own GTA V archive key material in the community layout (`gtav_ng_key.dat`, `gtav_ng_decrypt_tables.dat`, `gtav_hash_lut.dat`, optional `gtav_aes_key.dat`); checked against published digests, never committed | the user's own key material, checked against published digests |
| `PRIMASE_GTA_LISTING` | input | live | optional: an independent reader's listing of every archive of that install (JSON lines; schema in `worlds/gta/tests/census.rs`) | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `PRIMASE_GTA_ACTIVE` | input | live | optional: an independent reader's live-file tables of that install (JSON; schema in `worlds/gta/tests/archive_set.rs`) | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `PRIMASE_GTA_RESOURCES` | input | live | optional: a folder of an independent reader's decodings of chosen drawables and texture dictionaries of that install (JSON; schema in `worlds/gta/tests/resource_oracle.rs`) | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `PRIMASE_GTA_PARITY` | input | live | optional: a folder holding `dump.jsonl`, an independent reader's decoding of every file the gate regions' placements reach (the CodeWalker harness's `batch` mode over the list `parity_list` writes; schema in `worlds/gta/tests/parity.rs`) | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools (`batch` mode, written when needed) |
| `PRIMASE_GTA_GTXD` | input | live | optional: an independent reader's texture dictionary parents of that install with their source files (JSON; schema in `worlds/gta/tests/gtxd.rs`) | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `PRIMASE_GTA_FILTER` | filter | live | optional: only archives whose path contains this text | - |
| `PRIMASE_SCENES_SEKIRO` | input | retired | a folder of Sekiro skate-scene folders (every folder under it holding a `scene.json`) | an earlier C# exporter's scenes, since deleted; retired by its owners: its census readers drop it in their own steps |
| `PRIMASE_SCENE_FILTER` | filter | live | optional: only scene folders whose path contains this text | - |
| `PRIMASE_RESKATE_PATCH` | input | live | a Patch folder written by ReSkate Studio's compile (its `layout.toc`, `Win32` TOCs and CAS archives); `worlds/reskate/tests/postcompile_fb.rs` | third-party: ReSkate Studio's compile output |
| `PRIMASE_SKATE_DATA` | input | live | the skate. install's `Data` folder (its `layout.toc`, TOCs and CAS archives), read only | the user's own install |
| `PRIMASE_RESKATE_BLEND_MAP` | input | live | a map folder the ReSkate writer built with `vertex_blend` materials (holding `vertex_blend.json`); `worlds/reskate/tests/postcompile_vbrecord.rs` | our own writer's output: the stage's input, not a reference |
| `PRIMASE_RESKATE_BLEND_PATCH` | input | live | ReSkate Studio's Patch of that map, compiled by a build that carries the vertex-blend weight (validated: 20261012) | third-party: ReSkate Studio 20261012's compile of that map |
| `PRIMASE_RESKATE_BLEND_SPEC` | input | live | optional: a frozen resolution of that record against that Patch (a regression baseline of our own stage), `{"materials": {name: key}}` | regression baseline: an earlier run of our own stage, frozen; not an independent oracle |
| `PRIMASE_UNITY_V21_DATA` | input | live | a Unity player data folder with SerializedFile version 21 containers | the user's own install |
| `PRIMASE_UNITY_V21_CENSUS` | input | live | an independent UnityPy object-table census of that folder (JSON; schema in `worlds/unity/tests/serialized_census.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `PRIMASE_UNITY_DATA` | input | live | a Unity player's data folder (`<Game>_Data`, holding `globalgamemanagers` and the `levelN` / `*.assets` files) | the user's own install |
| `PRIMASE_UNITY_TPK` | input | live | a type tree package (TPK, uncompressed or LZ4) with the class layouts for that player's Unity version | input data: class layouts for the player's Unity version (repacked with a script kept with the private oracle tools) |
| `PRIMASE_UNITY_ORACLE` | input | live | optional: an independent reader's decoded sample objects of that folder (JSON; schema in `worlds/unity/tests/oracle.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `PRIMASE_SKATEBIRD_DATA` | input | live | SkateBIRD's data folder (`SkateBIRD_Data`, a Unity 2019.4.20f1 player), read with `PRIMASE_UNITY_TPK`; worlds/skatebird/tests/markers.rs | the user's own install |
| `PRIMASE_STUNTBOOST_DIR` | input | live | STUNTBOOST's install folder (Steam buildid 25832744: `Content/Models/Export` levels and their derived `Collision`/`Cuts` files); worlds/stuntboost/tests/oracle.rs | the user's own install |
| `PRIMASE_STUNTBOOST_ORACLE` | input | live | the game's own parsers run over that install: per level part `<L>_<N>.json` (node names as its `ParsedName` reads them; its collision model as `SE.CollisionModel` loads it: meshes, grippable bits, ramps, ledges) and `<L>_<N>.tris.bin` (every collision triangle); worlds/stuntboost/tests/oracle.rs | independent: the game's own code via reflection, the oracle tool kept with the game's private tools |
| `PRIMASE_UNITY_SCAN` | input | live | optional: an independent reader's scan of a level of that folder (JSON; schema in `worlds/unity/tests/hierarchy.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `PRIMASE_UNITY_MESHES` | input | live | optional: an independent reader's decoded meshes of that folder (JSON; schema in `worlds/unity/tests/mesh.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `PRIMASE_UNITY_TEXTURES` | input | live | optional: an independent reader's decoded textures of that folder (a folder of PNGs and `index.json`; schema in `worlds/unity/tests/texture.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `PRIMASE_UNITY_MATERIALS` | input | live | optional: an independent reader's materials of that folder (JSON; schema in `worlds/unity/tests/material.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `PRIMASE_UNITY_PORT` | input | live | optional: another exporter's output folder for that game's `level1` and its materials joined to their objects (JSON; schema in `worlds/unity/tests/lower.rs`) | another exporter's output: an appearance baseline, not a reading of the source |
| `PRIMASE_UNITY_PORT_MATERIALS` | input | live | optional: another exporter's output folder for that game's `level1` and its materials joined to their objects (JSON; schema in `worlds/unity/tests/lower.rs`) | another exporter's output, joined by a script kept with the private oracle tools |
| `PRIMASE_UNITY_PORT_TERRAIN` | input | live | optional: that exporter's terrain pieces unpacked (raw arrays, `index.json`, its layer means; schema in `worlds/unity/tests/terrain.rs`) | another exporter's terrain, unpacked by a script kept with the private oracle tools |
| `PRIMASE_OUT` | output | live | optional: where real-data tests write their outputs (default: the OS temporary directory) | - |
| `PRIMASE_ALLOW_LIVE` | switch | live | optional: `1` lets a real-data gate report input folders that changed during the run and continue | - |
| `PRIMASE_MAPS_GTA` | input | retired | a root containing exported map folders with `textures/` PNG images | an earlier exporter's map folders; retired by its owners: `toolkit/image/tests/census.rs` drops it in its own step |
| `PRIMASE_SCENE_SKATE` | input | live | our own skate-scene validator built from its last source (`scene-skate.dll`, run with `dotnet`, or its executable); not an independent oracle | our own C# validator, rebuilt from its last source (commit d78228a; the earlier build predated its byte-range and level-of-detail checks): it refuses every invalid case of the interchange tests for the same reason and accepts the fixture and our output; a second implementation of the same rules, not an independent oracle |
| `PRIMASE_GLTF_VALIDATOR` | input | live | the Khronos glTF-Validator executable (`gltf_validator`, 2.0.0-dev.3.10 release build) | third-party: Khronos glTF-Validator 2.0.0-dev.3.10 |
| `PRIMASE_OODLE5` | input | live | explicit trusted Oodle version-5 game DLL path | the game's own DLL from the user's install |
| `PRIMASE_OODLE6` | input | live | explicit trusted Oodle version-6 game DLL path | the game's own DLL from the user's install |
| `PRIMASE_OODLE9` | input | live | explicit trusted Oodle version-9 game DLL path | the game's own DLL from the user's install |
| `PRIMASE_DCX` | input | live | folder recursively containing loose KRAK DCX files | the game's own files, extracted from the user's install |
| `PRIMASE_CAS_SAMPLES` | input | live | directory of indexed individual `.casblock` frames | the game's own CAS frames, cut from the user's install |
| `PRIMASE_SEKIRO_DIR` | input | live | install folder containing Data1 through Data5 BHD/BDT pairs | the user's own install |
| `PRIMASE_FROMSOFT_RIGID_ORACLE` | input | live | a SoulsFormats dump of an area: parts.jsonl (every MSB map piece and object part with its model and matrix), models.jsonl, missing-rigid.jsonl (placed models it could not dump, with why) and per placed model a binary of mesh/vertex counts followed by position and normal f32 triples in model space; worlds/fromsoft/tests/rigid_census.rs, sekiro_scene.rs | independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (`rigid` mode) |
| `PRIMASE_FROMSOFT_OBJECT_ORACLE` | input | blocked | main-HKX body fixtures for reference bd77fcb, decoded by an independent Havok reader; worlds/fromsoft/tests/object_collision.rs | the existing fixtures were decoded by our earlier C# exporter, not independently; replacement: a thin dump through Havoc (SoulsAssetPipeline d11caf9) |
| `PRIMASE_FROMSOFT_NAMES` | input | live | caller-provided UTF-8 Sekiro path dictionary | third-party: UXM's Sekiro path dictionary |
| `PRIMASE_FROMSOFT_DDS_ORACLE` | input | blocked | trusted C# DDS oracle assembly; protocol in worlds/fromsoft/README.md | the C# oracle's decoder source is gone; replacement: a third-party DDS decoder |
| `PRIMASE_FROMSOFT_SHADOW_ORACLE` | input | live | original decompressed `map.msb` and independent `shadows.json` (source SHA-256 and ordered part names plus six raw shadow fields); `worlds/fromsoft/tests/shadow_census.rs` | the install's map.msb with shadows.json; the JSON's producer is not recorded yet |
| `PRIMASE_FROMSOFT_STATE_ORACLE` | input | live | independent MSB part entity IDs and groups (`entities.jsonl`) and event-script decode (`emevd.jsonl`); schema in worlds/fromsoft/README.md | a private dump tool's MSB/EMEVD reading; only its build output is retained, so its independence is not yet shown |
| `PRIMASE_FROMSOFT_MATERIAL_ORACLE` | input | blocked | independent MTD fields, area material assignments and per-model texture availability for object collision; schema in worlds/fromsoft/README.md | the earlier oracle is gone; replacement: SoulsFormats' MTD reading in the dump harness, hand-worked fixtures for assignments and availability |
| `PRIMASE_FROMSOFT_PBR_ORACLE` | input | live | source DDS in `census/dds` plus independent `texture-statistics.json` channel extrema and means; `worlds/fromsoft/tests/pbr_census.rs` | the install's own DDS with statistics computed by a script kept with the private oracle tools |
| `PRIMASE_FROMSOFT_REFLECTANCE_ORACLE` | input | live | a JSON file naming one layered material's G-buffer program (path, sha256, the dielectric literal's offset and bits) and its DDS inputs (paths, sha256), whose traced reflectance the static bake must reproduce at every texel while the other lowered channels stay unchanged; `worlds/fromsoft/src/sekiro/passes/bake_assembly.rs` | the game's own shader program and textures from the user's install, with the literal's offset read from the program's bytes; the file only names and hashes them |
| `PRIMASE_FROMSOFT_LAYER_PILOT` | input | blocked | bounded layered-shader pilot inputs and reference pixels; schema in worlds/fromsoft/README.md | the existing pilot came from our earlier C# exporter's layer bake; replacement: the game's shaders on WARP |
| `PRIMASE_FROMSOFT_SCENE` | input | live | our earlier exporter's skate-scene folder of an area: scene.json (region, materials), its collision tables (a regression baseline, not an independent reference) and textures; cli/tests/fromsoft_collision_scene.rs, web_texture_decode.rs, worlds/fromsoft/tests/material_assignments.rs, shadow_census.rs | regression baseline: our earlier C# exporter's scene of the area, not an independent reference; placements are checked against the SoulsFormats dump instead |
| `PRIMASE_WEB_KTX2_TILES` | input | live | a web tile export with KTX2 textures and its plain set: tiles.json, the tiles, images/, plain/tiles.json and its tiles (scene_to_tiles <scene> <out> --ktx2); formats/web/tests/ktx2_audit.rs | our own output (scene_to_tiles --ktx2 of PRIMASE_FROMSOFT_SCENE), checked with an independent decoder (the pure-Rust basisu transcoder) against its own plain set; the hashes it prints are compared with the viewer's WASM transcoder by the viewer's audit |
| `PRIMASE_FROMSOFT_BHD_ORACLE` | input | live | reference archive outputs; schema in worlds/fromsoft/README.md | independent: SoulsFormats 9c0da6b with UXM's Sekiro dictionary, the dump harness's census mode (kept with the private oracle tools) |
| `PRIMASE_FROMSOFT_TPF_ORACLE` | input | blocked | independent texture-package census (packages.jsonl, area-dds/, errors.tsv); schema in worlds/fromsoft/README.md; worlds/fromsoft/tests/tpf_census.rs, tpf_pixels.rs | not built (a stated coverage gap): SoulsFormats' TPF reader in the dump harness's census mode; build it when a step 20 or in-game difference points at textures |
| `PRIMASE_HAVOK_CORPUS` | input | blocked | loose Havok 2016.2 tagfiles, matching compendium, and optional independent `.hkx.soup` oracle files; schema in toolkit/havok/README.md | the game's own tagfiles; the `.hkx.soup` oracle needs an independent reader (Havoc or HKLib thin dump) |
| `PRIMASE_FROSTBITE_EBX` | input | live | a folder of EBX documents as the game wrote them, mirroring their asset paths (searched recursively); every field's value must survive a rewrite, and the byte layout is compared with the reference writer; worlds/frostbite/tests/ebx.rs | the game's own EBX documents |
| `PRIMASE_RESKATE_CLI` | input | live | the ReSkate CLI executable, used as an independent EBX reader and reference writer (`ebx-values`, `ebx-rewrite-diff`); worlds/frostbite/tests/ebx.rs | third-party: ReSkate Studio's CLI (20261012) |
| `PRIMASE_CODEC_CORPUS` | input | live | TSV: codec, compressed path, oracle decoded path (tab-separated) | game data with a third-party decoder's output |
| `PRIMASE_DXBC_SHADERS` | input | blocked | shader binaries, independent disassembly and optional raw-register value fixtures; schema in toolkit/dxbc/README.md | the game's own shaders with third-party disassembly; the existing raw-register traces came from our own interpreter, not independently |
| `PRIMASE_SKATE_MASK_ORACLE` | input | live | PNGs and independent mask pixel hashes in `cases.json`; schema below | independent: Pillow's decode and channel mapping of the game's own textures, generated by a script kept with the private oracle tools |
| `PRIMASE_DATA` | config | live | the absolute path of a local TOML file mapping these names to string values (see above) | - |
| `PRIMASE_REQUIRE_REAL_DATA` | switch | live | optional: `1` makes every unset input fail its test; `gate` only the inputs the landing gate must have | - |
| `PRIMASE_CACHE` | runtime | live | optional: the disk cache folder of KTX2 encodings that `primase convert --to web --ktx2` (and any command with the same `--cache` option) uses when no `--cache` is given; read by commands, not tests | - |
| `PRIMASE_DXC` | input | live | Microsoft's `dxc` shader compiler executable, for the DXIL evaluator's comparison with WARP; `toolkit/dxil/tests/warp.rs` | third-party: Microsoft's dxc |
| `PRIMASE_WARP` | input | live | the WARP oracle program that runs a shader on Microsoft's WARP rasterizer; `toolkit/dxil/tests/common/warp.rs` | third-party: Microsoft's WARP rasterizer, through a thin host program kept with the private oracle tools |
| `PRIMASE_WARP_SCRATCH` | output | live | a folder for the WARP comparison's case files and results | - |
| `PRIMASE_DXIL_FUZZ_RUNS` | switch | live | optional: how many mutated fixtures the DXIL fuzz test runs (default 3000) | - |
| `PRIMASE_FROMSOFT_FRAMES` | input | live | a folder holding `frames.jsonl`: SoulsFormats' reading of a few Sekiro models as stored (per mesh its LOD0 face set, per vertex position, normal, tangents and UV0); `worlds/fromsoft/tests/handedness.rs` | independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (frames mode) |
| `PRIMASE_FROMSOFT_NORMAL_CURL` | input | live | a JSON list of curl measurements of Sekiro normal maps by a script separate from the reader (`name`, `verdict` per map); `worlds/fromsoft/tests/normal_basis.rs` | our own measurement script over the game's normal maps (separate from the reader under test, not a third-party reader) |
| `PRIMASE_GTA_FRAMES` | input | live | one ydr's high-detail geometries as an independent reader (CodeWalker) reads them, as stored in GTA's frame: one JSON file `{path, value: {models: [{geometries: [{positions, normals, tangents (xyzw), texcoords, indices}]}]}}`; the frame change, triangle fronts and bitangent sign are checked against it (needs `PRIMASE_GTA_DIR`, `PRIMASE_GTA_KEYS`), and its stored tangent frames against the frames its texture coordinates give; `worlds/gta/tests/handedness.rs`, `uv_frames.rs` | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `PRIMASE_GTA_BOUNDS` | input | live | props' embedded collision bounds (a ydr's own, a yft's first physics LOD's) as an independent reader (CodeWalker) reads them: a folder of JSON files, one per prop, `{path, leaves: [{type, flags, polygons: [{type, material, vertices (placed)}] or corners (8, placed)}]}`; `lower-gta-embedded-collision`'s triangles are checked against them (needs `PRIMASE_GTA_DIR`, `PRIMASE_GTA_KEYS`); `worlds/gta/tests/embedded_bounds.rs` | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `PRIMASE_GTA_REGION` | filter | live | optional: only this named gate region in `worlds/gta/tests/region_pipeline.rs` | - |
| `PRIMASE_HAVOK_OBJECT_CORPUS` | input | live | a folder of Havok tagfiles holding object convex bodies, checked against their stored half-spaces; `toolkit/havok/tests/convex_corpus.rs` | the game's own tagfiles, checked against their own stored half-spaces |
| `PRIMASE_UNITY_SUN` | input | live | an independent reader's (UnityPy) record of the scan level's RenderSettings sun as stored, the render-pipeline asset classes, the shining directional lights and the main-light choice; `worlds/unity/tests/light.rs` | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `PRIMASE_UNITY_SHADERS` | input | live | an independent reader's (UnityPy) shader render state of every material (subshader tags; per pass LightMode, blend factors, zwrite and cull with their bound properties); `worlds/unity/tests/material.rs` | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `PRIMASE_UNITY_PROGRAMS` | input | live | an independent reader's (UnityPy) compiled fragment programs of every shader (per pass the player variants, keywords, program type and code hash, and the constant buffers, textures and samplers); `worlds/unity/tests/program.rs` | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `PRIMASE_UNITY_FRAMES` | input | live | an independent reader's (UnityPy) decoding of a few meshes in Unity's own frame (vertices, normals, tangents, UV0, submesh triangles); `worlds/unity/tests/handedness.rs` | independent: UnityPy, through dump scripts kept with the private oracle tools |

A reference is independent of the code it checks, or it is a regression baseline of the current route (a frozen
run of today's code, to show a change altered nothing). Our own old code is neither: an input it produced is not a
check, so it is not kept. Known coverage gaps, each without an independent reference yet, are stated here rather than
hidden by skips; build one when a whole-route (step 20) or in-game difference points at its area:

- FromSoft: texture packages (TPF census and pixels), material definitions and assignments (MTD), Havok map and
  object collision against an independent decoder, the layer bake (grouping, periods, images: the game's layer
  shaders on WARP), blocker cards, extras (lights, hang edges and markers as stored), MSB part kinds other than map
  pieces and objects, map offsets, and world-state membership (the event-script rules that turn flags into placement
  conditions; the facts they read, MSB entity IDs and the scripts' decode, are checked against the state oracle).
- ReSkate: the vertex-blend output (the postcompile stage retargeting Studio's blend keys). `postcompile_vbrecord.rs`
  checks the record stage's I/O on a real compile (declarations, indices, texture coordinates, tangents, positions,
  donor rows, determinism) with our own readers; no independent decode checks the result. The decal retarget is
  checked in game at step 20 (Legion's blended materials); the two-texture-set retarget has no producer in any route
  and is unit-tested only.

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
and how it was verified: which affected checks ran, and which inputs were missing.

## Open design questions

- Whole-map decisions when a map is written in parts (the ReSkate blend budget, collision cells): they are made in a
  spool and finalize stage before the write traversal (core's `convert_parts_gathered`: a gather traversal, groups,
  finalize passes), so a writer never decides. The alternative, a writer applying them itself in parts mode, was
  set aside for now. Revisit if the stage proves costly, or if other destinations need the same.
