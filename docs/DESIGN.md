# primase design

## 1. What it is

**One library, not one tool per job.** Converting a game's world to another engine, a 3D tool or a printer usually
means one bespoke tool per pair. primase turns that into plumbing between any input and any output:

- **Readers** take a game's (or a file format's) data and attach what they know to a shared in-memory world.
- **Writers** take that world and produce an output: a skate. map (via ReSkate), glTF (Blender and other tools), a web
  world for three.js / react-three-fiber, and later USD, STL/3MF, OBJ, FBX and pen-plotter drawings.
- **Passes** transform the world in between: bake layered materials, decode and verify textures, decimate, build
  LODs, split into streaming tiles, stack decals, smooth collision, repair meshes for printing.
- **A pattern toolkit** holds everything games keep repeating: one `TexFormat` enum for every BC/ETC/ASTC format,
  compression codecs, vertex layouts and quantisations, archive/TOC patterns, name hashes, handedness and axis flips,
  Havok shapes. Each game added grows it, so the next game's reader is mostly composition of proven pieces.

It is not a growing universal spec. The core is small and fixed; everything else is typed data that readers attach
and writers ask for (section 3).

## 2. Principles

0. **Roles.** Readers only read, passes transform, writers only write; core holds shared meanings and the pass
   machinery, the toolkit shared mechanisms; programs, in their consumers' repositories, compose them and make the
   choices. CONTRIBUTING.md ("The model", "Roles") says what each may and may not do and which check enforces it. Without these roles the plumbing between
   any input and any output stops being plumbing.
1. **Small core, open edges.** The core fixes only what everyone must agree on: entities, transforms, meshes,
   textures, units/axes/handedness, colour space. Game- and target-specific meaning lives in namespaced components.
2. **Readers attach what they know; writers declare what they need.** Nobody has to model everything.
3. **Lowering, not lossy import.** Data stays in its richest form until a writer needs a simpler one; then a pass
   lowers it (FromSoft layered material -> baked PBR) and records what was approximated.
4. **Correctness is part of the API.** Every decoder has a second, independent decoder or a fixture to check it
   against; every reader has a census over all of a game's assets; rules like "one name -> one payload" are validators
   over the world, not ad-hoc scripts. Decoder bugs in widely used community tools (a BC5 decoder that drops the second
   channel, a BC1 decoder that ignores punch-through alpha) are only ever caught by cross-checking.
