# primase architecture

How primase is arranged, what sits under it, and how a conversion runs. The rules themselves are in
[CONTRIBUTING](../CONTRIBUTING.md), which wins where this page disagrees; the design notes are in [DESIGN](DESIGN.md).

```text
read → World (shared + namespaced data) → selected passes → write → later stages
```

Readers read supported source information faithfully; they do not bake it for a destination. Passes select and
transform that information. Writers encode prepared data they support, report what they omit, and reject invalid
data that prevents valid output. Compilation, stitching and other later stages remain explicit operations.

## Layers

| Layer | What belongs here |
| --- | --- |
| [`core`](../core/) | Shared meanings: geometry, materials, lights, collision and world state; what a shader program's inputs, constants and outputs stand for (`core::shader`); the `World` store, texture forms, irradiance volumes, auto exposure and pass/conversion machinery. |
| [`toolkit`](../toolkit/) | Reusable mechanisms, independent of a particular game or destination: texture codecs, DDS, images, mesh operations, animation, lighting, the GLB codec, Havok lowering, a disk cache. |
| [`formats`](../formats/) | Formats that are not one game's or destination's: their readers and writers. |
| [`worlds`](../worlds/) | Game, engine-family and destination knowledge: source readers, namespaced data and domain passes/writers. |
| [`examples`](../examples/) | Programs using primase as a library. primase has no command line: a program composes readers, passes and writers itself. |
| [`checks`](../checks/) | The architecture rules' tests and tests spanning crates. |

A game crate may depend on its engine's crate, never the reverse. Each worlds crate names only its own game,
engine or destination. Reusable destination preparation belongs alongside that destination's writer, as separately
selected passes. Complete conversion programs live in consumer repositories and own their recipes and choices.
Remaining boundary exceptions are tracked in [`architecture.toml`](../architecture.toml).

## The crates under it

How a game stores its data is not primase's: the readers of game and engine formats, the shader lifter, and the
plumbing under them are libraries in their own repositories, so they can be used without primase. They use none of
primase's crates; primase's worlds crates read through them and turn what they read into the world.

| Repository | Crates | What |
| --- | --- | --- |
| [iridia](https://github.com/Kalmarv/iridia) | `iridia`, `iridia-interface`, `iridia-dxbc`, `iridia-dxil`, `iridia-d3d9`, `iridia-mojo`, `iridia-exec` | Compiled shaders (DXBC, DXIL, Direct3D 9, Mojo) lifted to one program form, run on the CPU, and read for what their code shows |
| [dx](https://github.com/Kalmarv/dx) | `dxcontainer`, `dxbc-reader`, `dxil-reader`, `rsx-reader` | Direct3D shader bytecode (the container, DXBC and DXIL, with their reflection), and the PlayStation 3's RSX textures and decoded vertex/fragment programs |
| [fromsoft-formats](https://github.com/Kalmarv/fromsoft-formats), [rage-engine-formats](https://github.com/Kalmarv/rage-engine-formats), [frostbite-formats](https://github.com/Kalmarv/frostbite-formats), [anvil-formats](https://github.com/Kalmarv/anvil-formats), [unity-formats](https://github.com/Kalmarv/unity-formats), [monogame-formats](https://github.com/Kalmarv/monogame-formats), [rme-formats](https://github.com/Kalmarv/rme-formats), [gbx-formats](https://github.com/Kalmarv/gbx-formats), [forzatech-formats](https://github.com/Kalmarv/forzatech-formats) | one per engine | An engine's archives and file formats |
| [havok-formats](https://github.com/Kalmarv/havok-formats), [havok-cloth](https://github.com/Kalmarv/havok-cloth), [morpheme-formats](https://github.com/Kalmarv/morpheme-formats), [lua51-bytecode](https://github.com/Kalmarv/lua51-bytecode) | one each | Middleware formats and cloth simulation several engines share |
| [primase-foundations](https://github.com/Kalmarv/primase-foundations) | `primase-foundations` | Checked byte reads, game-file hashes, vertex decoding, texture sampling, and the compression codecs game files use |
| [primase-naga](https://github.com/Kalmarv/primase-naga) | `primase-naga` | naga with the changes primase needs that upstream has not released |
| [primase-testdata](https://github.com/Kalmarv/primase-testdata) | `primase-testdata` | The real-data test loader and input manifests, a dev-dependency only |

Most of these are also published on crates.io; gbx-formats and forzatech-formats are not. primase depends on each at
one git commit (`primase-foundations` from crates.io), and every repository that shares one names the same commit
([CONTRIBUTING](../CONTRIBUTING.md#crates-in-their-own-repositories)).

The line between iridia and primase is the line between what a program is and what it means. iridia reports what a
shader's code shows: its interface, Direct3D's fixed-function state, whether a matrix constant is read by rows or
columns, which output lanes a discard tests or a blend weighs by. What those stand for (a view-projection matrix, an
albedo, a normal stored octahedrally) is primase's to say, in `core::shader`, together with the one reading of a pixel
program's outputs as a lit surface and the WGSL a GPU composite decodes them with.

## What follows from the model

- The program chooses what gets produced. Region, world state, quality, baking, collision generation and placement
  are program choices. Library operations can still have documented algorithm defaults.
- One read can feed different destinations. Supported information stays in core or a source namespace until
  selected passes transform it. Writers do not need to know which reader supplied it.
- Losses are visible. Reports retain known approximations, exclusions and unsupported output, including per-part
  details. Matching an old tool's encoded bytes is not a general requirement.
- New code has a home. How a game stores its data goes in its engine's formats repository, shared concepts in core,
  neutral mechanisms in toolkit, domain knowledge in formats or worlds, and recipe choices in the consuming program.

## From a Sekiro area to a skate. map

[`examples/examples/sekiro_to_reskate.rs`](../examples/examples/sekiro_to_reskate.rs) is one program in five stages:

- **Read.** `AreaReader` reads an area of Sekiro as the game stores it: placements, models, materials and their
  layered shaders, lights, spawn points and collision. It decides nothing for a destination; what has no shared
  meaning stays in the `fromsoft` namespace.
- **Select.** Passes run once on the world as read and lower Sekiro's data to core meanings: collision by hit class,
  static meshes posed, the finest level of detail, layered materials baked to plain textures, materials, lights and
  spawn points.
- **Prepare.** Passes run on each part before it is written and prepare it for skate.: textures sampled at the
  albedo's tiling, raised to the skate dialect, clipped to the region, the floor placed, the spawn chosen, water, the
  pause map, textures capped and tinted, decal overlays stacked, blend chains ordered and budgeted, the collision laid
  out.
- **Write.** `MapSink` encodes the prepared part as a map folder for ReSkate Studio. What it leaves out or
  approximates is a notice in the report; a value it cannot encode is an error.
- **Compile.** `compile_map` runs ReSkate Studio on the folder, then finishes the mod against skate.'s data with its
  version-9 Oodle library; its result includes the post-compile report.

`convert` drives the first four: it reads, runs the selection, loads the region, runs its preparation, checks it
against what the writer writes, and writes it. A larger map is converted in parts (`convert_parts`) and written as the
whole map would be. Every choice is a typed value, an enum or a checked number, so a choice that is not valid fails
while the pipeline is built, before anything is read.

A complete program adds passes of its own for what it chooses beyond the library: ledges chained into grind rails,
travel markers named for players with the spawn among them, collision added where the game has none under a rendered
surface, foliage drawn as plain surfaces, and markers snapped down to the ground.
