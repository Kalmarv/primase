# primase

A compiler-style library for converting 3D game worlds. Programs that use it compose its readers, passes and
writers and make the conversion's choices; a small command line is there for trying it.

Instead of writing N x M point-to-point exporters, **readers** lift a game or file format into a shared
entity-component [`World`](core/src/world.rs), **passes** lower and normalise dialects step by step, and **writers**
serialise the result. Named after the enzyme that lays down the starting primer every new DNA strand is built from.

```text
  Unity / GTA / FromSoft / glTF / skate-scene
                       │
                   [ Reader ]
                       ▼
        World (source & core dialects)
                       │
           [ PassManager + Target ]   ◄── plans lowering, verifies invariants,
                       ▼                  tracks losses & state/LOD axes
        World (legalised for backend)
                       │
                   [ Writer ]
                       ▼
   ReSkate / glTF / Three.js web tiles / skate-scene
   (later: USD, STL, pen-plotter drawings)
```

Status: being built. Design: [`docs/DESIGN.md`](docs/DESIGN.md).

## Architecture

The core design is borrowed from **MLIR**:

* **One container, many dialects (`core`)**: a `World` is a deterministic, entity-ordered ECS store. Engine-specific
  data (`unity.*`, `gta.*`, `skate.*`) lives alongside shared dialects (`core.*`, `pbr.*`) on the same entities, so
  lowering happens progressively instead of all at once in the reader.
* **Declarative passes and target legalisation (`core::pass`)**: every pass declares the components it `requires`,
  `provides`, `removes` and `modifies`, and says in words what it `approximates`. A writer declares a `Target` that
  gives each component name a legality (`Legal`, `Dynamic`, `Illegal`, `Carry`). `PassManager` plans the lowering
  pipeline from those declarations, checks the component invariants between passes, and in `Mode::Full` refuses to
  write a name the target neither accepts nor carries. Approximations are counted in the run report.
* **First-class world semantics**: coordinate frames (`SourceFrame`), discrete LOD sets (`core.lods`), conditional
  world states (`core.state_axis`, `core.state_override`), layered material stacks (`core.layered`) and collision
  (`core.collision`) are modelled in the world and resolved by dedicated passes.
* **Determinism**: iteration order is creation order (`Entity(u32)` keys in `BTreeMap`s), and the same input and
  pipeline give the same bytes. Std `HashMap` and `HashSet` are being removed from library code. `WorldPrinter`
  prints the world around every pass, to a sink the caller supplies, for diffing and debugging.
* **Enforced boundaries**: an architecture test (`cli/tests/architecture.rs`, exceptions in `architecture.toml`)
  checks crate layering, keeps game names out of `core`, `toolkit` and `formats`, and requires every library module
  that touches files, the environment or processes to declare itself a reader or writer (clippy `clippy.toml`
  bans those calls elsewhere). Writers only read the `&World` they are given.

## Workspace layout

| Directory | Role |
| :--- | :--- |
| [`core/`](core/) | The `World`, core/PBR/LOD/state/layered components, `PassManager`, target legalisation and the world printer. |
| [`toolkit/`](toolkit/) | Format and math primitives with no game knowledge: `bytes`, `compress`, `hash`, `dds`, `texcodec`, `image` (BCn/DDS/PNG textures), `meshops` and `vertex` (mesh and UV operations), `dxbc` (an SM5 shader interpreter), `havok`, `frostbite`, and `testdata` (real-data test configuration). |
| [`formats/`](formats/) | Interchange readers and writers: `gltf`, `skate-scene` and `web` (tiled Three.js worlds with KTX2, meshopt and GPU instancing, for a web viewer). |
| [`worlds/`](worlds/) | Per-game readers, dialect lowerings and builders: `fromsoft` (Sekiro), `unity`, `gta` (GTA V) and `reskate` (*skate.*). |
| [`cli/`](cli/) | The `primase` binary (`primase convert`, for trying a reader, passes and a writer together) and the architecture test. |

## Getting started

The library is used from Rust. Worked compositions of a reader, passes and a writer are the examples:

```sh
# Export a skate-scene folder as web tiles
cargo run --release -p primase-web --example scene_to_tiles -- path/to/scene path/to/out

# Export a Unity level (needs the game's data folder and a type-tree package)
cargo run --release -p primase-unity --example unity_export -- <Game>_Data typetrees.tpk level1 path/to/out --web

# Run the test suite (includes the architecture checks)
cargo test --workspace
```

Local game installs and data dumps are configured outside the repo: copy
[`primase-data.example.toml`](primase-data.example.toml) to `primase-data.toml`, point `PRIMASE_DATA` at it and
see the real-data section of [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Documentation

* [`docs/DESIGN.md`](docs/DESIGN.md): architecture, dialect model, pass rules and workspace invariants.
* [`docs/SPEC.md`](docs/SPEC.md): the columnar World Format v2 (`.pwc`).
* [`docs/research/MLIR.md`](docs/research/MLIR.md): how MLIR's concepts (operations, conversion targets, pass
  manager, text printing) map to primase.
* [`CONTRIBUTING.md`](CONTRIBUTING.md): the model (read it first), the reader, pass, writer and program roles, checks
  and real-data setup.

## Licence

[GPL-3.0-or-later](LICENSE)