5. **Community tools become test oracles, not dependencies.** Our readers decode the same files as the established
   community tools (CodeWalker, SoulsFormats, ALERT, Pillow, ReSkate's readers); any difference is a bug in one of
   them.
6. **Deterministic.** The same inputs give the same output bytes. Matching an older tool's bytes is not a goal:
   outputs are compared with references by meaning.

## 3. The world: core plus components

**Storage:** an entity-component store (entities + typed components + queries). **Our own small store** (`core/src/world.rs`), not `hecs`: every column iterates in entity order, so output is a function of
the world's contents (determinism, principle 6); components carry a stable namespaced name and a schema version
(serialisation and pass declarations use the name); unknown components are held as opaque values (JSON or bytes) and
written back untouched; ids are never reused. A reader spawns entities and attaches components; a pass queries
`(Mesh, LayeredMaterial)` and inserts `BakedPbr`.

**Registry, verification, references** (from MLIR, see `docs/research/MLIR.md`). Each crate
registers its components in a `Registry` (`pub fn register(reg: &mut Registry)`; the CLI composes them): name, schema
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
| `Light` | a punctual light as glTF's `KHR_lights_punctual` defines it: point, spot (cones) or directional, linear colour, intensity in candela (lux for directional), optional range, whether it casts shadows; placed by its transform, shining along its local -Z |
| `Shadows` (`core.shadows`) | a drawn entity's shadows (a placement, a prototype for its placements that state none, a self-placed mesh): cast off, on or only (a shadow proxy, not drawn), absent casts; receive, the renderer's default when absent. The one place casting is stated (a source's fact, else the program's choice: `set-shadow-casting`); materials state none |
| `Collision`, `Surfaces` (resource) | collision geometry placed like a mesh: a triangle soup in the entity's space, one surface per triangle, surfaces named in the world's table (game properties stay in dialects), each with an optional `SurfaceClass` |
| `Texture` | pixels or encoded blocks + `TexFormat`, colour space, role, normal convention (X in red/alpha, green up/down, Z stored/rebuilt) |
| `LayeredMaterial` (`core.layered`) | one to four ordinary material references (null contributes black), per-layer UV selection and transform, and vertex-channel weights mapped after interpolation without renormalisation; blended normals are renormalised |
| `Name`, `Source` | display name; provenance key (archetype, MSB part, asset path) for debugging and review links |
| `Priority` (`core.priority`) | authored placement optionality, 0 required through 255 most optional; preserved independently of consumer filtering; skate-scene stores 0-15 in PLAC bits 8-11 and rejects larger values; `drop-optional{max=N}` leaves out placements above N, only when a pipeline asks |
| `Lods` (`core.lods`) | a prototype's levels of detail: its own mesh's level (0 finest), its other forms (each an entity holding what goes with that form's mesh: mesh or mesh file, variants, dialect parts; a form without a mesh draws nothing) and the source's default form (none when it authors none, as GTA's distance switching: `select-lod{form=default}` then takes the finest, counted); `select-lod{form=finest\|default\|N}` (`form` required) keeps one form (moving the form's components onto the prototype with `World::move_components`) and despawns the others; skate-scene writes it back, the glTF, web and ReSkate writers refuse it |
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

A component type carries a schema version. Unknown components are carried through serialisation untouched unless their
kind is marked **required**, in which case a reader that doesn't know it fails (glTF's `extensionsRequired`).
**Lower and keep:** a lowering may keep its source component next to its result (the bake keeps
`fromsoft.layered_material` beside `pbr.material`), and a writer marks the source `LowerAndCarry`: carried (glTF:
`extras.primase`), and lowered too, so the planner runs the lowering.

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
`Illegal`, `Carry` (written through untouched) or `LowerAndCarry` (carried, and lowered as well), and what it does
with unknown names. Full conversion requires every name Legal or carried; partial conversion keeps unknown names per
the target. A pass also declares what it **legalizes** (lowers), by default what it removes: a pass whose removals are
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
the reading with `iridia::surface::rows` and decodes it with that module's WGSL. A game's layout is tested as a
surface in its own crate (the fromsoft `gbuffer_surface` tests, the anvil and gta `resolve_shaders` tests).

## 5. Crates

```
core/         world, core components, TexFormat, frame conversions, pass manager, gates, serialisation (v2)
toolkit/      texcodec (TexFormat decoders/encoders and gates), dds, image (PNG, sRGB, mips), compress (lz4,
              deflate/zlib, Frostbite CAS blocks, Oodle via the game's own DLL), container (TOC and archive helpers,
              version gates), hash, vertex (layouts, quantisation, strips), havok, meshops (weld, split, meshopt),
              frostbite (skate.'s containers: TOC, bundles, native DB, CAS archives, shader depots and lookups),
              validate (identity rules)
worlds/       one crate per game or engine: reskate (Frostbite CAS/TOC/EBX, map writer and reader), fromsoft, gta,
              forza, unity, insomniac, ...
formats/      gltf, web, usd, stl/3mf, obj, ... (skate-scene interchange v1: worlds/reskate)
bindings/     python (pyo3) and C# (csbindgen), so existing tools can call the toolkit
checks/       the architecture rules' tests and tests spanning crates
examples/     programs using the library (primase has no command line; programs live in their own repos)
```

**The crate rule.** Each game becomes its own crate (`worlds/sekiro`, `worlds/gta`, ...), and patterns that recur
across games become core or toolkit functions: the second time a game crate needs something another game already has
(a codec, a container walk, a vertex decode, a transform convention, a hash), it moves to the toolkit with a gate and
both games call it. A game crate ends up mostly composition: format specifics plus calls into the toolkit. Likewise a
CLI is a few dozen lines of reader + passes + writer; if it needs more than piping, the missing piece belongs in a
crate. Dependencies point down only (`worlds/formats -> toolkit -> core`); world crates never
depend on each other or on formats, and formats depend on no format but `formats/gltf` (CONTRIBUTING.md). `TexFormat` lives in core (a `Texture` component holds one); its codecs live in `toolkit/texcodec`. Crates are
two-way where it makes sense: a `reskate` reader lets a skate. map round-trip or go to Blender.

**Scope of this repository.** It holds library crates, thin CLIs and small examples only. Applications that consume
primase output (viewers, editors, services) live in their own repositories and depend on documented output formats,
not on this repository's internals.

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

**The web writer** is the glTF writer plus a web lowering: KTX2 (Basis Universal) textures, meshopt-compressed and
quantised vertices (`EXT_meshopt_compression`), prototypes and placements as `EXT_mesh_gpu_instancing` (three.js
`InstancedMesh`), baked PBR as `MeshStandardMaterial`, one GLB per 200 m tile with a distance-based tile loader,
collision soups for `@react-three/rapier`, grind curves, spawns and markers as glTF extras.

**The plotter writer** (later) hands a world to a hidden-line renderer to draw it as pen-plotter lines: silhouette and
crease edges picked by dihedral angle, optional hatching from baked colour or normals, per-material pens.

## 6. Testing (gates are the product)
- **Decoder gates:** a decoder against an independent decoder or fixtures, over synthetic data and the real data the
  change affects; exact where the format defines it, else a stated tolerance.
- **Reader census:** a reader can run over all assets of its kind in a game and report decode failures by count, when
  a change needs that breadth; it is not run for every change.
- **Writer round trips:** write -> read back -> compare by meaning; omitted features are reported.
- **World validators:** identity rules ("texture name -> one image", "EBX name -> one partition"), frame sanity,
  non-finite and degenerate checks.

Real data never enters the repository: tests read installs from documented environment variables and skip when they
are unset (CONTRIBUTING.md).

## 7. Roadmap

**v1.** Readers: Forza Horizon 6, a Unity game, Sekiro, GTA V (from existing scene-interchange folders first). Writers:
ReSkate (skate. maps), Blender via glTF (materials, collections, and the
un-lowered material data kept in `extras.primase` next to the baked PBR), and three.js / react-three-fiber.

**After v1:** Sunset Overdrive, Sonic Adventure 2, STL/3MF, the plotter writer, and rich Blender output: the games'
real material setups (layered materials, shading recipes, spec/normal workflows) rebuilt as Blender node trees by a
Blender-side importer reading our glTF plus a materials extension.
Also after v1: a **native skate. level writer** that writes the game's own level data directly instead of going
through ReSkate Studio: first test fixtures, deterministic identities and a round-trip harness, then replacing one
mesh in a small test level and loading it in game.

Order of work: M0 core + scene-interchange read/write; M1 glTF; M2 the toolkit pieces with their gates (texcodec, dds,
image, compress, hash, vertex, meshops); M3 the ReSkate writer; M4+ world readers one at a time, each
differential-tested against the tool it replaces.

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
