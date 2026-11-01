# primase design

## 1. What it is

**One library, not one tool per job.** Converting a game's world to another engine, a 3D tool or a printer usually
means one bespoke tool per pair. primase turns that into plumbing between any input and any output:

- **Readers** take a game's (or a file format's) data and attach what they know to a shared in-memory world.
- **Writers** take that world and produce an output: a skate. map (via ReSkate), glTF (Blender and other tools), and
  later USD, STL/3MF, OBJ, FBX and pen-plotter drawings.
- **Passes** transform the world in between: bake layered materials, decode and verify textures, decimate, build
  LODs, split into streaming tiles, stack decals, smooth collision, repair meshes for printing.
- **A pattern toolkit** holds everything games keep repeating: one `TexFormat` enum for every BC/ETC/ASTC format,
  compression codecs, vertex layouts and quantisations, archive/TOC patterns, name hashes, handedness and axis flips,
  Havok shapes. Each game added grows it, so the next game's reader is mostly composition of proven pieces.

It is not a growing universal spec. The core is small and fixed; everything else is typed data that readers attach
and writers ask for (section 3).

## 2. Principles

The laws in CONTRIBUTING.md ("The laws") are primase's principles.

## 3. The world: core plus components

**Storage:** an entity-component store (entities + typed components + queries). **Our own small store** (`core/src/world.rs`), not `hecs`: every column iterates in entity order, so output is a function of
the world's contents (determinism); components carry a stable namespaced name and a schema version
(serialisation and pass declarations use the name); unknown components are held as opaque values (JSON or bytes),
which core's own serialisation writes back untouched; ids are never reused. A reader spawns entities and attaches components; a pass queries
`(Mesh, LayeredMaterial)` and inserts `BakedPbr`.

**Registry, verification, references** (from MLIR, see `docs/research/MLIR.md`). Each crate
registers its components in a `Registry` (`pub fn register(reg: &mut Registry)`; a program composes them): name, schema
version, required/optional, verifier, entity-reference visitor, text codec. A value whose namespace is registered but
whose name or version is unknown is an error, never an opaque component; opaque components are for namespaces this
build doesn't know. Entities stay untyped ("has a"); **roles are checked, not stored**: the core verifier enforces the
role conventions (an `Instance` has a `Transform` and points at a live prototype; a primitive's material is alive and
has a material component) and each dialect verifies its own components. A component with `Entity` fields reports them
through its reference visitor, `Component::references`: `World::closure` follows it from root entities (a tile's
placements reach their prototypes, materials and textures), `World::subset` copies such a closure under the same ids, and
the verifier, the printer, the v2 writer and despawning passes can use it. Opaque components declare no references:
they travel with their entity but are not followed. One component of a name per entity (a list is one component holding
a list).

