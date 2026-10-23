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
| **test-input registry** (`toolkit/testdata`) | name and describe the real-data inputs tests read, games and third-party tools included; resolve them | game logic, depending on workspace crates, being a normal dependency of any crate | crate dependencies (not excusable in `architecture.toml`); `real_data_variables_are_declared` |
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
`toolkit/testdata`, which is never a normal dependency.

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

`PRIMASE_DATA` is the one configuration variable: the absolute path of a local TOML file, the whole configuration. Copy
[primase-data.example.toml](primase-data.example.toml) to an ignored `primase-data.toml` (or keep the private file
outside the checkout). The example contains placeholder paths only. Never commit a populated configuration.

```toml
[installs]
sekiro = 'D:/placeholder/games/Sekiro'
[keys]
gta = 'D:/placeholder/keys/gta'
[tools]
warp = 'D:/placeholder/tools/warp.exe'
[oracles]
root = 'D:/placeholder/oracles'   # <root>/<set>/<game>/<file>
[scratch]
root = 'D:/placeholder/scratch'
[run]
require = 'gate'
gate = 'smoke'                    # or 'full'
```

An install, keys or a tool is its key's path; a file in an install is below it; an oracle set's file is
`<oracles.root>/<set>/<game>/<file>`. Every section and key is optional, and an unknown section or key is an error
(a typo must not skip quietly). A query whose key is not configured prints one `SKIP` line naming the test and the key
and returns `None`. `[run] require = 'gate'` makes every input the landing gate must have fail instead (the `gate`
column below: a blocked file always is one), `'all'` every input: use them when running the real-data tests a
change affects, so a skipped affected test counts as a failure, not a pass. `[run] gate` says how much the heavy real-data tests cover
(`primase_testdata::depth`): `'smoke'` (the default) a sample, drawn with `primase_testdata::seed` (`[run] seed` when
given, to rerun one sample exactly, else the day's, so coverage accumulates across runs; the test prints it and names it
in its failures), for every batch; `'full'` everything, for the overnight gate. `[run] threads` (one by default) is
how many threads a heavy test's work may use, stated in its world as `core.threads` (`primase_testdata::threads`): a
machine opts into the load, and only work whose result is the same on any count reads it. A configured path that does not exist
fails: a configured input that is gone is never a skip. The one exception is an oracle set the machine does not hold:
with no `<oracles.root>/<set>` folder (in a root that exists) the set counts as not configured there, so a machine
can keep only the sets it has; a file missing inside a set that is there fails (a typo or a broken tree). A file in an install or an oracle set must be declared
(the declared files below): asking for an undeclared one fails, so its relative path is a literal the architecture
check can see. A configured input that yields nothing fails through `expect_found`. Values are literal: no expansion
and no relative-path rewriting, so use absolute paths and TOML literal strings (single quotes) for Windows
backslashes. Debug switches are not data: `PRIMASE_DEBUG='key=value;key=value'` sets the debug keys below.

The resolver lives in `toolkit/testdata` (`primase-testdata`), which must only appear in consumers'
`[dev-dependencies]`; normal core and production builds have no TOML dependency or feature flag. The file is read
afresh when needed, without mutating the environment. Ignored private names are `primase-data.toml` and
`primase-data.*.toml`; only the placeholder example is tracked.

For this release the older configuration still works: each `PRIMASE_*` variable, in the environment or in a flat
file of them, stands for what the last table gives and says once that it is deprecated. The next release removes
them. Tests ask by query only; a `PRIMASE_*` name in a source file is the configuration's, read by `primase-testdata`,
or a test's handoff to its own child process (the architecture check lists both).

A declared file's `status`: `live`, the obligation stands and the input it names is acceptable; `blocked`, the
obligation stands but no acceptable input exists yet (the existing data is not independent of what it checks, or its
producer is gone), so it is a landing-gate input whose tests fail when required and never pass by skipping;
`unheld`, the obligation stands and the input is acceptable but no machine here holds it (the why says what it needs),
so it is never a landing-gate input and its tests skip naming why wherever it is not there. An
obligation its owner retires is removed with its tests. `source` says where the data comes from (the game itself, a
third-party reader and its version, another exporter, our own earlier output) and so how far it is independent: a
regression baseline of our own output is labelled as one, never as an oracle. A file called independent must be
produced by something other than the code it checks.

