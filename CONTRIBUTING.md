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
| **reader** | read the source it is given; attach core components and its own namespace (`gta.*`, `fromsoft.*`); keep the source as authored (bone-space geometry with its skeleton and pose, every level of detail, every state); report what it tolerated or can't read; load the heavy data (meshes, textures) of the parts it is asked for | lower, bake, approximate, apply poses, pick levels of detail or states, select what a destination draws, shape for a destination (another namespace's components, destination units), read options from the environment | I/O only in modules that opt in as `reader I/O` (clippy); the namespaces a reader declares, checked by `convert` (core) for every reader |
| **pass** | transform the world it is given: lower, bake, approximate, select, split, merge, prepare for a destination; declare what it requires, provides, removes and modifies; report every approximation and exclusion by name | touch files, the environment or processes; run outside the pass manager | clippy's file/environment ban; every pass registered by name and every public lowering run by a pass (`checks/tests/architecture.rs`) |
| **writer** | encode the prepared data its format supports, laying it out as its encoding needs (files, tiles, splits by encoded size) without changing what it means; omit and report what it doesn't support; refuse invalid prepared data with the reason | lower, approximate, resize, select, merge, generate or move content (a pass does that first), read other files, change the world | writer entry points take `&World` (`checks/tests/architecture.rs`); I/O only in modules that opt in as `writer I/O` |
| **core** | shared meanings, the store, frames, the pass machinery and its generic passes, gates | game or destination names, renderer tuning, file access, dependencies on other crates | crate dependencies, game names, clippy's file ban (`checks/tests/architecture.rs`) |
| **toolkit** | shared mechanisms that games supply data to | game branches, depending on formats or worlds | crate dependencies, game names |
| **formats** | file formats that are not one game's: their readers, writers and passes | depending on worlds or on formats other than `formats/gltf` | crate dependencies, game names |
| **test-input registry** (primase-testdata) | name and describe the real-data inputs tests read, games and third-party tools included; resolve them | game logic, depending on workspace crates, being a normal dependency of any crate | crate dependencies (not excusable in `architecture.toml`); `real_data_variables_are_declared` |
| **examples** | compose readers, passes, writers and `convert` as a program does, to show the library | data logic of their own (reimplementing a step, querying or editing world components, reading a format, running a pass past the manager); hiding a recipe's choices | `checks/tests/architecture.rs` (`entry_points`) |

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
checks/               the architecture rules' tests and tests spanning crates (no library)
examples/             programs using the library, quoted by the README (no library)
```

Dependencies point down only: `worlds/formats -> toolkit -> core` (`checks` and `examples` may use any of them). primase is a
library: a program that converts something (a game's map to a destination) lives in its own repository and composes
these crates; primase has no command line. A world crate depends on the toolkit and
core, never on a format. Each says what it is in its manifest (`[package.metadata.primase] kind`): a `game` crate (a
game or a game family) or a `destination` crate may depend on its engine family's `engine` crate (`engine = "<package>"`),
never the other way round, and an engine crate never depends on a game, a destination or a recipe; no other world
depends on another. A format crate depends on the toolkit, core and `formats/gltf` (the scene writer other formats
embed; the GLB codec itself is `toolkit/glb`) only. What two of them share belongs lower. Dev-dependencies may add format crates (round-trip gates) and
`primase-testdata`, which is never a normal dependency.

### Crates in their own repositories

The readers of game and engine formats, and the plumbing under them, are libraries in their own repositories, so
someone looking for one finds it without primase. Any primase crate may use them; they use none of primase's.

| repository | crates |
| --- | --- |
| [primase-foundations](https://github.com/Kalmarv/primase-foundations) | checked byte reads, hashes, vertex decoding, colour, sampling, codecs (`compress`) |
| [primase-testdata](https://github.com/Kalmarv/primase-testdata) | the real-data test loader (`GAME_DATA`), a dev-dependency only |
| [dx](https://github.com/Kalmarv/dx) | dxcontainer, dxbc-reader, dxil-reader, rsx-reader |
| [iridia](https://github.com/Kalmarv/iridia) | iridia, iridia-interface, iridia-dxbc, iridia-dxil, iridia-d3d9, iridia-mojo, iridia-exec |
| [frostbite-formats](https://github.com/Kalmarv/frostbite-formats), [rage-engine-formats](https://github.com/Kalmarv/rage-engine-formats), [anvil-formats](https://github.com/Kalmarv/anvil-formats), [fromsoft-formats](https://github.com/Kalmarv/fromsoft-formats), [unity-formats](https://github.com/Kalmarv/unity-formats), [monogame-formats](https://github.com/Kalmarv/monogame-formats), [rme-formats](https://github.com/Kalmarv/rme-formats) | one engine's formats each |
| [havok-formats](https://github.com/Kalmarv/havok-formats), [morpheme-formats](https://github.com/Kalmarv/morpheme-formats), [lua51-bytecode](https://github.com/Kalmarv/lua51-bytecode) | middleware formats |
| [primase-naga](https://github.com/Kalmarv/primase-naga) | a fork of naga carrying changes upstream has not released |

They are git dependencies pinned to a commit (`[workspace.dependencies]`). A change to one is made, checked and pushed
in its repository; primase then moves its `rev` to that commit. Every repository that uses a shared crate (most use
primase-foundations) names the same commit of it as primase does: two commits of one repository are two different crates
to cargo, and their types don't meet. To work on one beside primase, point cargo at a local checkout without editing the
manifest: `cargo check --config "patch.'https://github.com/Kalmarv/dx'.dxbc-reader.path='../dx/dxbc-reader'"`.

Names are owned: core, toolkit and formats name no game, engine or destination; a world crate uses only its own
(`[package.metadata.primase] names`): a game crate its games and engine, an engine crate its engine, a destination
crate its destination and engine. A source crate naming a destination, or any crate naming another game, is a break.
Vendor formats used across engines (Havok, DXBC, DXIL) are toolkit mechanisms. One vendor vocabulary is a stated exception
and sits in core: Direct3D's addressing of shader resources (`core::shader`: `D3dBinding`, `D3dClass`, `D3dSignature`, and the
`ShaderFormat` names of the bytecodes). It is **provenance**: a lifter maps a program's resources and varyings into the
API-independent meaning once and keeps the register, class and signature row it came from beside it, because every shader
lifted here comes from a D3D-family bytecode and what runs or inspects a program (an executor, a consumer answering a request)
addresses its resources by those. It is never the meaning itself, and no other vendor's vocabulary gets this place: a new
engine's or middleware's names stay in its own crate.

Namespaced components (`fromsoft.*`, `gta.*`, `skate.*`, ...) are owned by one crate: the game's or destination's
crate (`skate.*`: `worlds/reskate`, with the skate-scene interchange it carries). `pbr.*` lives in core. A middleware or
format crate that several games share owns its own namespace too (`havok.*`: `toolkit/havok`), with the passes that lower it
in ways no game's policy enters (a game-free lowering: what the data means, not what a game wants from it); the crate names no
game, as every toolkit crate, and a game's choice over the middleware's data (which clips, what to do with a bone it lacks)
stays in the game's crate as a pass of its own. A namespace has one owner: two crates never write the same prefix.

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
  `BTreeMap`, `IndexMap`, or sort; `clippy::iter_over_hash_type` refuses a `for` loop over one, not an iterator
  chain); no thread-timing-dependent order; output names come from content. Parallel code must produce the same
  bytes as serial code (a test proves it).
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
block); oracle outputs committed as fixtures are generated from synthetic inputs. Tests that need real data ask
`primase-testdata` for it by a typed query: `install(Game::Sekiro)`, a file in an install
(`install_file(Game::Skate, "Data")`), `keys(Game::Gta)`, `tool(Tool::Warp)`, a file of an oracle set
(`oracle(OracleSet::CodeWalker, Game::Gta, "listing.jsonl")`), `scratch()` for what a test writes and keeps, and
`debug("key")` for a debug switch. Code never has a default data path.

`GAME_DATA` is the one configuration variable: the absolute path of a local TOML file naming where this machine's
installs, keys, tools, oracle sets and scratch folder are, and how much the heavy tests cover. Its layout, every key, the
`[run]` settings (`require`, `gate`, `seed`, `threads`) and the debug keys of `GAME_DATA_DEBUG` are
[primase-testdata's](https://github.com/Kalmarv/primase-testdata) to describe (its README and
[game-data.example.toml](https://github.com/Kalmarv/primase-testdata/blob/main/game-data.example.toml), placeholder
paths only). Copy the example to an ignored `game-data.toml` (or keep the private file outside the checkout); never
commit a populated configuration. A query whose key is not configured prints one `SKIP` line naming the test and the
key; `[run] require = 'gate'` makes the inputs a change must have fail instead, so run the real-data tests a change
affects with it and a skipped affected test counts as a failure, not a pass.

Each crate declares what its tests read in its own `tests/inputs/mod.rs`; the table beside it,
`tests/inputs/README.md`, is made from those declarations and checked against them (`tests/real_data_inputs.rs`), and
asking for an undeclared input fails. primase-testdata is only ever a dev-dependency, so normal builds carry no TOML
dependency or feature flag. A `PRIMASE_*` name in a source file is a test's handoff to its own child process (the
architecture check lists them).

A declared file's `status`: `live`, the obligation stands and the input it names is acceptable; `blocked`, the
obligation stands but no acceptable input exists yet (the existing data is not independent of what it checks, or its
producer is gone), so it is a landing-gate input whose tests fail when required and never pass by skipping;
`unheld`, the obligation stands and the input is acceptable but no machine here holds it (the why says what it needs),
so it is never a landing-gate input and its tests skip naming why wherever it is not there. An
obligation its owner retires is removed with its tests. `source` says where the data comes from (the game itself, a
third-party reader and its version, another exporter, our own earlier output) and so how far it is independent: a
regression baseline of our own output is labelled as one, never as an oracle. A file called independent must be
produced by something other than the code it checks.

The inputs of each crate are in its `tests/inputs/README.md` (for the crates in their own repositories, in theirs).

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
- ReSkate: the level-root string repair (`postcompile::levelroot`). `level_root.rs` checks the gate, the repair and the
  writer's layout on a document in the shape of Studio's level root and on Legion's Patch with our own readers; that the
  slot's target is what faults (`strlen` of the loaded `char*`, three Shaun White level 4 crash dumps) is read from the
  dumps, and that the repaired level loads is checked only in game (repeated launches of level 4).

Declare a file in the crate's `tests/inputs/mod.rs` when a test first asks for it. Test outputs go to `scratch()` or a temporary directory, never into the source
tree.

`oracles.pillow.sekiro/skate-mask` supplies `cases.json` for the target mask census. Its `cases` array contains
`source` (PNG path relative to the fixture root, or absolute), `source_sha256`, `metallic`, `roughness`,
`occlusion` (explicit packed R binding), `strength`, `width`, `height` and `mask_sha256` (independently decoded
and mapped row-major RGBA8 base pixels). Source images and independent expected values stay outside the repository.

## Dependencies

Small, focused, maintained crates that do one job, each behind our own trait so swapping it is local, each held to the
same gates as our code. Pinned in the root `Cargo.toml` `[workspace.dependencies]` (exact version, default features
off) and used with `workspace = true`. **No umbrella crates** (`image`, `image_dds`, engine frameworks like
`bevy_ecs`). In use or approved: bcdec_rs, block_compression (CPU path only, no wgpu: pure-Rust BC7 encode) / texpresso (pure Rust), ddsfile, png, zune-jpeg (with zune-core: pure-Rust JPEG decoding, MIT OR Apache-2.0 OR Zlib; no unsafe without its `x86`/`neon` features, which stay off; approved 2026-10-23: png's counterpart for the glTF samples' JPEGs), lz4_flex, flate2, binrw, bytemuck,
half, meshopt, ruzstd (pure-Rust zstd decoding, MIT), quantydb-sqlite (pure-Rust read-only SQLite 3 file reader, MIT, no unsafe, no dependencies; behind `rme_formats::database`, which keeps our strict refusals; approved 2026-10-26 to replace our own `primase-sqlite`), pyo3, csbindgen, libloading (a game's own Oodle DLL, loaded at run time from the install, never
bundled), oozextract (Oodle's decoder off Windows, where there is no game library to load, a target-specific dependency of `primase-foundations`; on Windows only as a gate's second decoder, held equal to the game's DLL), thiserror, serde/serde_json, indexmap, sha2, aes (AES-128 block operations), num-bigint (raw RSA public operations), encoding_rs (strict Shift-JIS), sha1 (key material checks), quick-xml (pull parser for game XML data files), naga (the shader IR `iridia::Program` wraps, with its validator and WGSL writer; SPIR-V and GLSL front ends later: our own IR would need every one of those written by us; approved 2026-10-22), criterion
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