**Core components (fixed, versioned, small):**
| component | content |
|---|---|
| `SourceFrame` (resource, provenance) | the frame the reader converted from (`core::frame`); the world itself is always metres, right-handed, +Y up, -Z north |
| `Transform` | affine 3x4 (sheared/mirrored allowed, flagged by tolerance) |
| `Mesh` / `MeshFile`, `Instance`, `MaterialVariants` | shared geometry (vertex streams by semantic, primitives in draw order) or the file it is still in; placements instance it, picking per-variant materials |
| `Light` | a punctual light as glTF's `KHR_lights_punctual` defines it: point, spot (cones) or directional, linear colour, intensity in candela (lux for directional), optional range, whether it casts shadows, optionally its highlights' own colour and intensity; placed by its transform, shining along its local -Z |
| `Shadows` (`core.shadows`) | a drawn entity's shadows (a placement, a prototype for its placements that state none, a self-placed mesh): cast off, on or only (a shadow proxy, not drawn), absent casts; receive, the renderer's default when absent. The one place casting is stated (a source's fact, else the program's choice: `set-shadow-casting`); materials state none |
| `Collision`, `Surfaces` (resource) | collision geometry placed like a mesh: a triangle soup in the entity's space, one surface per triangle, surfaces named in the world's table (game properties stay in dialects), each with an optional `SurfaceClass` |
| `Texture` | pixels or encoded blocks + `TexFormat`, colour space, role, normal convention (X in red/alpha, green up/down, Z stored/rebuilt) |
| `LayeredMaterial` (`core.layered`) | one to four ordinary material references (null contributes black), per-layer UV selection and transform, and vertex-channel weights mapped after interpolation without renormalisation; blended normals are renormalised |
| `Name`, `Source` | display name; provenance key (archetype, MSB part, asset path) for debugging and review links |
| `Priority` (`core.priority`) | authored placement optionality, 0 required through 255 most optional; preserved independently of consumer filtering; skate-scene stores 0-15 in PLAC bits 8-11 and rejects larger values; `drop-optional{max=N}` leaves out placements above N, only when a pipeline asks |
| `Lods` (`core.lods`) | a prototype's levels of detail: its own mesh's level (0 finest), its other forms (each an entity holding what goes with that form's mesh: mesh or mesh file, variants, dialect parts; a form without a mesh draws nothing) and the source's default form (none when it authors none, as GTA's distance switching: `select-lod{form=default}` then takes the finest, counted); `select-lod{form=finest\|default\|N}` (`form` required) keeps one form (moving the form's components onto the prototype with `World::move_components`) and despawns the others; skate-scene writes it back, the glTF and ReSkate writers refuse it |
| `Tile` | for streaming and chunked processing |
| `StateAxis`, `Condition`, `StateOverride`, `StateAssignment` (resource) | world state (`core::state`): variables a level can be in (event flags, door states, IPL groups, active flags), when a placed entity exists (OR of ANDs over axis values) and what changes with the state (transform, material variant, prototype); `resolve-state` picks one state (option, else the resource, else defaults) for writers that cannot toggle, and leaves a world without state untouched |

**Dialects (namespaced component sets, each owned by a crate):**
- `fromsoft::` layered material (per-vertex layer weights, SAT permutation), MSB parts, Havok collision, hang ledges.
- `gta::` shader params, overlay draw order, time-of-day light flags, SLOD links.
- `insomniac::` zone records, grind/ledge paths, material overrides.
- `photo::` georeference, capture metadata, plane fits.
- `skate::` material classes, surface ids, markers (spawn, travel points, trainer spots, VFX), grind curves, streaming
  distances: what the ReSkate writer needs.
- `pbr::` baked metal-rough material (the common lowering target for glTF/USD/skate).

A component type carries a schema version. Core's own serialisation (v2 tables, the `.pwt` text form) carries unknown
components untouched unless their kind is marked **required**, in which case a reader that doesn't know it fails. No
other writer carries them: core is the interchange, and a format writer writes its format's meaning only (CONTRIBUTING.md,
the laws). **Lower and keep:** a lowering may keep its source component next to its result in the world (the bake keeps
`fromsoft.layered_material` beside `pbr.material`); a format writer writes the result its format expresses and reports
the source as left out.

**On disk:** the world serialises as "core + named component tables" (v2, `docs/SPEC.md`): a small manifest plus
little-endian binary tables, generalising the skate-scene v1 interchange format (a JSON manifest, binary tables and
GLB meshes) that existing front ends already write; `primase_reskate::interchange` reads and writes v1 unchanged.

## 4. Passes and lowering

A pass has a stable name, options with a text form, and declares the component names it **requires**, **provides**,
**removes** and **modifies** (changes in place), plus what it approximates. After each pass the manager checks that the
names that appeared are within `provides` and those that vanished within `removes`, and runs the **verifier** (on a
conversion's reader output and before its writer in every build; after every pass in debug builds, and in release
builds only when the program opts in with `PassManager::with_pass_checks(true)`: off there by default for speed). A pass that declares it **despawns entities**
(`drop-optional`, `select-lod`) may also make a name vanish that only the entities it despawned held, so it need not
list every component a dropped placement or a discarded form may carry; a name a surviving entity lost must still be in
`removes`, and the planner counts on `removes` alone. A writer declares a **target** (MLIR's conversion
target): per component name `Legal`, `Dynamic` (a per-value check: texture format, TRS-decomposable transform),
`Illegal`, and what it does with unknown names: a format writer leaves them out and reports them. `Carry` (written
through untouched) and `LowerAndCarry` (carried, and lowered as well) belong to core's own serialisation only, never to a
format writer's target. Full conversion requires every name Legal (or, for core's serialisation, carried); partial
conversion leaves unknown names out and reports them. A pass also declares what it **legalizes** (lowers), by default what it removes: a pass whose removals are
side effects (the file link of a texture it rewrote) or that keeps its source says what it lowers.

Pipelines are explicit lists in a text form (`load-texture-files,bake-fromsoft-layered{size=1024}`). The planner
builds one by MLIR's legalization depth: for each name to lower, the pass chain of least depth that lowers it with
legal outputs (ties by priority, then name), plus the producers of what those passes require, ordered by data
dependencies (providers, then passes that modify a name, then those that read it as it is) and checked by simulating the
declarations. An illegal name must be removed, a dynamic name's failing values removed or rewritten in place, a
`LowerAndCarry` name only lowered. Passes are made with the caller's options, by pass name. It fails naming the missing link ("no pass lowers `fromsoft.layered_material` to
anything `stl` accepts"). The chosen pipeline is printed and recorded in the report, so every run is reproducible as an
explicit pipeline. No fixed-point rewriting, no backtracking.

