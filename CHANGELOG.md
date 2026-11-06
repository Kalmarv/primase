# Changelog

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

## 0.1.0 (2026-11-06)

- First release: the 27 libraries on crates.io (`primase-core`, the format crates and the toolkit). The
  [GitHub Release](https://github.com/Kalmarv/primase/releases/tag/v0.1.0) lists the commits up to it.
