# primase-dds

A Rust library that reads and writes DDS texture files for primase. It handles every layout Direct3D stores in one
under the legacy or the DX10 header, and keeps the encoded images as stored.

## Features

- Reads and writes 1D, 2D, cube and volume textures, arrays of 1D and 2D textures, and cube arrays, with their mips
- Reads legacy headers by FourCC or by channel masks, and DX10 headers by DXGI format
- Writes the header bytes texconv writes, under texconv's choice of header or a chosen one (`WriteHeader`)
- Keeps DX10 sRGB tags and alpha modes; reads premultiplied DXT2 and DXT4 as BC2 and BC3 with that alpha mode
- Reads DX10 cubes whose `arraySize` counts faces rather than cubes, as FromSoftware's games write them
- Converts to and from `primase_core::Texture` (one image) and `TextureLayers` (arrays, cubes, volumes)
- Maps `TexFormat` and colour space to and from DXGI format numbers (`to_dxgi`, `from_dxgi`)
- Builds for `wasm32-unknown-unknown`; no native code
- Refuses what it cannot keep exactly, by name: a payload of the wrong size, typeless formats, unknown flags

## Getting Started

### Installation

The crate is part of primase and is published on crates.io with it:

```sh
cargo add primase-core primase-dds
```

### Usage

```rust
use primase_core::{ColorSpace, TexFormat};
use primase_dds::{AlphaMode, Dds, HeaderKind, Shape, WriteHeader, parse, write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // one 8x8 BC1 cube with two levels: six faces of 4 blocks, then six faces of 1 block
    let cube = Dds {
        shape: Shape::Cube { cubes: 1 },
        width: 8,
        height: 8,
        format: TexFormat::Bc1,
        color_space: ColorSpace::Srgb,
        alpha_mode: AlphaMode::Unknown,
        levels: vec![vec![vec![0; 32]; 6], vec![vec![0; 8]; 6]],
    };
    let bytes = write(&cube, WriteHeader::Auto)?;
    let read = parse(&bytes)?;
    // a legacy header can't state sRGB, so texconv's choice (and this one) is DX10
    assert_eq!(read.header, HeaderKind::Dx10);
    assert_eq!(read.dds, cube);
    Ok(())
}
```

`read` and `write_dx10` (or `write_legacy`) take a single 2D `Texture`; `read_layers` and `write_layers` take any
shape. This example needs no game files.

A header means what it means to DirectXTex (Microsoft's reference, may2026), whose `DecodeDDSHeader` and
`EncodeDDSHeader` the reader and writer follow field by field. They differ from it on purpose here:

- The payload must be exactly the layout's size; DirectXTex accepts trailing bytes.
- Typeless DXGI formats and DX10 misc flags other than the cube flag are refused.
- Legacy formats that DirectXTex converts as it loads are refused with what it would convert them to: 24-bit RGB,
  A8R3G3B2, R3G3B2, the palettized P8 and A8P8, A4L4, L6V5U5, X8L8V8U8, A2W10V10U10, and A2R10G10B10 (which it reads
  with red and blue swapped).
- DX10 cubes whose `arraySize` counts faces are read; DirectXTex reads them only with `DDS_FLAGS_PERMISSIVE`.
  `Parsed::faces_counted` says when a file was one.
- A legacy pixel format with the FourCC flag and no FourCC, which DirectXTex refuses, reads as its channel masks say.
- Legacy files read as linear, as there is no tag, except where an NVIDIA Texture Tools header sets its sRGB flag.
- `WriteHeader::Auto` writes DX10 for a lone 1D texture and for an alpha mode, which texconv's default header loses.

### Tests

```sh
cargo test -p primase-dds --test synthetic --test directxtex
```

`tests/fixtures/directxtex` holds 61 files that texconv and texassemble (DirectXTex may2026) wrote, one per layout,
header and format family, with what DirectXTex's library reads from each (`generate.py` makes them). All of them
read as DirectXTex reads them (330 checks of metadata and images). Each is written back byte for byte under the header
texconv chose. With `tools.directxtex_dump` configured ([tests/inputs](tests/inputs/README.md)), DirectXTex's library
also reads all 1,275 files the writer makes across shapes, formats, headers and alpha modes as this reader does.

On the games' own files (`tests/census.rs`, `worlds/fromsoft/tests/environment_dds.rs`):

| Files | Distinct | Read alike by DirectXTex | Notes |
| --- | --- | --- | --- |
| Sekiro textures (`census/dds`) | 813 | 813 | written back byte for byte under the same header, 79 of them legacy |
| Sekiro cubes and volumes | 10,083 | 10,083 | 2,232 BC6H cubes count faces (DirectXTex: permissive load) |
| Elden Ring environment textures | 218,353 | 2,820 distinct headers | 44,269 BC6H cubes count faces; 269 RGBA16F cubes do not |
| TrackMania Nations Forever | 42 | 40 | 7 legacy cubes; 2 R8G8B8 files refused here, converted by DirectXTex |

Licensed under GPL-3.0-or-later, as primase.