<!-- real-data tables: made by primase_testdata::doc::tables from toolkit/testdata/src/registry.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `installs.sekiro` | yes | Sekiro's install folder (Data1 to Data5 BHD/BDT pairs, the Oodle 6 DLL) | the user's own install |
| `installs.elden` | - | Elden Ring's `Game` folder (Data0 to Data3, `sd/sd`, DLC archives when installed) | the user's own install |
| `installs.gta` | yes | a GTA V install folder (Enhanced or Legacy: the executable and the `.rpf` archives) | the user's own install |
| `installs.riders` | - | Riders Republic's folder of `.forge` archives (Steam build 25394987) | the user's own install |
| `installs.skatebird` | yes | SkateBIRD's install folder (`SkateBIRD_Data`: a Unity 2019.4.20f1 player) | the user's own install |
| `installs.sledding` | yes | Sledding Game's install folder (`Sledding Game_Data`: a Unity 6000.3 player) | the user's own install |
| `installs.stuntboost` | yes | STUNTBOOST's install folder (Steam buildid 25832744: `Content/Models/Export`) | the user's own install |
| `installs.skate` | yes | the skate. install the ReSkate tools use (`Data`: its `layout.toc`, TOCs and CAS archives; the Oodle 9 DLL) | the user's own install |
| `keys.gta` | yes | the user's GTA V archive key material in the community layout (`gtav_ng_key.dat`, `gtav_ng_decrypt_tables.dat`, `gtav_hash_lut.dat`, optional `gtav_aes_key.dat`); never committed | the user's own key material, checked against published digests |
| `keys.elden` | - | the user's Elden Ring archive keys, one PKCS#1 `RSA PUBLIC KEY` PEM per archive named after its stem, and `regulation.key`, the key of `regulation.bin` as 64 hexadecimal digits (held to the SHA-256 worlds/fromsoft/src/elden/mod.rs states) | the community unpacker's published keys (UXM's ArchiveKeys.cs) and SoulsFormats' RegulationDecryptor key for the game, written out by a script kept with the game's private tools |
| `tools.warp` | yes | the WARP oracle program that runs a shader on Microsoft's WARP rasterizer | third-party: Microsoft's WARP rasterizer, through a thin host program kept with the private oracle tools |
| `tools.dxc` | yes | Microsoft's `dxc` shader compiler executable | third-party: Microsoft's dxc |
| `tools.fxc` | - | Microsoft's `fxc` shader compiler executable | third-party: Microsoft's fxc |
| `tools.reskate_cli` | - | ReSkate Studio's CLI executable (`reskate_cli`), an independent EBX reader and reference writer | third-party: ReSkate Studio's CLI (20261012) |
| `tools.gltf_validator` | yes | the Khronos glTF-Validator executable (2.0.0-dev.3.10 release build) | third-party: Khronos glTF-Validator 2.0.0-dev.3.10 |
| `tools.oodle5` | yes | an Oodle version-5 DLL (`oo2core_5_win64.dll`) | the game's own DLL from the user's install |
| `tools.oodle6` | yes | an Oodle version-6 DLL (`oo2core_6_win64.dll`) | the game's own DLL from the user's install |
| `tools.oodle9` | yes | an Oodle version-9 DLL (`oo2core_9_win64.dll`) | the game's own DLL from the user's install |
| `tools.scene_skate` | yes | our own skate-scene validator (`scene-skate.dll`, run through `tools.dotnet`, or its executable) | our own C# validator, rebuilt from its last source (commit d78228a): it refuses what its checks refuse, it is not an oracle |
| `tools.unity_tpk` | yes | a type tree package (TPK, uncompressed or LZ4) with the class layouts for the Unity players read | input data: class layouts for the player's Unity version (repacked with a script kept with the private oracle tools) |
| `tools.dotnet` | yes | the .NET host (`dotnet`, with the .NET 8 runtime) that runs the C# programs given as assemblies: `scripts.sekiro/dds`'s DdsWorker.dll and a `tools.scene_skate` that is a .dll | third-party: Microsoft's .NET 8 runtime |
| `tools.python_pillow` | yes | a Python 3 interpreter with Pillow 11.0.0, which runs the Pillow oracle worker (toolkit/texcodec/tests/oracles/worker.py) | third-party: Pillow 11.0.0 |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

#### Oracle sets (`<oracles.root>/<set>/<game>/`)

| set | about | source |
| --- | --- | --- |
| `oracles.unitypy` | UnityPy's readings of a Unity player (object tables, samples, meshes, textures, materials, shaders, frames, listings) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.codewalker` | CodeWalker's readings of a GTA V install (archive listings, live-file tables, texture parents, resources, frames, bounds) | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `oracles.soulsformats` | SoulsFormats' readings of FromSoftware archives and files (rigid parts, skins, parts, census, frames, formats) | independent: SoulsFormats 9c0da6b (Sekiro) and SoulsFormatsNEXT ee1dd61 (Elden Ring), through a thin dump harness kept with the private oracle tools |
| `oracles.uxm` | UXM's path dictionaries (`dictionary.txt`, UTF-8, one archive path per line) | third-party: UXM-Selective-Unpack 9501be8's dictionaries |
| `oracles.game-code` | a game's own code run over its install (STUNTBOOST's parsers via reflection: one JSON per level part) | independent: the game's own code via reflection, the oracle tool kept with the game's private tools |
| `oracles.rrassetstudio` | RRAssetStudio's read of every Mesh of a Riders Republic install (`summary.jsonl`, one row per Mesh) | third-party: RRAssetStudio v3, run over the user's own install by the primase-oracles harness |
| `oracles.khronos` | Khronos glTF-Sample-Assets, one folder per model, with their expected readings | third-party: Khronos glTF-Sample-Assets at commit 15cd02d304e54cb42a4db4eefe719f76190f4b68; expectations by pygltflib and Pillow |
| `oracles.studio` | ReSkate Studio's compiles of maps our writer built (Patch folders) | third-party: ReSkate Studio 20261012's compile output |
| `oracles.reskate` | ReSkate's own readers' readings of skate. data (the render skeleton) | ReSkate's EBX reader (the source primase's EBX reader is ported from), through our C++ harness: a port check |
| `oracles.d3d-disassembler` | Microsoft's D3D shader disassembly of a game's shader containers (`NNNN.dxbc` with its listing) | independent: Microsoft's D3D Shader Disassembler, run over the user's own install |
| `oracles.pillow` | Pillow's decodings of game textures and what is computed from them (the skate mask cases) | independent: Pillow's decode and channel mapping of the game's own textures, generated by a script kept with the private oracle tools |
| `oracles.port` | another exporter's output for a game (its level folder, materials joined to their objects, terrain unpacked) | another exporter's output: an appearance baseline, not a reading of the source |
| `oracles.corpus` | the games' own files cut from their installs (shader containers, EBX documents, Havok tagfiles, DCX files, CAS frames) | the user's own installs: files extracted as stored |
| `oracles.scripts` | measurements by our own scripts over game data, separate from the readers they check (normal curl, texture statistics, shadow and state records, the eclipse reference) | our own scripts kept with the private oracle tools: separate from the readers under test, not third-party; some producers are not recorded yet |
| `oracles.baseline` | our own earlier outputs kept to detect change (an earlier exporter's scene, a map our writer built, a frozen stage result) | regression baselines: our own earlier outputs, not independent oracles |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `installs.riders/DataPC_MAD_USNAT.forge` | live | - | a large Anvil forge of version 27 (Riders Republic, Steam build 25394987: `DataPC_MAD_USNAT.forge`, 15,650,919,308 bytes, 767,463 entries; its tables are read, about 163 MB); worlds/anvil/tests/mesh_shape.rs, shapes.rs, meshes.rs | the user's own install; two MeshShape bodies' prefixes, section runs and tails hand-checked in a separate read-only decode (games/riders-republic docs/recon/meshshape-second-fixture.md, c068d20) |
| `installs.riders/DataPC_MAD_USNAT_patch_01.forge` | live | - | the patch forge of installs.riders/DataPC_MAD_USNAT.forge (Riders Republic, Steam build 25394987: `DataPC_MAD_USNAT_patch_01.forge`, 1,331,014,794 bytes, 320,777 entries); worlds/anvil/tests/meshes.rs | the user's own install; four Mesh bodies' descriptors, buffers and splits hand-checked in a separate read-only decode (games/riders-republic docs/recon/mesh-second-fixture.md, a03c970) |
| `installs.riders/DataPC_MAD_WhiteRoom.forge` | live | - | the base forge of installs.riders/DataPC_MAD_WhiteRoom_patch_01.forge's patch (Riders Republic, Steam build 25394987: `DataPC_MAD_WhiteRoom.forge`, 8,951,822 bytes, 388 entries); worlds/anvil/tests/data.rs | the user's own install; two resource blocks' sizes, checksums, names and class ids hand-checked in a separate read-only decode (games/riders-republic docs/recon/archive-reader-fit.md, a9f4959) |
| `installs.riders/DataPC_MAD_WhiteRoom_patch_01.forge` | live | - | one Anvil forge archive of version 27 (Riders Republic, Steam build 25394987: `DataPC_MAD_WhiteRoom_patch_01.forge`, 1,021,872 bytes, 337 entries); worlds/anvil/tests/forge.rs | the user's own install; rows hand-checked against a separate read-only census of the file |
| `installs.skate/Data` | live | yes | the skate. install's `Data` folder (its `layout.toc`, TOCs and CAS archives), read only | the user's own install |
| `installs.skatebird/SkateBIRD_Data` | live | yes | SkateBIRD's data folder (`SkateBIRD_Data`, a Unity 2019.4.20f1 player: SerializedFile version 21 containers), read with `tools.unity_tpk`; `worlds/unity/tests/serialized_census.rs`, `worlds/skatebird/tests/markers.rs` | the user's own install |
| `installs.sledding/Sledding Game_Data` | live | yes | a Unity player's data folder (`<Game>_Data`, holding `globalgamemanagers` and the `levelN` / `*.assets` files) | the user's own install |
| `installs.stuntboost/Content/Effects` | live | - | STUNTBOOST's compiled effects (`D2/*.xnb`, `D3/*.xnb`: MonoGame `EffectReader`, MGFX version 12, OpenGL profile): every one is read to its shaders, parameters and passes; worlds/stuntboost/tests/effects.rs | the user's own install |
| `oracles.baseline.gta/legion/blend-spec.json` | live | yes | optional: a frozen resolution of that record against that Patch (a regression baseline of our own stage), `{"materials": {name: key}}` | regression baseline: an earlier run of our own stage, frozen; not an independent oracle |
| `oracles.baseline.gta/legion/map` | live | yes | a map folder the ReSkate writer built with `vertex_blend` materials (holding `vertex_blend.json`); `worlds/reskate/tests/postcompile_vbrecord.rs` | our own writer's output: the stage's input, not a reference |
| `oracles.baseline.sekiro/ktx2-tiles` | live | yes | a web tile export with KTX2 textures and its plain set, both written from the same prepared cell worlds (one conversion writing both sets, `primase_web::convert::tiles`): tiles.json, the tiles, images/, plain/tiles.json and its tiles; formats/web/tests/ktx2_audit.rs | our own writer's output: examples/examples/sekiro_ktx2_tiles.rs writes it (slice A of Sekiro's m11_01, both sets in one conversion), so the plain set holds the KTX2 encoder's input; the checks are an independent decoder's (the pure-Rust basisu transcoder, against the plain set) and the viewer's WASM transcoder's (the printed hashes) |
| `oracles.baseline.sekiro/layer-pilot` | live | yes | layer-bake pipeline states of an area's layered materials (cases.json; per case, size cap and weight variant a state folder) with WARP's outputs for each (warp/); schema in worlds/fromsoft/README.md; worlds/fromsoft/tests/layer_pilot.rs | the states: our own bake's (its stated policy), written by worlds/fromsoft/examples/layer_pilot_states.rs over the user's install (m11_01, region -170,180,-50,300); independent: Microsoft's WARP (d3d10warp 6.2.26100.9711) running the game's programs on them, through a thin quad host kept with the private oracle tools |
| `oracles.baseline.sekiro/outfits` | live | - | a folder of the old C# character exporter's manifests, one `<id>.json` per character (`c1010`): the NPC row chosen, the display masks it showed (`options.DisplayMasks`), and per source mesh its material, display mask and whether it was included, hidden by its mask or left out as equipment; worlds/fromsoft/tests/npc_outfits.rs | regression baseline: the manifests our earlier C# exporter (sekiro-skate SekiroExtract, CharacterExport.cs) wrote for the Sekiro character pack 0.2.x; our own output, so it detects a change of the selection and checks no correctness |
| `oracles.baseline.sekiro/scene` | live | yes | our earlier exporter's skate-scene folder of an area: scene.json (region, materials), its collision tables (a regression baseline, not an independent reference) and textures; checks/tests/web_texture_decode.rs, worlds/fromsoft/tests/material_assignments.rs, shadow_census.rs | regression baseline: our earlier C# exporter's scene of the area, not an independent reference; placements are checked against the SoulsFormats dump instead |
| `oracles.baseline.skate/fbmod` | live | - | a folder of `.fbmod` files (searched recursively); each is read, written again byte for byte and every payload decoded (`tools.oodle9` for Oodle blocks); worlds/frostbite/tests/fbmod.rs. The costume files among them (top level, with Studio's `<name>.readback.glb` beside them when present) also gate the costume steps with `installs.skate/Data`: worlds/reskate/tests/costume.rs | ReSkate Studio 2026-10-12 `cosmetic-mesh-import` output: our own producer's files, a round-trip baseline, not an oracle |
| `oracles.codewalker.gta/active.json` | live | yes | optional: an independent reader's live-file tables of that install (JSON; schema in `worlds/gta/tests/archive_set.rs`) | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `oracles.codewalker.gta/ShadersGen9Conversion.xml` | live | - | optional: CodeWalker's `ShadersGen9Conversion.xml`, which names every effect's texture and constant parameters (current and previous name, constant-buffer offset); `worlds/gta/tests/program_tables.rs` holds the shader database's parameter records to it | independent: CodeWalker.Core 485d56b, a data file of its repository (CodeWalker.Core/ShadersGen9Conversion.xml) |
| `oracles.codewalker.gta/bounds` | live | yes | props' embedded collision bounds (a ydr's own, a yft's first physics LOD's) as an independent reader (CodeWalker) reads them: a folder of JSON files, one per prop, `{path, leaves: [{type, flags, polygons: [{type, material, vertices (placed)}] or corners (8, placed)}]}`; `lower-gta-embedded-collision`'s triangles are checked against them (needs `installs.gta`, `keys.gta`); `worlds/gta/tests/embedded_bounds.rs` | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `oracles.codewalker.gta/frames.json` | live | yes | one ydr's high-detail geometries as an independent reader (CodeWalker) reads them, as stored in GTA's frame: one JSON file `{path, value: {models: [{geometries: [{positions, normals, tangents (xyzw), texcoords, indices}]}]}}`; the frame change, triangle fronts and bitangent sign are checked against it (needs `installs.gta`, `keys.gta`), and its stored tangent frames against the frames its texture coordinates give; `worlds/gta/tests/handedness.rs`, `uv_frames.rs` | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `oracles.codewalker.gta/gtxd.json` | live | yes | optional: an independent reader's texture dictionary parents of that install with their source files (JSON; schema in `worlds/gta/tests/gtxd.rs`) | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `oracles.codewalker.gta/listing.jsonl` | live | yes | optional: an independent reader's listing of every archive of that install (JSON lines; schema in `worlds/gta/tests/census.rs`) | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `oracles.codewalker.gta/parity` | live | - | optional: a folder holding `dump.jsonl`, an independent reader's decoding of every file the gate regions' placements reach (the CodeWalker harness's `batch` mode over the list `parity_list` writes; schema in `worlds/gta/tests/parity.rs`) | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools (`batch` mode, written when needed) |
| `oracles.codewalker.gta/resources` | live | yes | optional: a folder of an independent reader's decodings of chosen drawables and texture dictionaries of that install (JSON; schema in `worlds/gta/tests/resource_oracle.rs`) | independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools |
| `oracles.codewalker.gta/ycd` | live | - | a folder holding `dictionaries.jsonl`: an independent reader's decoding of every clip dictionary (`.ycd`) of that install, the CodeWalker harness's `batch` mode over the install's `.ycd` list (`DumpYcd`: per animation its frames, duration, bone ids, and per sequence block and bone track the channel types and SHA-256 of the channel values and of the evaluated vectors); the channel decoder's values are checked against it (needs `installs.gta`, `keys.gta`); `worlds/gta/tests/ycd_channels.rs` | independent: CodeWalker.Core 485d56b (GPL-licensed: run as a tool, no code taken), through a thin dump harness kept with the private oracle tools (`batch` mode) |
| `oracles.corpus.elden/shaders` | live | - | a folder (searched recursively) of the DXIL containers of Elden Ring's Shader Model 6 shader bundles (one file per container), lifted, validated and written as WGSL, the refusals counted by reason with an example program; toolkit/dxil/tests/lift_corpus.rs | the user's own install: the containers of its `/shader/*.shaderbnd.dcx` binders and `/shader/*.shaderbdlebnd.dcx` bundles (ray-tracing `[rt]` bundles included), worlds/fromsoft/examples/corpus_elden_shaders.rs |
| `oracles.corpus.general/codecs.tsv` | live | yes | TSV: codec, compressed path, oracle decoded path (tab-separated) | independent: the games' own streams (Sekiro's DFLT DCX envelopes, skate.'s zlib CAS blocks, Riders Republic's terrain LZ4 blocks, GTA V's raw deflate archive files, cut by the corpus_* example programs) decoded by Python's zlib and the lz4 package 4.4.5, toolkit/compress/tests/oracles/codecs.py |
| `oracles.corpus.general/dxbc` | live | yes | Sekiro's 390 pixel programs (`.ppo`), Microsoft's disassembly of each (`.ppo.asm`) and the bindings, inputs and constant layout read from that text (`.ppo.reflection.json`); toolkit/dxbc/tests/disassembly.rs, reflection.rs; schema in toolkit/dxbc/README.md | independent: the game's own programs (`corpus.sekiro/shaders`) disassembled by Microsoft's fxc 10.0.26100 (`fxc /nologo /dumpbin`), the reflection read from the disassembly's text by toolkit/dxbc/tests/oracles/dxbc_corpus.py |
| `oracles.corpus.gta/legacy-effects` | unheld: it needs GTA V Legacy's install, which no machine holds (the Enhanced edition has no effect files) | - | a folder (searched recursively) of GTA V Legacy's compiled effects (`.fxc`, RAGE `rgxe`) as extracted from the install (the Enhanced edition has none); every one is read, its programs' containers checked and lifted; worlds/gta/tests/fxc_census.rs | the user's own install: the effect files extracted from its archives |
| `oracles.corpus.gta/shader-database.awc` | live | - | GTA V Enhanced's shader database (`sga_win32_60_final.awc`, magic `SGD2`, from update/update2.rpf `common/shaders/fxdb`) as the install stores it after decryption and decompression: every program and effect is read, every program lifted; worlds/gta/tests/fxc_census.rs | the user's own install: the file extracted from its archive, worlds/gta/examples/corpus_gta_shader_database.rs |
| `oracles.corpus.riders/shaders` | live | - | a folder (searched recursively) of the DXBC containers of Riders Republic's shader pack (ShaderDataDX11.bin, split at its containers), lifted, validated and written as WGSL, the refusals counted by reason; toolkit/dxbc/tests/lift_corpus.rs | the user's own install: the programs split from its shader pack |
| `oracles.corpus.sekiro/dcx` | live | yes | folder recursively containing loose KRAK DCX files | the game's own files from the user's install: worlds/fromsoft/examples/corpus_sekiro_dcx.rs (KRAK envelopes as stored, the first 256 in hash order up to 4 MiB) |
| `oracles.corpus.sekiro/havok` | blocked | yes | loose Havok 2016.2 tagfiles, matching compendium, and optional independent `.hkx.soup` oracle files; schema in toolkit/havok/README.md | the game's own tagfiles; the `.hkx.soup` oracle has no independent producer: Sekiro's Havok 2016.2 compressed meshes have no third-party decoder (HKLib refuses SDK 20160200; Havoc gives the mesh tree as stored, undecoded; Smithbox's decoder needs HKLib's 2018 objects) |
| `oracles.corpus.sekiro/object-hkx` | live | yes | a folder of Havok tagfiles holding object convex bodies, checked against their stored half-spaces; `toolkit/havok/tests/convex_corpus.rs` | the game's own tagfiles, checked against their own stored half-spaces |
| `oracles.corpus.sekiro/shaders` | live | - | a folder (searched recursively) of the DXBC containers of Sekiro's extracted pixel and vertex programs (`.ppo`, `.vpo`), lifted, validated and written as WGSL, the refusals counted by reason; toolkit/dxbc/tests/lift_corpus.rs | the user's own install: the programs extracted from its shader archives |
| `oracles.corpus.skate/cas` | live | yes | directory of indexed individual `.casblock` frames | the game's own CAS frames from the user's install: worlds/reskate/examples/corpus_skate_cas.rs (one Oodle block per frame, 16 entries per superbundle TOC) |
| `oracles.corpus.skate/ebx` | live | - | a folder of EBX documents as the game wrote them, mirroring their asset paths (searched recursively); every field's value must survive a rewrite, and the byte layout is compared with the reference writer; worlds/frostbite/tests/ebx.rs | the game's own EBX documents |
| `oracles.corpus.skate/shaders` | live | - | a folder (searched recursively) of the DXIL containers of skate.'s CompiledBytecode resources (one file per container), lifted, validated and written as WGSL, the refusals counted by reason with an example program; toolkit/dxil/tests/lift_corpus.rs | the user's own install: the containers of its superbundles' CompiledBytecode resources, worlds/reskate/examples/corpus_skate_shaders.rs |
| `oracles.corpus.sledding/shaders` | live | - | a folder (searched recursively) of the DXBC containers of a Unity player's compiled pixel programs, as primase-unity extracts them, lifted, validated and written as WGSL, the refusals counted by reason; toolkit/dxbc/tests/lift_corpus.rs | the user's own install: the programs read from its shader assets |
| `oracles.d3d-disassembler.riders/shader-pack` | live | - | a folder of DXBC containers `NNNN.dxbc` (Riders Republic's ShaderDataDX11.bin, Steam build 25394987, split at its containers: 2,443) each with `NNNN.txt`, Microsoft's D3D Shader Disassembler text of it; toolkit/dxbc/tests/disassembly.rs | independent: Microsoft's D3D Shader Disassembler, run over the user's own install's pack (kept in a private cache) |
| `oracles.game-code.stuntboost/levels` | live | yes | the game's own parsers run over that install: per level part `<L>_<N>.json` (node names as its `ParsedName` reads them; its collision model as `SE.CollisionModel` loads it: meshes, grippable bits, ramps, ledges) and `<L>_<N>.tris.bin` (every collision triangle); worlds/stuntboost/tests/oracle.rs | independent: the game's own code via reflection, the oracle tool kept with the game's private tools |
| `oracles.khronos.general/sample-assets` | live | yes | a folder of Khronos glTF-Sample-Assets models, one folder per model (`<Model>/<Model>.glb` or `.gltf` with its buffers, `expected.json` written by pygltflib from the same file, `validator.json` by the Khronos validator and, for a model with images, `images.json` by Pillow; for a JPEG image `<Model>/images/<n>.jpg` and the RGBA8 rows Pillow decodes of it, `<Model>/images/<n>.rgba`) | third-party: Khronos glTF-Sample-Assets at commit 15cd02d304e54cb42a4db4eefe719f76190f4b68; expected.json by pygltflib 1.16.5; images.json by Pillow 11.0.0 |
| `oracles.pillow.sekiro/skate-mask` | live | yes | PNGs and independent mask pixel hashes in `cases.json`; schema below | independent: Pillow's decode and channel mapping of the game's own textures, generated by a script kept with the private oracle tools |
| `oracles.port.sledding/level1` | live | yes | optional: another exporter's output folder for that game's `level1` and its materials joined to their objects (JSON; schema in `worlds/unity/tests/lower.rs`) | another exporter's output: an appearance baseline, not a reading of the source |
| `oracles.port.sledding/materials.json` | live | yes | optional: another exporter's output folder for that game's `level1` and its materials joined to their objects (JSON; schema in `worlds/unity/tests/lower.rs`) | another exporter's output, joined by a script kept with the private oracle tools |
| `oracles.port.sledding/terrain` | live | yes | optional: that exporter's terrain pieces unpacked (raw arrays, `index.json`, its layer means; schema in `worlds/unity/tests/terrain.rs`) | another exporter's terrain, unpacked by a script kept with the private oracle tools |
| `oracles.reskate.skate/renderskeleton.json` | live | - | skate.'s render skeleton as ReSkate's EBX reader reads it (`renderskeleton.json`: every bone's name, parent, name hash, rest and world basis rows and translation, inverse bind rows); worlds/reskate/tests/game_skeleton.rs | ReSkate's EBX reader (the source primase's EBX reader is ported from) through our C++ harness `cos_skeleton` (games/sekiro tools/reskate-spec, commit 1e43b57): checks the port and the field reading, not the interpretation; the test's independent evidence is the asset's own poses agreeing with each other |
| `oracles.rrassetstudio.riders/meshes` | live | - | RRAssetStudio v3's read of every Mesh in installs.riders (a folder: summary.jsonl, one row per Mesh with counts, submeshes and material ids, and SHA-256s of its positions as f32 and absolute indices as u32, little-endian; sample/; identity.json); worlds/anvil/tests/census.rs, run by hand | a third-party oracle: RRAssetStudio v3 (zip SHA-256 f13edec5...) run over the user's own install by the primase-oracles harness dumps/rras-anvil (d5c48a9); identity.json pins its modules and the forges |
| `oracles.scripts.sekiro/dds` | live | yes | a folder holding the DDS decoding worker `DdsWorker.dll` and the libraries it loads; protocol in worlds/fromsoft/README.md; worlds/fromsoft/tests/tpf_pixels.rs | independent: BCnEncoder.Net 2.2.1 (MIT OR Unlicense), through a thin decoding worker kept with the private oracle tools |
| `oracles.scripts.sekiro/normal-curl.json` | live | yes | a JSON list of curl measurements of Sekiro normal maps by a script separate from the reader (`name`, `verdict` per map); `worlds/fromsoft/tests/normal_basis.rs` | our own measurement script over the game's normal maps (separate from the reader under test, not a third-party reader) |
| `oracles.scripts.sekiro/pbr` | live | yes | source DDS in `census/dds` plus independent `texture-statistics.json` channel extrema and means; `worlds/fromsoft/tests/pbr_census.rs` | the install's own DDS with statistics computed by a script kept with the private oracle tools |
| `oracles.scripts.sekiro/reflectance` | live | yes | a folder: `reflectance.json`, naming one layered material's G-buffer program (path, sha256, the dielectric literal's offset and bits) and its DDS inputs (paths, sha256), every path relative to the folder, and the program and DDS files it names; the static bake must reproduce the traced reflectance at every texel while the other lowered channels stay unchanged; `worlds/fromsoft/src/sekiro/passes/bake_assembly.rs` | the game's own shader program and textures from the user's install, with the literal's offset read from the program's bytes; the file only names and hashes them |
| `oracles.scripts.sekiro/shadow` | live | yes | original decompressed `map.msb` and independent `shadows.json` (source SHA-256 and ordered part names plus six raw shadow fields); `worlds/fromsoft/tests/shadow_census.rs` | the install's map.msb with shadows.json; the JSON's producer is not recorded yet |
| `oracles.scripts.sekiro/state` | live | yes | independent MSB part entity IDs and groups (`entities.jsonl`) and event-script decode (`emevd.jsonl`); schema in worlds/fromsoft/README.md | a private dump tool's MSB/EMEVD reading; only its build output is retained, so its independence is not yet shown |
| `oracles.scripts.skate/eclipse` | live | - | a folder of the decodings of three skate. clips' ECLP resources (`c_proto_ofbnb_idle_lfoot`, `c_proto_onb_inair_bs_grab_method_loop`, `c_proto_onb_trick_o_varialheel_high`): per clip `<clip>.eclp` (the bytes), `<clip>.tracks.json` (formats and every rotation, translation and scalar track's frames and values), and `sha256.txt` of the bytes; worlds/reskate/tests/game_eclipse.rs | our own earlier Python decoder (from the executable's encoder), a port check, not an independent oracle |
| `oracles.soulsformats.elden/census` | live | - | an independent census of installs.elden in oracles.soulsformats.sekiro/census's schema (the archives' decrypted headers as `<name>.plain`, entries.tsv, binders.tsv, errors.tsv, samples; hashes 16 hex digits; an archive's shard is its index in UXM's Elden Ring archive list); worlds/fromsoft/tests/census.rs, and asset_collision.rs (the asset collision binders entries.tsv lists) | independent: SoulsFormatsNEXT ee1dd61 with UXM 9501be8's keys, RSA and path hash, the Elden Ring harness's `census` mode (kept with the private oracle tools) |
| `oracles.soulsformats.elden/format` | live | - | an independent reader's field-by-field reading of Elden Ring files read from installs.elden (JSON lines, floats as hex bit patterns): `msb.jsonl` (every map layout: its models, every part with kind, model, transform, entity IDs, shadow controls and hit filter, and its spawn point regions with name, position and rotation), `btl.jsonl` (every light bank: its version and lights, or the reference's refusal), `havok.jsonl` (chosen tiles' map collision and assets' collision: every compressed mesh shape's stored sections, vertices, primitives, shape keys and extern shape instances), `matbin.jsonl` (every material binder's members in member order: shader and source paths, key, every parameter's name, key, type and value, every sampler's type, path, key and two trailing floats), `metaparam.jsonl` (every shader bundle's metaparam: binder, bundle and member names, header words, texture slots, render passes, CAParams and MtdParams); schema in worlds/fromsoft/README.md; worlds/fromsoft/tests/msb_census.rs, btl_census.rs, havok_census.rs, matbin_census.rs, metaparam_census.rs | independent: SoulsFormatsNEXT ee1dd61 with UXM 9501be8's keys and path hash, and HKLib ac40bc9 for Havok, the Elden Ring harness's format modes (kept with the private oracle tools) |
| `oracles.soulsformats.elden/npc.jsonl` | live | - | JSON lines: one of the NPC param's header facts (`kind` param: member, regulation version, paramType, dataVersion, detected and definition row size, format flags, unk06, row count), one per row (`kind` row: id, name, the 32 modelDispMask cells), then one per Enemy and DummyEnemy part of every map the UXM dictionary names (`kind` enemy or dummy: map path, part name, model, NPCParamID); worlds/fromsoft/tests/npc_outfits.rs | independent: SoulsFormatsNEXT ee1dd61 (RegulationDecryptor, PARAM with Paramdex's ER/Defs/NpcParam.xml as Smithbox cbd477a8 ships it, applied at the regulation's version; MSBE), the Elden Ring harness's `npc` mode (kept with the private oracle tools) |
| `oracles.soulsformats.sekiro/census` | live | yes | reference archive outputs; schema in worlds/fromsoft/README.md | independent: SoulsFormats 9c0da6b with UXM's Sekiro dictionary, the dump harness's census mode (kept with the private oracle tools) |
| `oracles.soulsformats.sekiro/frames` | live | yes | a folder holding `frames.jsonl`: SoulsFormats' reading of a few Sekiro models as stored (per mesh its LOD0 face set, per vertex position, normal, tangents and UV0); `worlds/fromsoft/tests/handedness.rs` | independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (frames mode) |
| `oracles.soulsformats.sekiro/m11_01` | live | yes | a SoulsFormats dump of an area: parts.jsonl (every MSB map piece and object part with its model and matrix), models.jsonl, missing-rigid.jsonl (placed models it could not dump, with why) and per placed model a binary of mesh/vertex counts followed by position and normal f32 triples in model space; worlds/fromsoft/tests/rigid_census.rs, sekiro_scene.rs | independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (`rigid` mode) |
| `oracles.soulsformats.sekiro/materials` | live | yes | independent MTD fields and block lengths (mtd.jsonl, mtd-boundaries.jsonl); schema in worlds/fromsoft/README.md; worlds/fromsoft/tests/mtd_census.rs | independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (`mtd` mode): the fields from its MTD reader, the consumed block lengths from its writer's rewrite of each file |
| `oracles.soulsformats.sekiro/npc` | live | - | a folder of SoulsFormats dumps of the Sekiro NPC param and of characters' materials: npcparam.json (NpcParam.param of gameparam.parambnd.dcx read by PARAM with Paramdex's NpcParam.xml: header facts, row size, every row's id, name and every cell by its internal name), per character id `<id>.materials.json` (the FLVER's materials by index, name and MTD, and each mesh's material index), and `enemies.jsonl` with `maps.json` (every Enemy and DummyEnemy part of the install's ten MSBS maps: map, model name, NPCParamID, ThinkParamID); worlds/fromsoft/tests/npc_outfits.rs | independent: SoulsFormats 9c0da6b with Paramdex's SDT/Defs/NpcParam.xml (sha256 dc4be2da), through a thin dump harness kept with the private oracle tools (`params`, `chrmaterials` and `enemies` modes) |
| `oracles.soulsformats.sekiro/objects` | live | yes | the game's object main .hkx files (179, castle) with Havoc's reading of each (`<name>.hkx.json`: every object, its class and fields as stored, floats as bits); bodies, shape classes, convex cores and poses checked exactly, compressed meshes counted as uncovered; worlds/fromsoft/tests/object_collision.rs | independent: Havoc (SoulsAssetPipeline d11caf9), through a thin dump harness kept with the private oracle tools (`havok-json` mode); no independent decoder reads Sekiro's compressed meshes (HKLib supports Havok 2018 only) |
| `oracles.soulsformats.sekiro/parts` | live | - | a folder of SoulsFormats dumps of the Wolf's parts, one folder per part name (`bd_m_9120_l`): the files of oracles.soulsformats.sekiro/skin's skeleton.json, meshes.jsonl, meshes/<i>.bin and chrbnd.json (the partsbnd's members), for the part's FLVER; worlds/fromsoft/tests/sekiro_parts.rs | independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (`skin` mode, area `part:<name>`) |
| `oracles.soulsformats.sekiro/skin` | live | - | a folder of SoulsFormats dumps of Sekiro characters, one folder per character id (`c1010`): skeleton.json (every FLVER bone's name, parent, stored transform and ComputeLocalTransform), meshes.jsonl (each mesh's dynamic flag, palette, face sets and layouts), meshes/<i>.bin (per vertex position, normal, bone indices and weights as stored), dummies.json, anibnd.json; worlds/fromsoft/tests/character_skin.rs | independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (`skin` mode) |
| `oracles.soulsformats.sekiro/tpf` | live | yes | independent texture-package census (packages.jsonl, area-dds/, errors.tsv); schema in worlds/fromsoft/README.md; worlds/fromsoft/tests/tpf_census.rs, tpf_pixels.rs | independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (`tpf` mode); area-dds/ holds every distinct DDS under `/map/` |
| `oracles.studio.gta/legion/patch` | live | yes | ReSkate Studio's Patch folder (its `layout.toc`, `Win32` TOCs and CAS archives) of the map in `oracles.baseline.gta/legion/map`, compiled by a build that carries the vertex-blend weight (validated: 20261012); `worlds/reskate/tests/postcompile_fb.rs`, `postcompile_vbrecord.rs` | third-party: ReSkate Studio 20261012's compile of that map |
| `oracles.unitypy.skatebird/census.json` | live | yes | an independent UnityPy object-table census of that folder (JSON; schema in `worlds/unity/tests/serialized_census.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.skatebird/listing.json` | live | yes | optional: an independent reader's (UnityPy) object names and BuildSettings scene list of `installs.skatebird/SkateBIRD_Data` (JSON; schema in `worlds/unity/tests/listing.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.sledding/frames.json` | live | yes | an independent reader's (UnityPy) decoding of a few meshes in Unity's own frame (vertices, normals, tangents, UV0, submesh triangles); `worlds/unity/tests/handedness.rs` | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.sledding/listing.json` | live | yes | optional: an independent reader's (UnityPy) object names and BuildSettings scene list of `installs.sledding/Sledding Game_Data` (JSON; schema in `worlds/unity/tests/listing.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.sledding/materials.json` | live | yes | optional: an independent reader's materials of that folder (JSON; schema in `worlds/unity/tests/material.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.sledding/meshes.json` | live | yes | optional: an independent reader's decoded meshes of that folder (JSON; schema in `worlds/unity/tests/mesh.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.sledding/samples.json` | live | yes | optional: an independent reader's decoded sample objects of that folder (JSON; schema in `worlds/unity/tests/oracle.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.sledding/scan.json` | live | yes | optional: an independent reader's scan of a level of that folder (JSON; schema in `worlds/unity/tests/hierarchy.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.sledding/shader-programs.json` | live | yes | an independent reader's (UnityPy) compiled fragment programs of every shader (per pass the player variants, keywords, program type and code hash, and the constant buffers, textures and samplers); `worlds/unity/tests/program.rs` | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.sledding/shader-state.json` | live | yes | an independent reader's (UnityPy) shader render state of every material (subshader tags; per pass LightMode, blend factors, zwrite and cull with their bound properties); `worlds/unity/tests/material.rs` | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.sledding/sun.json` | live | yes | an independent reader's (UnityPy) record of the scan level's RenderSettings sun as stored, the render-pipeline asset classes, the shining directional lights and the main-light choice; `worlds/unity/tests/light.rs` | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.unitypy.sledding/textures` | live | yes | optional: an independent reader's decoded textures of that folder (a folder of PNGs and `index.json`; schema in `worlds/unity/tests/texture.rs`) | independent: UnityPy, through dump scripts kept with the private oracle tools |
| `oracles.uxm.elden/dictionary.txt` | live | - | caller-provided UTF-8 Elden Ring path dictionary | third-party: UXM's Elden Ring path dictionary |
| `oracles.uxm.sekiro/dictionary.txt` | live | yes | caller-provided UTF-8 Sekiro path dictionary | third-party: UXM's Sekiro path dictionary |

#### Debug keys (`PRIMASE_DEBUG`)

| key | about |
| --- | --- |
| `gta.archive_filter` | only archives whose path contains this text |
| `gta.region` | only this named gate region of worlds/gta/tests/region_pipeline.rs |
| `scene.filter` | only scene folders whose path contains this text |
| `allow_live` | `1` lets a real-data gate report input folders that changed during the run and continue |
| `shader.diff_limit` | how many programs of a corpus the WARP comparison runs |
| `shader.warp_all` | `1` compares every program the WARP oracle can bind |
| `skatebird.voronoi_level` | the SkateBIRD level the Voronoi bake test bakes (default `level1`) |
| `skatebird.voronoi_digest` | the digest the Voronoi bake test's maps must have |
| `fromsoft.bake_digest` | the digest the baked layer textures of Sekiro's m11_01 must have (printed by an earlier run) |
| `anvil.bake_digest` | the digest the baked Riders Republic Materials must have |
| `dxil.fuzz_runs` | how many mutated fixtures the DXIL fuzz test runs (default 3000) |

#### Older names (this release only)

| name | stands for |
| --- | --- |
| `PRIMASE_SCENES_GTA` | retired |
| `PRIMASE_SCENES_SEKIRO` | retired |
| `PRIMASE_MAPS_GTA` | retired |
| `PRIMASE_GTA_DIR` | `installs.gta` |
| `PRIMASE_GTA_KEYS` | `keys.gta` |
| `PRIMASE_GTA_LISTING` | `oracles.codewalker.gta/listing.jsonl` |
| `PRIMASE_GTA_ACTIVE` | `oracles.codewalker.gta/active.json` |
| `PRIMASE_GTA_RESOURCES` | `oracles.codewalker.gta/resources` |
| `PRIMASE_GTA_PARITY` | `oracles.codewalker.gta/parity` |
| `PRIMASE_GTA_GTXD` | `oracles.codewalker.gta/gtxd.json` |
| `PRIMASE_GTA_FRAMES` | `oracles.codewalker.gta/frames.json` |
| `PRIMASE_GTA_BOUNDS` | `oracles.codewalker.gta/bounds` |
| `PRIMASE_GTA_SHADER_CONVERSION` | `oracles.codewalker.gta/ShadersGen9Conversion.xml` |
| `PRIMASE_GTA_SHADERS` | `oracles.corpus.gta/legacy-effects` |
| `PRIMASE_GTA_SHADER_DB` | `oracles.corpus.gta/shader-database.awc` |
| `PRIMASE_GTA_FILTER` | debug key `gta.archive_filter` |
| `PRIMASE_GTA_REGION` | debug key `gta.region` |
| `PRIMASE_SCENE_FILTER` | debug key `scene.filter` |
| `PRIMASE_RESKATE_PATCH` | `oracles.studio.gta/legion/patch` |
| `PRIMASE_RESKATE_BLEND_PATCH` | `oracles.studio.gta/legion/patch` |
| `PRIMASE_RESKATE_BLEND_MAP` | `oracles.baseline.gta/legion/map` |
| `PRIMASE_RESKATE_BLEND_SPEC` | `oracles.baseline.gta/legion/blend-spec.json` |
| `PRIMASE_SKATE_DATA` | `installs.skate/Data` |
| `PRIMASE_UNITY_V21_DATA` | `installs.skatebird/SkateBIRD_Data` |
| `PRIMASE_UNITY_V21_CENSUS` | `oracles.unitypy.skatebird/census.json` |
| `PRIMASE_UNITY_V21_LISTING` | `oracles.unitypy.skatebird/listing.json` |
| `PRIMASE_UNITY_DATA` | `installs.sledding/Sledding Game_Data` |
| `PRIMASE_UNITY_TPK` | `tools.unity_tpk` |
| `PRIMASE_UNITY_LISTING` | `oracles.unitypy.sledding/listing.json` |
| `PRIMASE_UNITY_ORACLE` | `oracles.unitypy.sledding/samples.json` |
| `PRIMASE_UNITY_SCAN` | `oracles.unitypy.sledding/scan.json` |
| `PRIMASE_UNITY_MESHES` | `oracles.unitypy.sledding/meshes.json` |
| `PRIMASE_UNITY_TEXTURES` | `oracles.unitypy.sledding/textures` |
| `PRIMASE_UNITY_MATERIALS` | `oracles.unitypy.sledding/materials.json` |
| `PRIMASE_UNITY_SUN` | `oracles.unitypy.sledding/sun.json` |
| `PRIMASE_UNITY_SHADERS` | `oracles.unitypy.sledding/shader-state.json` |
| `PRIMASE_UNITY_PROGRAMS` | `oracles.unitypy.sledding/shader-programs.json` |
| `PRIMASE_UNITY_FRAMES` | `oracles.unitypy.sledding/frames.json` |
| `PRIMASE_UNITY_PORT` | `oracles.port.sledding/level1` |
| `PRIMASE_UNITY_PORT_MATERIALS` | `oracles.port.sledding/materials.json` |
| `PRIMASE_UNITY_PORT_TERRAIN` | `oracles.port.sledding/terrain` |
| `PRIMASE_STUNTBOOST_DIR` | `installs.stuntboost` |
| `PRIMASE_STUNTBOOST_ORACLE` | `oracles.game-code.stuntboost/levels` |
| `PRIMASE_ANVIL_DIR` | `installs.riders` |
| `PRIMASE_ANVIL_FORGE` | `installs.riders/DataPC_MAD_WhiteRoom_patch_01.forge` |
| `PRIMASE_ANVIL_FORGE_BASE` | `installs.riders/DataPC_MAD_WhiteRoom.forge` |
| `PRIMASE_ANVIL_FORGE_USNAT` | `installs.riders/DataPC_MAD_USNAT.forge` |
| `PRIMASE_ANVIL_FORGE_USNAT_PATCH` | `installs.riders/DataPC_MAD_USNAT_patch_01.forge` |
| `PRIMASE_ANVIL_RRAS_MESHES` | `oracles.rrassetstudio.riders/meshes` |
| `PRIMASE_OUT` | `scratch.root` |
| `PRIMASE_WARP_SCRATCH` | `scratch.root/warp` |
| `PRIMASE_ALLOW_LIVE` | debug key `allow_live` |
| `PRIMASE_SCENE_SKATE` | `tools.scene_skate` |
| `PRIMASE_GLTF_VALIDATOR` | `tools.gltf_validator` |
| `PRIMASE_KHRONOS_GLTF` | `oracles.khronos.general/sample-assets` |
| `PRIMASE_OODLE5` | `tools.oodle5` |
| `PRIMASE_OODLE6` | `tools.oodle6` |
| `PRIMASE_OODLE9` | `tools.oodle9` |
| `PRIMASE_DCX` | `oracles.corpus.sekiro/dcx` |
| `PRIMASE_CAS_SAMPLES` | `oracles.corpus.skate/cas` |
| `PRIMASE_CODEC_CORPUS` | `oracles.corpus.general/codecs.tsv` |
| `PRIMASE_SEKIRO_DIR` | `installs.sekiro` |
| `PRIMASE_ELDEN_DIR` | `installs.elden` |
| `PRIMASE_ELDEN_KEYS` | `keys.elden` |
| `PRIMASE_ELDEN_NAMES` | `oracles.uxm.elden/dictionary.txt` |
| `PRIMASE_ELDEN_BHD_ORACLE` | `oracles.soulsformats.elden/census` |
| `PRIMASE_ELDEN_FORMAT_ORACLE` | `oracles.soulsformats.elden/format` |
| `PRIMASE_FROMSOFT_NAMES` | `oracles.uxm.sekiro/dictionary.txt` |
| `PRIMASE_FROMSOFT_RIGID_ORACLE` | `oracles.soulsformats.sekiro/m11_01` |
| `PRIMASE_FROMSOFT_SKIN_ORACLE` | `oracles.soulsformats.sekiro/skin` |
| `PRIMASE_FROMSOFT_PART_ORACLE` | `oracles.soulsformats.sekiro/parts` |
| `PRIMASE_FROMSOFT_BHD_ORACLE` | `oracles.soulsformats.sekiro/census` |
| `PRIMASE_FROMSOFT_FRAMES` | `oracles.soulsformats.sekiro/frames` |
| `PRIMASE_FROMSOFT_MATERIAL_ORACLE` | `oracles.soulsformats.sekiro/materials` |
| `PRIMASE_FROMSOFT_TPF_ORACLE` | `oracles.soulsformats.sekiro/tpf` |
| `PRIMASE_FROMSOFT_OBJECT_ORACLE` | `oracles.soulsformats.sekiro/objects` |
| `PRIMASE_FROMSOFT_SHADOW_ORACLE` | `oracles.scripts.sekiro/shadow` |
| `PRIMASE_FROMSOFT_STATE_ORACLE` | `oracles.scripts.sekiro/state` |
| `PRIMASE_FROMSOFT_PBR_ORACLE` | `oracles.scripts.sekiro/pbr` |
| `PRIMASE_FROMSOFT_REFLECTANCE_ORACLE` | `oracles.scripts.sekiro/reflectance` |
| `PRIMASE_FROMSOFT_NORMAL_CURL` | `oracles.scripts.sekiro/normal-curl.json` |
| `PRIMASE_FROMSOFT_DDS_ORACLE` | `oracles.scripts.sekiro/dds` |
| `PRIMASE_FROMSOFT_LAYER_PILOT` | `oracles.baseline.sekiro/layer-pilot` |
| `PRIMASE_FROMSOFT_SCENE` | `oracles.baseline.sekiro/scene` |
| `PRIMASE_WEB_KTX2_TILES` | `oracles.baseline.sekiro/ktx2-tiles` |
| `PRIMASE_HAVOK_CORPUS` | `oracles.corpus.sekiro/havok` |
| `PRIMASE_HAVOK_OBJECT_CORPUS` | `oracles.corpus.sekiro/object-hkx` |
| `PRIMASE_SKATE_SKELETON_ORACLE` | `oracles.reskate.skate/renderskeleton.json` |
| `PRIMASE_SKATE_ECLIPSE_REFERENCE` | `oracles.scripts.skate/eclipse` |
| `PRIMASE_SKATE_MASK_ORACLE` | `oracles.pillow.sekiro/skate-mask` |
| `PRIMASE_FROSTBITE_EBX` | `oracles.corpus.skate/ebx` |
| `PRIMASE_RESKATE_CLI` | `tools.reskate_cli` |
| `PRIMASE_RESKATE_FBMOD` | `oracles.baseline.skate/fbmod` |
| `PRIMASE_DXBC_SHADERS` | `oracles.corpus.general/dxbc` |
| `PRIMASE_DXBC_PACK_DISASSEMBLY` | `oracles.d3d-disassembler.riders/shader-pack` |
| `PRIMASE_SHADER_CORPUS_SEKIRO` | `oracles.corpus.sekiro/shaders` |
| `PRIMASE_SHADER_CORPUS_RR` | `oracles.corpus.riders/shaders` |
| `PRIMASE_SHADER_CORPUS_UNITY` | `oracles.corpus.sledding/shaders` |
| `PRIMASE_SHADER_DIFF_LIMIT` | debug key `shader.diff_limit` |
| `PRIMASE_SHADER_WARP_ALL` | debug key `shader.warp_all` |
| `PRIMASE_SKATEBIRD_VORONOI_LEVEL` | debug key `skatebird.voronoi_level` |
| `PRIMASE_SKATEBIRD_VORONOI_DIGEST` | debug key `skatebird.voronoi_digest` |
| `PRIMASE_ANVIL_BAKE_DIGEST` | debug key `anvil.bake_digest` |
| `PRIMASE_FROMSOFT_BAKE_DIGEST` | debug key `fromsoft.bake_digest` |
| `PRIMASE_DXIL_FUZZ_RUNS` | debug key `dxil.fuzz_runs` |
| `PRIMASE_DXC` | `tools.dxc` |
| `PRIMASE_FXC` | `tools.fxc` |
| `PRIMASE_WARP` | `tools.warp` |
| `PRIMASE_REQUIRE_REAL_DATA` | `[run] require` (`1`: `all`) |

<!-- end of real-data tables -->

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

Declare a file in `toolkit/testdata/src/registry.rs` when a test first asks for it; the tables above are made from
the registry and checked against it. Test outputs go to `scratch()` or a temporary directory, never into the source
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
half, meshopt, ruzstd (pure-Rust zstd decoding, MIT), pyo3, csbindgen, libloading (a game's own Oodle DLL, loaded at run time from the install, never
bundled), oozextract (only as a gate's second decoder), thiserror, serde/serde_json, indexmap, sha2, aes (AES-128 block operations), num-bigint (raw RSA public operations), encoding_rs (strict Shift-JIS), sha1 (key material checks), quick-xml (pull parser for game XML data files), naga (the shader IR `primase_shader::Program` wraps, with its validator and WGSL writer; SPIR-V and GLSL front ends later: our own IR would need every one of those written by us; approved 2026-10-22), criterion
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
