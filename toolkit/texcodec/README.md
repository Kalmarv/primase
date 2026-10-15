# Texture codecs

`TexDecoder` decodes one tightly packed level to RGBA8 or RGBA32F. `Decoder`
implements BC1-5 and uncompressed formats, delegating BC6H/BC7 to bcdec_rs.
`Bcdec` is a separate adapter for cross-checks. Unknown formats, mismatched input
lengths, overflowing dimensions and allocation failures return errors.

Pixels are row-major, top first, straight RGBA, with no colour-space transfer.
Missing colour channels are zero, missing alpha is one. BC4 is red-only.
Signed formats remain signed in float output; byte output biases present signed
channels to [0,255], rounding ties to even. HDR float output is not clamped;
byte output clamps to [0,1].

The decoder uses D3D/bcdec arithmetic: packed-endpoint colour interpolation and
rounded BC4/5 weights. Independent C# and Pillow values can differ by one channel
level; those gates count exact and within-one results and print distributions.
The independent bcdec adapter remains an exact gate for BC1-5. There are no
alternate library arithmetic modes.
BC1 three-colour index 3 and reserved BC7 tags decode to transparent black.
Signed endpoints clamp before mode selection.

Run `cargo test -p primase-texcodec -- --nocapture`. Synthetic fixtures cover
BC1 transparency, uncompressed channels, signed endpoint extremes, and all
BC6H/BC7 modes. Python generators in `tests/oracles` require Pillow 11.0.0.
The native BC6H generator uses the public [bcdec](https://github.com/iOrange/bcdec)
header, SHA256 `867d2f6b13ec64c9fd807e3c7709c113feeff2a67d30d9ee333895cabb2ee7db`.
Compile `native.c` with that header on the include path, run `pillow.py`, then
`modes.py <native-executable>`; all generated data is synthetic.
The C# golden outputs are independent reference-decoder fixtures.

Real-data gates use `PRIMASE_SCENES_GTA` and `PRIMASE_SCENES_SEKIRO` only,
recursively finding DDS beneath `textures` directories and comparing level zero.
Pillow's grayscale presentation of BC4 is normalized to red-only; sRGB DXGI tags
are relabeled for Pillow 11 without altering payloads or applying gamma.

Censuses deduplicate input files by SHA256 within each configured root, reporting
path counts separately from checked distinct contents. All paths are read and hashed.