**Reports and instrumentation.** Passes record named counters and approximation remarks (entity, `Source`, what was
approximated) in the run report, which is deterministic (no timings in it). Instrumentation hooks run before and after
each pass and on failure: timing (kept out of the report), print-world-after(-change), and a reproducer (the pipeline
plus the world before the failing pass). Per-entity passes go through an adaptor that may run in parallel and applies
results in entity order; such a pass reads its entity and what it references, and writes only its entity. Passes that
merge entities merge their provenance. Analysis caching is deferred (`modifies` makes invalidation computable later).

**Text form.** The world has a deterministic text form (`.pwt`, see `docs/research/MLIR.md` §8: entities ascending, one
component per line, JSON bodies, payloads elided to length + sha256 in digest mode) that round-trips in full mode,
including f32 bit patterns. Golden tests are `input.pwt` + `pipeline.txt` -> `expected.pwt`; verifier tests annotate
expected errors in the input.

**First passes:** FromSoft layered material -> baked colour+normal (the
DXBC interpreter running Sekiro's own shader); GTA Blinn-Phong spec -> smoothness; FH6 swatch tint -> one texture;
DirectX normals -> OpenGL; left- to right-handed mirror + winding; decal overlay stacking (GTA order inside an entity,
opaque-first across); collision roof smoothing (crest surface per face); weld; long-edge split; texture caps with box
filter; tiling into 200 m cells; level rules (one name -> one payload) as a validator pass.

**A program's outputs as a surface.** A game crate states what each lane of a pixel program's outputs holds
(`OutputRole` on the binding's `OutputBinding`s: an albedo, a normal and its encoding, a gloss and its scalar encoding,
an occlusion of which light). What a lighting pass takes from those roles is read in one place,
`primase_core::shader::surface`: which output and lanes hold the albedo, the normal, the roughness (stored, one minus a
gloss, or from a Blinn-Phong exponent), the metallic, the reflectance, the specular strength, the emission and the
occlusion, and how each stored value decodes. Every consumer takes it from there: a material bake decoding a program it
evaluated on the CPU (`bake-fromsoft-materials`), and a viewer's composite decoding a G-buffer on the GPU, which packs
the reading with `primase_core::shader::surface::rows` and decodes it with that module's WGSL. A game's layout is
tested as a surface in its own crate (the fromsoft `gbuffer_surface` tests, the anvil and gta `resolve_shaders` tests).

## 5. Crates

```
core/         world, core components (among them what a shader program's values stand for, core::shader), TexFormat,
              frame conversions, pass manager, gates, serialisation (v2)
toolkit/      texcodec (TexFormat decoders/encoders and gates), dds, image (PNG, JPEG, sRGB, mips), glb, meshops
              (weld, split, meshopt), animation, lighting, exactsum, havok (Havok data lowered), cache
worlds/       one crate per game, engine family or destination: fromsoft, gta, frostbite, anvil, shaunwhite, unity,
              skatebird, peak, monogame, stuntboost, shred, reskate (the skate. map writer, its preparation and
              compile)
formats/      gltf (later: usd, stl/3mf, obj, ...; skate-scene interchange v1: worlds/reskate)
checks/       the architecture rules' tests and tests spanning crates
examples/     programs using the library (primase has no command line; programs live in their own repos)
```

Game and engine formats, the shader lifter (iridia), shader bytecode readers, and the byte, hash, vertex and
compression plumbing under them are libraries in their own repositories (CONTRIBUTING.md, "Crates in their own
repositories"). Bindings for other languages (python through pyo3, C# through csbindgen) are later.

**The crate rule.** Each game becomes its own crate (`worlds/sekiro`, `worlds/gta`, ...), and patterns that recur
across games become core or toolkit functions: the second time a game crate needs something another game already has
(a codec, a container walk, a vertex decode, a transform convention, a hash), it moves to the toolkit with a gate and
both games call it. A game crate ends up mostly composition: format specifics plus calls into the toolkit. Dependencies point down only (`worlds/formats -> toolkit -> core`); world crates never
depend on each other or on formats, and formats depend on no format but `formats/gltf` (CONTRIBUTING.md). `TexFormat` lives in core (a `Texture` component holds one); its codecs live in `toolkit/texcodec`. Crates are
two-way where it makes sense: a `reskate` reader lets a skate. map round-trip or go to Blender.

**Dependencies: small, focused, maintained crates that do one job well; no umbrella crates.** Each sits behind our own
trait (`TexDecoder`, `Compressor`, ...), so swapping it is local, and each is held to the same gates as our code.
Versions are pinned and default features off.
- **In (by role):** BCn decode `bcdec_rs`, BCn encode `block_compression` (CPU path, default features off, no `wgpu`) or `texpresso`, both pure Rust (`intel_tex_2` is out: its prebuilt C++/ISPC kernels need a native runtime to link and dispatch per CPU), DDS header `ddsfile`, PNG `png`,
  LZ4 `lz4_flex`, deflate/zlib `flate2`, binary layouts `binrw`, casts `bytemuck`, f16 `half`, mesh optimisation
  `meshopt`, bindings `pyo3` and `csbindgen`. Oodle is never bundled: on Windows the game's own `oo2core` DLL is loaded at run
  time from the install the user points at; off Windows (no game library to load) `oozextract` is the decoder, a
  target-specific dependency of `primase-foundations` that refuses by name what it cannot decode, and the gates hold
  it equal to the DLL on Windows. ETC/ASTC via
  `texture2ddecoder` only behind a gate.
- **Out:** umbrella libraries that pull in a whole domain for one function (`image`, `image_dds`), engine frameworks
  such as `bevy_ecs`.
- **Our own, deliberately:** the glTF writer, sRGB and mip filters, every game format.

**The ReSkate writer's geometry input** is `core.mesh`: what readers loaded and passes lowered (the layered-overlay
lowering among them) is what it writes. Re-reading a prototype's GLB from `core.mesh_file`, when the prototype has no
`core.mesh`, is the legacy path, kept because the writer loads geometry lazily, one placement band at a time: an M3
area holds 22-82 MB of GLB, but one tile of a whole GTA city 1.1-1.7 GB (six tiles: 38,051 GLBs, 8.6 GB), more
than a whole-scene load should hold. That laziness is to move behind the `core.mesh` interface (a loader filling
`core.mesh` per band) rather than stay a second input. The two paths are checked to write the same map.

**The plotter writer** (later) hands a world to a hidden-line renderer to draw it as pen-plotter lines: silhouette and
crease edges picked by dihedral angle, optional hatching from baked colour or normals, per-material pens.

## 6. Testing (gates are the product)
- **Decoder gates:** a decoder against an independent decoder or fixtures, over synthetic data and the real data the
  change affects; exact where the format defines it, else a stated tolerance.
- **Reader census:** a reader can run over all assets of its kind in a game and report decode failures by count, when
  a change needs that breadth; it is not run for every change.
- **Writer round trips:** write -> read back -> compare by meaning; omitted features are reported. A round trip through
  our own reader detects change; correctness of the output is checked by a consumer we didn't write (a validator, the
  destination program).
