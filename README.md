# primase

[![crates.io: primase-core](https://img.shields.io/crates/v/primase-core?label=primase-core)](https://crates.io/crates/primase-core) [![crates.io: primase-gltf](https://img.shields.io/crates/v/primase-gltf?label=primase-gltf)](https://crates.io/crates/primase-gltf) [![crates.io: primase-reskate](https://img.shields.io/crates/v/primase-reskate?label=primase-reskate)](https://crates.io/crates/primase-reskate) [![docs.rs](https://img.shields.io/docsrs/primase-core)](https://docs.rs/primase-core) [![CI](https://img.shields.io/github/actions/workflow/status/Kalmarv/primase/ci.yml?branch=main&label=CI)](https://github.com/Kalmarv/primase/actions/workflows/ci.yml)

A Rust library for reading 3D game worlds into a shared in-memory world, transforming them with passes, and writing them out as glTF, Blender scenes or skate. maps. It has no command line: a program composes the readers, passes and writers it needs.

## Features

- Reads Sekiro: Shadows Die Twice and Elden Ring areas and characters
- Reads Grand Theft Auto V regions from an install, with caller-supplied archive keys
- Reads Riders Republic and Shaun White Skateboarding (PC) data from Anvil forge archives
- Reads Unity players' data files, with extra readers for SkateBIRD and PEAK
- Reads STUNTBOOST levels and lifts MonoGame effect programs
- Reads Tony Hawk: SHRED (PS3), Forza Horizon 6 tracks and TrackMania Nations Forever maps
- Reads Frostbite data folders as skate. uses them
- Reads and writes glTF 2.0 and GLB, using standard glTF and Khronos extensions only
- Writes map folders for ReSkate Studio and runs its compile and post-compile steps
- Writes Blender scenes whose materials are Cycles shader node graphs, saves them as `.blend` and renders them
- Provides toolkit crates for BCn textures, DDS, PNG and JPEG, mesh operations, animation, Havok data and lighting
- Converts large maps in parts, one part in memory at a time
- Runs pipelines as stated: the pass manager never adds, removes or reorders a pass
- Reports every omission, approximation and refusal as a notice in the conversion report
- Produces the same bytes for the same input, and denies `unsafe` code outside the loading of a game's Oodle library

## Getting Started

### Requirements

- Rust 1.99.0 (pinned in `rust-toolchain.toml`)
- Your own installs of the games you read, and their archive keys where a game needs them
- For the skate. stages: a skate. install and ReSkate Studio's command line (`reskate_cli.exe`)
- For the Blender stages: a Blender executable

On Windows, archives compressed with Oodle are read with the game's own `oo2core` DLL, loaded at run time from the install. Elsewhere `oozextract` decodes them.

### Installation

Every package below is published on crates.io, all at one version:

```sh
cargo add primase-core primase-meshops
```

| Package | What it does |
| --- | --- |
| `primase-core` | The `World` store, core components, frames, the pass manager and the conversion loop |
| `primase-texcodec` | BC1 to BC7 texture decoding and encoding behind one `TexFormat` |
| `primase-dds` | DDS read and write in every layout: arrays, cubes, volumes, legacy and DX10 headers |
| `primase-image` | PNG and JPEG pixels, sRGB transfer, mip chains and texture passes |
| `primase-meshops` | Mesh operations, tessellation and mesh passes |
| `primase-animation` | Clip evaluation, poses, morphs, skinning and retargeting |
| `primase-lighting` | Ambient light as fill lights, emissive surfaces as lights |
| `primase-glb` | The GLB container, a strict GLB reader and a mesh encoder |
| `primase-havok` | Havok convex shapes, skeletons, animations and cloth as world data |
| `primase-exactsum` | An order-independent exact sum of `f64` values |
| `primase-cache` | A content-addressed disk cache for expensive pure steps |
| `primase-gltf` | glTF 2.0 / GLB reader and writer |
| `primase-fromsoft` | Sekiro and Elden Ring |
| `primase-gta` | Grand Theft Auto V |
| `primase-anvil` | Anvil forge archives (Riders Republic) |
| `primase-shaunwhite` | Shaun White Skateboarding (PC) |
| `primase-unity` | Unity players' data files |
| `primase-skatebird` | SkateBIRD, over `primase-unity` |
| `primase-peak` | PEAK's script-set shader globals, over `primase-unity` |
| `primase-monogame` | MonoGame effect programs |
| `primase-stuntboost` | STUNTBOOST levels |
| `primase-shred` | Tony Hawk: SHRED (PS3) |
| `primase-forzatech` | Forza Horizon 6 tracks |
| `primase-trackmania` | TrackMania Nations Forever maps |
| `primase-frostbite` | Frostbite data folders |
| `primase-reskate` | The skate. destination: preparation passes, the ReSkate Studio map writer, compile and post-compile |
| `primase-blender` | The Blender destination: writer, `.blend` save and Cycles render |

The game and engine format readers, the iridia shader lifter and `primase-foundations` live in their own repositories and are crates.io dependencies at the versions the root `Cargo.toml` names. A program that uses one of them directly together with primase needs a version semver-compatible with primase's, so that cargo resolves the two to one crate and their types meet.

### Usage

A program reads with a reader, runs the passes it states, and writes with a writer. This function converts a world, turning curves into polylines within 1 cm:

```rust
use primase_core::convert::{convert, ConvertReport, Reader, Result, Writer};
use primase_core::pass::{PassManager, PassRegistry, Pipeline};
use primase_meshops::FlattenCurves;

fn export(reader: &mut dyn Reader, writer: Box<dyn Writer>) -> Result<ConvertReport> {
    let passes = Pipeline {
        passes: vec![Box::new(FlattenCurves::new(0.01)?)],
    };
    let select = Pipeline::default(); // no selection: keep everything that was read
    let manager = PassManager::new(PassRegistry::new());
    convert(reader, writer, &manager, &select, &passes)
}
```

The returned report's `notices()` lists what was left out or approximated. `convert_parts` converts a world in planned parts.

The `examples` package holds complete programs. Each states its paths and choices in the source:

```sh
# a Sekiro area to a skate. map, compiled by ReSkate Studio (paths are in the file)
cargo run --release -p primase-examples --example sekiro_to_reskate
# Forza Horizon 6's Shibuya Scramble to a skate. map; Studio compile when both optional paths are given
cargo run --release -p primase-examples --example fh6_to_reskate -- <ForzaHorizon6 install> [<skate install> <ReSkate CLI>]
# a skate. Quick Drop item cloned into an .fbmod
cargo run --release -p primase-examples --example quickdrop_clone -- <skate. game folder> <out.fbmod>
# Elden Ring outfit and Stormveil Castle slice to GLB (install and keys from GAME_DATA)
cargo run --release -p primase-examples --example elden_outfit_gltf -- outfit.glb
cargo run --release -p primase-examples --example elden_stormveil_gltf -- stormveil.glb
```

Run the tests with `cargo nextest run`, and the architecture rules alone with `cargo test -p primase-checks --test architecture`. Real-data tests read their inputs from the TOML file `GAME_DATA` names and skip when it is unset; see [CONTRIBUTING](CONTRIBUTING.md) for that setup, [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [docs/DESIGN.md](docs/DESIGN.md) for the design, and each crate's README for its details.

Licensed under [GPL-3.0-or-later](LICENSE).
