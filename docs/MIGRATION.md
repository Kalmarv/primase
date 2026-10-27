# Moving to the split crates

The format readers and the plumbing under them left primase for their own repositories (CONTRIBUTING.md, "Crates in
their own repositories"). Code that used them through primase changes its dependencies and paths as below. Each new
crate is a git dependency; name the same `rev` primase's root `Cargo.toml` names, so the types meet.

## Packages that were renamed

| was | is now |
| --- | --- |
| `primase-bytes` | `primase-foundations` (`primase_foundations::bytes`) |
| `primase-hash` | `primase-foundations` (`primase_foundations::hash`) |
| `primase-vertex` | `primase-foundations` (`primase_foundations::vertex`) |
| `primase-compress` | `primase-foundations` with the `compress` feature (`primase_foundations::compress`) |
| `primase-dxcontainer` | `dxcontainer` (dx) |
| `primase-dxbc` | `dxbc-reader` (dx: the container, tokens and reflection) and `iridia-dxbc` (iridia: the lifter) |
| `primase-dxil` | `dxil-reader` (dx: the bitcode and the module) and `iridia-dxil` (iridia: the lifter) |
| `primase-rsx` | `rsx-reader` (dx) |
| `primase-d3d9` | `iridia-d3d9` (iridia) |
| `primase-mojoshader` | `iridia-mojo` (iridia) |
| `primase-shader` | `iridia` (iridia) |
| `primase-shader-exec` | `iridia-exec` (iridia) |
| `primase-lua51` | `lua51-bytecode` |
| `oozextract` (vendored) | `oozextract = "=0.5.7"` (crates.io) |
| `naga` (patched) | `naga = { package = "primase-naga", git = ... }` |

## Code that moved out of a crate that stays

| from | to |
| --- | --- |
| `primase_core::shader` (the program interface: `ProgramFacts`, bindings, render state, `D3dBinding`, ...) | `iridia_interface` (`primase_core::shader::ProgramFacts` is `iridia_interface::ProgramFacts<Entity>`) |
| `primase_core::parallel::Threads::of(world)` | `primase_core::parallel::threads(world)` (`Threads` is `primase_foundations::parallel::Threads`) |
| `primase_image`'s sRGB curves (`srgb_to_linear`, `linear_to_srgb`, ...) and `sampling` | `primase_foundations::color`, `primase_foundations::sampling` |
| the file readers of `primase-frostbite` | `frostbite-formats` |
| the file readers of `primase-gta` | `rage-engine-formats` |
| the file readers of `primase-anvil` and `primase-shaunwhite` | `anvil-formats` (Shaun White's under `anvil_formats::shaunwhite`) |
| the file readers of `primase-fromsoft` | `fromsoft-formats` |
| Havok's tagfile, packfile and animation reading of `primase-havok` | `havok-formats` |
| the file readers of `primase-unity` | `unity-formats` |
| the XNB and MGFX readers of `primase-monogame` | `monogame-formats` |
| SHRED's file readers of `primase-shred` | `rme-formats` (CellGcmTexture: `rsx-reader`) |
| `toolkit/morpheme` | `morpheme-formats` |

Keys and digests the readers check against are the caller's: `fromsoft_formats::Keys::from_folder` takes the digest of
Elden Ring's `regulation.key`, and `rage_engine_formats` takes `KeyDigests` for GTA V's.

Real-data tests read `GAME_DATA` (primase-testdata's TOML) instead of `PRIMASE_DATA`, and each crate declares its own
inputs in `tests/inputs/mod.rs`.