- **World validators:** identity rules ("texture name -> one image", "EBX name -> one partition"), frame sanity,
  non-finite and degenerate checks.

Real data never enters the repository: tests read it through `GAME_DATA` (CONTRIBUTING.md, "Fixtures and real
data").

## 7. Roadmap

**Later:** Sunset Overdrive, Sonic Adventure 2, STL/3MF, the plotter writer, and rich Blender output: the games'
real material setups (layered materials, shading recipes, spec/normal workflows) rebuilt as Blender node trees: its
own writer, which may build on the glTF writer, fed by Blender passes from core and the games' namespaced data.
Also later: a **native skate. level writer** that writes the game's own level data directly instead of going
through ReSkate Studio: first test fixtures, deterministic identities and a round-trip harness, then replacing one
mesh in a small test level and loading it in game.

## 8. Licence

GPL-3.0-or-later for the whole workspace. Dependencies must be GPL-3-compatible.

## Surface classes

`core.surfaces` (VERSION 2) gives each surface its source name and an optional class from a closed vocabulary:
`concrete`, `asphalt`, `stone`, `brick`, `tile`, `metal`, `wood`, `glass`, `plastic`, `rubber`, `fabric`, `grass`,
`dirt`, `gravel`, `sand`, `mud`, `snow`, `ice`, `water` (swimmable or wading) and `other` (classified, nothing
fits). A reader leaves the class unset; the source's lowering pass sets it from a named table of its own names and
counts the names it does not map; a writer maps classes onto its target (the skate raise: `class_to_skate`). No game
crate maps another game's surface names. Adding a class takes evidence that a source has such surfaces and a target
treats them differently, its line here and a VERSION bump; writers match the enum without a wildcard, so each must map
it before the workspace builds.
