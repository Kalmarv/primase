# Image operations

`read_png` returns a straight RGBA `Image` with either `Samples::U8` or
`Samples::U16`; 16-bit samples retain their full precision. Grayscale, RGB,
grayscale-alpha, RGBA and indexed PNG inputs are supported, including packed
low-bit-depth samples and tRNS transparency. `write_png` emits deterministic
RGBA PNGs at the image's bit depth.

`read_jpeg` returns a `Jpeg`: straight RGBA8 (alpha 255) of a baseline, extended
sequential or progressive Huffman-coded 8-bit grey, YCbCr or RGB file, through the
pure-Rust `zune-jpeg` decoder built without its SIMD features, so every machine
decodes to the same bytes. Strict: a file that departs from the standard is an error;
CMYK, YCCK, 12-bit, arithmetic-coded and lossless files are refused by name. EXIF
(orientation included), ICC and XMP are not applied and are listed in `Jpeg::ignored`.
The pixels are this decoder's integer IDCT and upsampling, not libjpeg's: against
Pillow 11 (libjpeg) the Khronos samples' JPEGs differ by at most 3 levels on 0.2 to
0.3 percent of the samples (`tests/jpeg.rs`).

No colour transfer is applied implicitly. PNG ancillary chunks not represented
in the image are returned by name in `Png::ignored_ancillary`. Animation is
rejected. Chunk framing, CRCs, palette indices, image lengths and trailing bytes
are checked. The public `Image` fields are validated before writing/filtering.

`srgb_to_linear` and `linear_to_srgb` implement the piecewise sRGB transfer in
f64, without clamping. `half` computes rounded integer means in stored channel
space, including straight alpha. Odd dimensions retain every source pixel:
each target pixel covers `[x*w/w', (x+1)*w/w')` along each axis. `mip_chain`
includes the original and ends at 1x1. `pick_level` selects the first available
level under the cap, or the smallest stored level.

Run `cargo test -p primase-image -- --nocapture`. Synthetic tests compare PNGs
against independently constructed source samples and Pillow 11, and mip levels
against reference C# golden outputs. `tests/oracles/png.py` regenerates synthetic
PNG fixtures. No external assets are included.

Real PNGs are checked where they are read: `formats/gltf/tests/textures.rs`
compares the Khronos sample assets' PNGs with Pillow 11's RGBA8 (and their JPEGs within 3 levels), and
`checks/tests/web_texture_decode.rs` round-trips every texture of a real scene
through the PNG writer. PNG compressed-byte identity with other encoders is not
promised: the contract is exact pixel roundtrip and deterministic output.

`load_textures`, a reader's load step, loads PNG and DDS file components atomically,
retaining file provenance and stored mip levels. Absolute paths are used directly;
relative paths resolve against the optional base directory. Existing textures take
priority. PNG samples retain 8/16-bit precision and default to sRGB; semantic
lowerings assign role and normal convention. No colour transfer is performed.

`texture_level_png` decodes a selected level through the texture decoder and emits
deterministic RGBA8 PNG, including the decoder's clamped projection for HDR data.
`NormalizeNormals` / `normalize_normals` explicitly convert normal maps to green-up
and stored positive-hemisphere Z across every mip. Green uses `255-G`; reconstructed
Z uses `sqrt(max(0,1-x*x-y*y))` and nearest byte rounding. Changed textures become
RGBA8 and lose their file component; unchanged textures retain it. Missing normal
metadata fails before mutation. The pass log records reconstruction and decoding.
`write_png_rgb(width, height, rgb)` emits 8-bit RGB (PNG colour type 2) for
textures without alpha. Its input must contain exactly width*height*3 samples;
`write_png` continues to preserve RGBA8/16 input. Neither applies colour transfer.

Normal-map filtering is explicit: `resize_normals` and `normal_mip_chain`
normalize RGBA8 source vectors, average in vector space and renormalize. Means
shorter than sqrt(3)/255 become flat +Z; the returned count records each such
replacement. Alpha is averaged separately and no sRGB transfer is applied.
Generic `half`/`mip_chain` continue to filter stored channel values; callers
with normal-map semantics must use the normal-specific functions.

`PackOrm` (`pack-orm{size=metallic-roughness|occlusion,missing-levels=generate|refuse}`,
both required) packs a material's separate occlusion and metallic-roughness maps into
one linear RGBA8 image, glTF's ORM layout (R occlusion, G roughness, B metallic, A
255), and points both slots at it; materials sharing a pair share the image. The
slots must sample alike (coordinate set, wrap, transform) and both maps be linear,
or the pass refuses. Every stored level is decoded through `texture_level_rgba8`.
The image takes the size and stored levels of the map `size` names; the other map's
level of another size is resampled nearest (counted), and a level it does not store
is box-filtered from its full level with `mip_chain` (`generate`, counted) or refused
(`refuse`): occlusion is never replaced by white.

## Example

```rust
use primase_image::{Image, Samples, read_png, write_png};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let image = Image { width: 1, height: 1, samples: Samples::U8(vec![255, 0, 0, 255]) };
    let png = write_png(&image)?;
    assert_eq!(read_png(&png)?.image, image);
    Ok(())
}
```

This example needs no game files.
