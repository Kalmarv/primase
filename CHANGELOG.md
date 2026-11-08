# Changelog

## Unreleased

- **Breaking:** iridia 0.6.0 by git revision of its main until its next release (was 0.5.0) and the dx readers 0.12.0 (`dxbc-reader`,
  `dxil-reader`, `dxcontainer`, `rsx-reader`; were 0.5.0), with `anvil-formats` 0.2.1 and `rme-formats` 0.5.0 (the versions primase already used) rebuilt on the same dx by git revision of
  their `dx-0.12-on-0.2.1` and `dx-0.12-on-0.5.0` branches, only the dx dependency changed, so the build links one version of each dx reader. Public fields and functions that carry iridia's types
  (`LiftedShader`, the derivation and specialisation programs, `iridia_interface` render states and address modes, `iridia_exec::ProgramShader`
  in the Sekiro bake) now carry iridia 0.6's.
- **Breaking:** the shader programs primase holds, derives and specialises are iridia's IR (`iridia_ir::Program`), not the naga-module `iridia::Program`:
  `LiftedShader`, `Derivation`, `Specialised`, the blend and input-fallback analyses of primase-core, and the shader passes of
  primase-unity, -monogame, -frostbite, -stuntboost, -anvil and -fromsoft lift with `primase_core::shader::{lift_dxbc, lift_dxil}` (and their
  `_with` forms), which lift and prepare the program (`iridia_passes::prepare`; a test keeps every source on them; monogame prepares its `iridia_mojo::ir::lift` the same way), take value graphs and constant reads from `iridia_analysis`, specialise with `iridia_passes` and run on
  `iridia_exec`'s `from_ir` executors. The Sekiro material bake's cache key carries the IR lifters' versions (cached bakes rebuild once). The
  census tests write WGSL with `iridia_naga` (its writer's refusals are counted by reason). primase-gta and primase-shaunwhite are not on the IR yet.

## 0.2.0 (2026-11-06)

- **Breaking:** primase-unity reads with unity-formats 0.5.0. Textures of every format it names become a world texture
  (RGB24, its signed form and RGB48 widened with an opaque alpha, ARGB32 and RGBA4444 reordered to RGBA8, RGB9e5 decoded to
  RGBA32 floats, the rest kept as stored), and crunched DXT1 and DXT5 textures unpack to their blocks; before, only 11
  formats were read and a crunched texture was refused. A format with no world format (YUY2, ATC, signed EAC, HDR ASTC, R16
  and RGB48 signed) and an empty texture (a dynamic font's) are refused by name.
- **Breaking:** legacy AnimationClips lower like Mecanim ones: their keyframe curves are Hermite channels, a weighted curve
  is sampled at `clipdata::WEIGHTED_STEPS` keys a stretch with the largest difference halfway between keys reported
  (`ClipLowering::weighted_channels`, `ClipLowering::worst_weighted_error`, `LevelReport::worst_weighted_curve_error`), and
  a ping-pong clip is unrolled into a loop of twice its length (`ClipLowering::ping_pong_unrolled`). Object-reference (PPtr)
  curves, a material or sprite swap, are left out by reason. Before, those clips were refused.
- primase-stuntboost's gates run on STUNTBOOST build 26429886 (primase-testdata 0.1.15, the game's own dump of it in
  `oracles.game-code.stuntboost/levels-build26429886`). The build changes two levels: StairsSlide_1's room-bounds collision
  mesh grows from 68 to 80 triangles (one ledge and two ledge corners more) and BoosterLaunching_0's checkpoints are
  `Custom` where they were `Order`; the gates read both as the game does.
- Release: dev-dependencies on other crates of the workspace that are not among a crate's normal dependencies are
  paths with no version (primase-frostbite on primase-texcodec, primase-reskate and primase-unity on primase-gltf,
  primase-texcodec on primase-dds), so publishing never looks for them on crates.io; `release-plz` publishes in the
  order of normal dependencies only, and 0.2.0's first run stopped at primase-texcodec for want of primase-dds 0.2.0. An
  architecture test keeps it so.

## 0.1.0 (2026-11-06)

- First release: the 27 libraries on crates.io (`primase-core`, the format crates and the toolkit). The
  [GitHub Release](https://github.com/Kalmarv/primase/releases/tag/v0.1.0) lists the commits up to it.
