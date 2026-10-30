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

The decoders meet real data in the readers' gates: `worlds/unity/tests/texture.rs`
decodes a Unity player's textures against bcdec (bit for bit) and UnityPy's
images, and `worlds/anvil/tests/texture.rs` against RRAssetStudio's decodes.
`worlds/fromsoft/tests/tpf_pixels.rs` compares Sekiro's texture packages with
bcdec, C# and Pillow 11 when the declared inputs are configured (it normalizes Pillow's
grayscale BC4 to red-only).

`Encoder` (behind `TexEncoder::encode_rgba8`) encodes BC7 only and refuses every other format; it encodes straight RGBA8 as BC7 through `block_compression` (pure-Rust CPU port of Intel's
ISPC kernels, all modes; slow settings, alpha fitted only when some texel has alpha < 255). Partial edge blocks are
padded by repeating the edge texels. No native code, no CPU dispatch: same input, same bytes.

## Example

```rust
use primase_core::TexFormat;
use primase_texcodec::{Decoder, TexDecoder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // One BC1 block: red endpoint, black endpoint, every texel choosing red.
    let pixels = Decoder::default().decode_rgba8(TexFormat::Bc1, 4, 4, &[0, 248, 0, 0, 0, 0, 0, 0])?;
    assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    Ok(())
}
```

This example needs no game files.
