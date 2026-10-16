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
6. **Deterministic.** The same inputs give byte-identical outputs.

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
| `Light` | a punctual light as glTF's `KHR_lights_punctual` defines it: point, spot (cones) or directional, linear colour, intensity in candela (lux for directional), optional range; placed by its transform, shining along its local -Z |
| `Collision`, `Surfaces` (resource) | collision geometry placed like a mesh: a triangle soup in the entity's space, one surface per triangle, surfaces named in the world's table (game properties stay in dialects) |
| `Texture` | pixels or encoded blocks + `TexFormat`, colour space, role, normal convention (X in red/alpha, green up/down, Z stored/rebuilt) |
| `Name`, `Source` | display name; provenance key (archetype, MSB part, asset path) for debugging and review links |
| `Bounds`, `Tile` | for streaming and chunked processing |
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
`fromsoft.layered_material` beside `pbr.material`), and a writer marks the source `Carry` (glTF: `extras.primase`).

**On disk:** the world serialises as "core + named component tables" (v2, `docs/SPEC.md`): a small manifest plus
little-endian binary tables, generalising the skate-scene v1 interchange format (a JSON manifest, binary tables and
GLB meshes) that existing front ends already write; `formats/skate-scene` reads and writes v1 unchanged.

## 4. Passes and lowering

A pass has a stable name, options with a text form, and declares the component names it **requires**, **provides**,
**removes** and **modifies** (changes in place), plus what it approximates. After each pass the manager checks that the
names that appeared are within `provides` and those that vanished within `removes`, and runs the **verifier** (on the
reader's output, after every pass, before the writer; always on). A writer declares a **target** (MLIR's conversion
target): per component name `Legal`, `Dynamic` (a per-value check: texture format, TRS-decomposable transform),
`Illegal` or `Carry` (written through untouched), and what it does with unknown names. Full conversion requires every
name Legal or Carry; partial conversion keeps unknown names per the target.

Pipelines are explicit lists in a text form (`load-texture-files,bake-fromsoft-layered{size=1024}`). The planner
builds one by MLIR's legalization depth: for each illegal name, the pass chain of least depth that removes it with legal
outputs (ties by priority, then name), plus the producers of what those passes require, ordered by data dependencies and
checked by simulating the declarations. It fails naming the missing link ("no pass lowers `fromsoft.layered_material` to
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

## 5. Crates

```
core/         world, core components, TexFormat, frame conversions, pass manager, gates, serialisation (v2)
toolkit/      texcodec (TexFormat decoders/encoders and gates), dds, image (PNG, sRGB, mips), compress (lz4,
              deflate/zlib, Frostbite CAS blocks, Oodle via the game's own DLL), container (TOC and archive helpers,
              version gates), hash, vertex (layouts, quantisation, strips), havok, meshops (weld, split, meshopt),
              validate (identity rules)
worlds/       one crate per game or engine: reskate (Frostbite CAS/TOC/EBX, map writer and reader), fromsoft, gta,
              forza, unity, insomniac, ...
formats/      skate-scene (interchange v1), gltf, web, usd, stl/3mf, obj, ...
bindings/     python (pyo3) and C# (csbindgen), so existing tools can call the toolkit
cli/          thin compositions: `primase extract sekiro --game <dir> --area m11_01 --to gltf out/`
```

**The crate rule.** Each game becomes its own crate (`worlds/sekiro`, `worlds/gta`, ...), and patterns that recur
across games become core or toolkit functions: the second time a game crate needs something another game already has
(a codec, a container walk, a vertex decode, a transform convention, a hash), it moves to the toolkit with a gate and
both games call it. A game crate ends up mostly composition: format specifics plus calls into the toolkit. Likewise a
CLI is a few dozen lines of reader + passes + writer; if it needs more than piping, the missing piece belongs in a
crate. Dependencies point down only (`cli -> worlds/formats -> toolkit -> core`); world crates never depend on each
other. `TexFormat` lives in core (a `Texture` component holds one); its codecs live in `toolkit/texcodec`. Crates are
two-way where it makes sense: a `reskate` reader lets a skate. map round-trip or go to Blender.

**Scope of this repository.** It holds library crates, thin CLIs and small examples only. Applications that consume
primase output (viewers, editors, services) live in their own repositories and depend on documented output formats,
not on this repository's internals.

**Dependencies: small, focused, maintained crates that do one job well; no umbrella crates.** Each sits behind our own
trait (`TexDecoder`, `Compressor`, ...), so swapping it is local, and each is held to the same gates as our code.
Versions are pinned and default features off.
- **In (by role):** BCn decode `bcdec_rs`, BCn encode `intel_tex_2` or `texpresso`, DDS header `ddsfile`, PNG `png`,
  LZ4 `lz4_flex`, deflate/zlib `flate2`, binary layouts `binrw`, casts `bytemuck`, f16 `half`, mesh optimisation
  `meshopt`, bindings `pyo3` and `csbindgen`. Oodle is never bundled: the game's own `oo2core` DLL is loaded at run time
  from the install the user points at, with `oozextract` only as a gate's second decoder. ETC/ASTC via
  `texture2ddecoder` only behind a gate.
- **Out:** umbrella libraries that pull in a whole domain for one function (`image`, `image_dds`), engine frameworks
  such as `bevy_ecs`.
- **Our own, deliberately:** the glTF writer, sRGB and mip filters, every game format.

**The web writer** is the glTF writer plus a web lowering: KTX2 (Basis Universal) textures, meshopt-compressed and
quantised vertices (`EXT_meshopt_compression`), prototypes and placements as `EXT_mesh_gpu_instancing` (three.js
`InstancedMesh`), baked PBR as `MeshStandardMaterial`, one GLB per 200 m tile with a distance-based tile loader,
collision soups for `@react-three/rapier`, grind curves, spawns and markers as glTF extras.

**The plotter writer** (later) hands a world to a hidden-line renderer to draw it as pen-plotter lines: silhouette and
crease edges picked by dihedral angle, optional hatching from baked colour or normals, per-material pens.

## 6. Testing (gates are the product)
- **Decoder gates:** every decoder against a second decoder or fixtures, over synthetic data and a sample of every
  game's real data; bit exact or a stated tolerance.
- **Reader census:** every reader runs over all assets of its kind in a game and reports decode failures by count.
- **Writer round trips:** write -> read back -> compare; the ReSkate writer is gated byte-identical against the
  existing reference writer.
- **World validators:** identity rules ("texture name -> one image", "EBX name -> one partition"), frame sanity,
  non-finite and degenerate checks.

Real data never enters the repository: tests read installs from documented environment variables and skip when they
are unset (CONTRIBUTING.md).

## 7. Roadmap

**v1.** Readers: Forza Horizon 6, a Unity game, Sekiro, GTA V (from existing scene-interchange folders first). Writers:
ReSkate (skate. maps, byte-identical to the reference writer), Blender via glTF (materials, collections, and the
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
