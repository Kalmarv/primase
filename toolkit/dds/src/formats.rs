//! The format tables: DXGI values, and the legacy (Direct3D 9) pixel formats as DirectXTex reads and writes them.
//!
//! The legacy table is DirectXTex's `g_LegacyDDSMap` (may2026, MIT), in its order and matched by its rule
//! ([`legacy_format`]), so a legacy header means here what it means to texconv. Where DirectXTex changes the payload
//! as it loads (swaps channels, forces alpha, expands to another format), a verbatim reader cannot give the same
//! answer: those entries are listed and refused by name instead of being misread.

use primase_core::{ColorSpace, TexFormat};

use crate::{AlphaMode, Error, Result};

// Typed DXGI values only: typeless, depth/stencil, integer and unknown layouts are errors.
const DXGI: &[(u32, TexFormat)] = &[
    (2, TexFormat::Rgba32Float),
    (10, TexFormat::Rgba16Float),
    (11, TexFormat::Rgba16Unorm),
    (16, TexFormat::Rg32Float),
    (24, TexFormat::Rgb10a2Unorm),
    (28, TexFormat::Rgba8Unorm),
    (31, TexFormat::Rgba8Snorm),
    (34, TexFormat::Rg16Float),
    (35, TexFormat::Rg16Unorm),
    (37, TexFormat::Rg16Snorm),
    (41, TexFormat::R32Float),
    (49, TexFormat::Rg8Unorm),
    (51, TexFormat::Rg8Snorm),
    (54, TexFormat::R16Float),
    (56, TexFormat::R16Unorm),
    (61, TexFormat::R8Unorm),
    (63, TexFormat::R8Snorm),
    (65, TexFormat::A8Unorm),
    (71, TexFormat::Bc1),
    (74, TexFormat::Bc2),
    (77, TexFormat::Bc3),
    (80, TexFormat::Bc4Unorm),
    (81, TexFormat::Bc4Snorm),
    (83, TexFormat::Bc5Unorm),
    (84, TexFormat::Bc5Snorm),
    (85, TexFormat::B5g6r5Unorm),
    (86, TexFormat::B5g5r5a1Unorm),
    (87, TexFormat::Bgra8Unorm),
    (88, TexFormat::Bgrx8Unorm),
    (95, TexFormat::Bc6hUfloat),
    (96, TexFormat::Bc6hSfloat),
    (98, TexFormat::Bc7),
    (115, TexFormat::B4g4r4a4Unorm),
];
const SRGB: &[(u32, TexFormat)] = &[
    (29, TexFormat::Rgba8Unorm),
    (72, TexFormat::Bc1),
    (75, TexFormat::Bc2),
    (78, TexFormat::Bc3),
    (91, TexFormat::Bgra8Unorm),
    (93, TexFormat::Bgrx8Unorm),
    (99, TexFormat::Bc7),
];

/// Map a typed DXGI format; typeless formats are deliberately not guessed.
pub fn from_dxgi(value: u32) -> Result<(TexFormat, ColorSpace)> {
    for (table, color) in [(DXGI, ColorSpace::Linear), (SRGB, ColorSpace::Srgb)] {
        if let Some((_, f)) = table.iter().find(|(n, _)| *n == value) {
            return Ok((*f, color));
        }
    }
    Err(Error::Unsupported(format!("DXGI format {value}")))
}

/// Map a format and colour space without dropping an sRGB tag.
pub fn to_dxgi(format: TexFormat, color: ColorSpace) -> Result<u32> {
    let table = if color == ColorSpace::Srgb { SRGB } else { DXGI };
    table
        .iter()
        .find(|(_, f)| *f == format)
        .map(|(n, _)| *n)
        .ok_or_else(|| Error::Unsupported(format!("DXGI mapping for {format:?} {color:?}")))
}

/// A legacy `DDS_PIXELFORMAT` without its size word (always 32).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelFormat {
    pub flags: u32,
    pub fourcc: [u8; 4],
    pub bits: u32,
    /// R, G, B, A.
    pub masks: [u32; 4],
}

pub const FOURCC: u32 = 0x4;
const RGB: u32 = 0x40;
const RGBA: u32 = 0x41;
const LUMINANCE: u32 = 0x2_0000;
const LUMINANCEA: u32 = 0x2_0001;
const ALPHAPIXELS: u32 = 0x1;
const ALPHA: u32 = 0x2;
const PAL8: u32 = 0x20;
const PAL8A: u32 = 0x21;
const BUMPLUMINANCE: u32 = 0x4_0000;
const BUMPDUDV: u32 = 0x8_0000;
const BUMPDUDVA: u32 = 0x8_0001;

const fn four(code: &[u8; 4]) -> PixelFormat {
    PixelFormat {
        flags: FOURCC,
        fourcc: *code,
        bits: 0,
        masks: [0; 4],
    }
}
/// A D3DFMT value used as the FourCC (D3DX's float and 16-bit formats).
const fn d3dfmt(value: u32) -> PixelFormat {
    four(&value.to_le_bytes())
}
const fn masks(flags: u32, bits: u32, masks: [u32; 4]) -> PixelFormat {
    PixelFormat {
        flags,
        fourcc: [0; 4],
        bits,
        masks,
    }
}

/// What a legacy entry reads as: a verbatim format, or a refusal naming what DirectXTex does to the bytes.
#[derive(Clone, Copy)]
enum Reads {
    As(TexFormat),
    /// DXT2 and DXT4: the BC2/BC3 blocks hold premultiplied colour.
    Premultiplied(TexFormat),
    Refused(&'static str),
}

const A8R8G8B8: PixelFormat = masks(RGBA, 32, [0xff_0000, 0xff00, 0xff, 0xff00_0000]);
const X8R8G8B8: PixelFormat = masks(RGB, 32, [0xff_0000, 0xff00, 0xff, 0]);
const A8B8G8R8: PixelFormat = masks(RGBA, 32, [0xff, 0xff00, 0xff_0000, 0xff00_0000]);
const G16R16: PixelFormat = masks(RGB, 32, [0xffff, 0xffff_0000, 0, 0]);
const A2B10G10R10: PixelFormat = masks(RGBA, 32, [0x3ff0_0000, 0xf_fc00, 0x3ff, 0xc000_0000]);
const R5G6B5: PixelFormat = masks(RGB, 16, [0xf800, 0x7e0, 0x1f, 0]);
const A1R5G5B5: PixelFormat = masks(RGBA, 16, [0x7c00, 0x3e0, 0x1f, 0x8000]);
const A4R4G4B4: PixelFormat = masks(RGBA, 16, [0xf00, 0xf0, 0xf, 0xf000]);
const L8: PixelFormat = masks(LUMINANCE, 8, [0xff, 0, 0, 0]);
const L16: PixelFormat = masks(LUMINANCE, 16, [0xffff, 0, 0, 0]);
const A8L8: PixelFormat = masks(LUMINANCEA, 16, [0xff, 0, 0, 0xff00]);
const A8: PixelFormat = masks(ALPHA, 8, [0, 0, 0, 0xff]);
const V8U8: PixelFormat = masks(BUMPDUDV, 16, [0xff, 0xff00, 0, 0]);
const Q8W8V8U8: PixelFormat = masks(BUMPDUDV, 32, [0xff, 0xff00, 0xff_0000, 0xff00_0000]);
const V16U16: PixelFormat = masks(BUMPDUDV, 32, [0xffff, 0xffff_0000, 0, 0]);

/// DirectXTex's `g_LegacyDDSMap`, in its order (the first match wins).
const LEGACY: &[(PixelFormat, Reads)] = {
    use Reads::*;
    use TexFormat::*;
    &[
        (four(b"DXT1"), As(Bc1)),
        (four(b"DXT3"), As(Bc2)),
        (four(b"DXT5"), As(Bc3)),
        (four(b"DXT2"), Premultiplied(Bc2)),
        (four(b"DXT4"), Premultiplied(Bc3)),
        // DXT5 with swizzled channels (normal-map tools): the blocks are BC3, returned as stored.
        (four(b"A2D5"), As(Bc3)),
        (four(b"xGBR"), As(Bc3)),
        (four(b"RxBG"), As(Bc3)),
        (four(b"RBxG"), As(Bc3)),
        (four(b"xRBG"), As(Bc3)),
        (four(b"RGxB"), As(Bc3)),
        (four(b"xGxR"), As(Bc3)),
        (four(b"GXRB"), As(Bc3)),
        (four(b"GRXB"), As(Bc3)),
        (four(b"RXGB"), As(Bc3)),
        (four(b"BRGX"), As(Bc3)),
        (four(b"BC4U"), As(Bc4Unorm)),
        (four(b"BC4S"), As(Bc4Snorm)),
        (four(b"BC5U"), As(Bc5Unorm)),
        (four(b"BC5S"), As(Bc5Snorm)),
        (four(b"ATI1"), As(Bc4Unorm)),
        (four(b"ATI2"), As(Bc5Unorm)),
        (four(b"A2XY"), As(Bc5Unorm)),
        (four(b"BC6H"), As(Bc6hUfloat)),
        (four(b"BC7L"), As(Bc7)),
        (four(b"BC7\0"), As(Bc7)),
        (four(b"RGBG"), Refused("R8G8_B8G8 (no TexFormat)")),
        (four(b"GRGB"), Refused("G8R8_G8B8 (no TexFormat)")),
        (A8R8G8B8, As(Bgra8Unorm)),
        (X8R8G8B8, As(Bgrx8Unorm)),
        (A8B8G8R8, As(Rgba8Unorm)),
        (
            masks(RGB, 32, [0xff, 0xff00, 0xff_0000, 0]),
            Refused("X8B8G8R8 (DirectXTex sets the unused byte to 255 as it loads)"),
        ),
        (G16R16, As(Rg16Unorm)),
        (
            masks(RGBA, 32, [0x3ff, 0xf_fc00, 0x3ff0_0000, 0xc000_0000]),
            Refused("A2R10G10B10 (DirectXTex swaps red and blue as it loads, the D3DX 10:10:10:2 reversal)"),
        ),
        (A2B10G10R10, As(Rgb10a2Unorm)),
        (
            masks(RGB, 24, [0xff_0000, 0xff00, 0xff, 0]),
            Refused("R8G8B8 (24 bits; DirectXTex expands it to R8G8B8A8)"),
        ),
        (R5G6B5, As(B5g6r5Unorm)),
        (A1R5G5B5, As(B5g5r5a1Unorm)),
        (
            masks(RGB, 16, [0x7c00, 0x3e0, 0x1f, 0]),
            Refused("X1R5G5B5 (DirectXTex sets the unused bit as it loads)"),
        ),
        (
            masks(RGBA, 16, [0xe0, 0x1c, 0x3, 0xff00]),
            Refused("A8R3G3B2 (DirectXTex expands it to R8G8B8A8)"),
        ),
        (
            masks(RGB, 8, [0xe0, 0x1c, 0x3, 0]),
            Refused("R3G3B2 (DirectXTex expands it to B5G6R5)"),
        ),
        (L8, As(R8Unorm)),
        (L16, As(R16Unorm)),
        (A8L8, As(Rg8Unorm)),
        // A8L8 with an 8-bit count (DirectXTex's DDSPF_A8L8_ALT): still two bytes per pixel.
        (masks(LUMINANCEA, 8, [0xff, 0, 0, 0xff00]), As(Rg8Unorm)),
        // NVTT 1 wrote luminance with the RGB flag.
        (masks(RGB, 8, [0xff, 0, 0, 0]), As(R8Unorm)),
        (masks(RGB, 16, [0xffff, 0, 0, 0]), As(R16Unorm)),
        (masks(RGBA, 16, [0xff, 0, 0, 0xff00]), As(Rg8Unorm)),
        (A8, As(A8Unorm)),
        (d3dfmt(36), As(Rgba16Unorm)),
        (
            d3dfmt(110),
            Refused("Q16W16V16U16 (no TexFormat for R16G16B16A16_SNORM)"),
        ),
        (d3dfmt(111), As(R16Float)),
        (d3dfmt(112), As(Rg16Float)),
        (d3dfmt(113), As(Rgba16Float)),
        (d3dfmt(114), As(R32Float)),
        (d3dfmt(115), As(Rg32Float)),
        (d3dfmt(116), As(Rgba32Float)),
        (masks(RGB, 32, [0xffff_ffff, 0, 0, 0]), As(R32Float)),
        (
            masks(PAL8A, 16, [0, 0, 0, 0xff00]),
            Refused("A8P8 (palettized; DirectXTex expands it)"),
        ),
        (
            masks(PAL8, 8, [0; 4]),
            Refused("P8 (palettized; DirectXTex expands it)"),
        ),
        (A4R4G4B4, As(B4g4r4a4Unorm)),
        (
            masks(RGB, 16, [0xf00, 0xf0, 0xf, 0]),
            Refused("X4R4G4B4 (DirectXTex sets the unused bits as it loads)"),
        ),
        (
            masks(LUMINANCEA, 8, [0xf, 0, 0, 0xf0]),
            Refused("A4L4 (DirectXTex expands it to B4G4R4A4)"),
        ),
        (four(b"YUY2"), Refused("YUY2 (no TexFormat)")),
        (four(b"UYVY"), Refused("UYVY (no TexFormat)")),
        (V8U8, As(Rg8Snorm)),
        (Q8W8V8U8, As(Rgba8Snorm)),
        (V16U16, As(Rg16Snorm)),
        (
            masks(BUMPLUMINANCE, 16, [0x1f, 0x3e0, 0xfc00, 0]),
            Refused("L6V5U5 (DirectXTex expands it to R8G8B8A8)"),
        ),
        (
            masks(BUMPLUMINANCE, 32, [0xff, 0xff00, 0xff_0000, 0]),
            Refused("X8L8V8U8 (DirectXTex converts it as it loads)"),
        ),
        (
            masks(BUMPDUDVA, 32, [0x3ff0_0000, 0xf_fc00, 0x3ff, 0xc000_0000]),
            Refused("A2W10V10U10 (DirectXTex converts it as it loads)"),
        ),
    ]
};

/// DirectXTex's match of a file's pixel format against one table entry (`GetDXGIFormat`): in FourCC mode only the
/// FourCC counts; otherwise the flags and bit count must be equal and the masks the entry's kind uses must agree.
fn matches(file: &PixelFormat, entry: &PixelFormat) -> bool {
    if file.flags & FOURCC != 0 {
        return entry.flags & FOURCC != 0 && file.fourcc == entry.fourcc;
    }
    if file.flags != entry.flags || file.bits != entry.bits {
        return false;
    }
    let [r, g, b, a] = file.masks;
    let [er, eg, eb, ea] = entry.masks;
    if entry.flags & PAL8 != 0 {
        true
    } else if entry.flags & ALPHA != 0 {
        a == ea
    } else if entry.flags & (LUMINANCE | BUMPDUDV) != 0 {
        r == er && (entry.flags & ALPHAPIXELS == 0 || a == ea)
    } else if entry.flags & ALPHAPIXELS != 0 {
        [r, g, b, a] == [er, eg, eb, ea]
    } else {
        [r, g, b] == [er, eg, eb]
    }
}

/// The format and alpha mode of a legacy pixel format, as DirectXTex reads it.
///
/// `nvtt` is the NVIDIA Texture Tools marker (`"NVTT"` in the header's reserved word 9): its writer sets two
/// nonstandard flags, `0x40000000` for sRGB and `0x80000000` for a normal map, which are ignored for the match and
/// the first gives the sRGB colour space where the format has an sRGB view.
///
/// One extension beyond DirectXTex: the FourCC flag with a zero FourCC (`0x44`, an Elden Ring layer texture's writer)
/// lets the masks decide, where DirectXTex finds no FourCC entry and refuses the file.
pub fn legacy_format(pf: &PixelFormat, nvtt: bool) -> Result<(TexFormat, ColorSpace, AlphaMode)> {
    let mut file = *pf;
    if nvtt {
        file.flags &= !0xc000_0000;
    }
    if file.flags & FOURCC != 0 && file.fourcc == [0; 4] {
        file.flags &= !FOURCC;
    }
    let Some((_, reads)) = LEGACY.iter().find(|(entry, _)| matches(&file, entry)) else {
        return Err(Error::Unsupported(if file.flags & FOURCC != 0 {
            format!("legacy FourCC {:?}", String::from_utf8_lossy(&pf.fourcc))
        } else {
            format!(
                "legacy pixel flags {:#x}, {} bits, masks {:x?}",
                pf.flags, pf.bits, pf.masks
            )
        }));
    };
    let (format, alpha) = match *reads {
        Reads::As(f) => (f, AlphaMode::Unknown),
        Reads::Premultiplied(f) => (f, AlphaMode::Premultiplied),
        Reads::Refused(why) => return Err(Error::Unsupported(format!("legacy {why}"))),
    };
    let srgb = nvtt && pf.flags & 0x4000_0000 != 0 && SRGB.iter().any(|(_, f)| *f == format);
    let color = if srgb {
        ColorSpace::Srgb
    } else {
        ColorSpace::Linear
    };
    Ok((format, color, alpha))
}

/// The legacy pixel format DirectXTex's `EncodeDDSHeader` writes for a format, if it has one.
///
/// `dx9` is `DDS_FLAGS_FORCE_DX9_LEGACY` (texconv `-dx9`): it also gives the sRGB formats their linear twin's pixel
/// format (the colour space is lost), 10:10:10:2 the D3DX mask order, and BC4/BC5 unorm the `ATI1`/`ATI2` codes.
/// Premultiplied alpha is `DXT2`/`DXT4` for BC2/BC3; no other pixel format states an alpha mode.
pub fn legacy_pixel_format(
    format: TexFormat,
    color: ColorSpace,
    alpha: AlphaMode,
    dx9: bool,
) -> Option<PixelFormat> {
    use TexFormat::*;
    let pm = alpha == AlphaMode::Premultiplied;
    if color == ColorSpace::Srgb {
        if !dx9 {
            return None;
        }
        return match format {
            Rgba8Unorm => Some(A8B8G8R8),
            Bc1 => Some(four(b"DXT1")),
            Bc2 => Some(four(if pm { b"DXT2" } else { b"DXT3" })),
            Bc3 => Some(four(if pm { b"DXT4" } else { b"DXT5" })),
            Bgra8Unorm => Some(A8R8G8B8),
            Bgrx8Unorm => Some(X8R8G8B8),
            _ => None,
        };
    }
    Some(match format {
        Rgba8Unorm => A8B8G8R8,
        Rg16Unorm => G16R16,
        Rg8Unorm => A8L8,
        R16Unorm => L16,
        R8Unorm => L8,
        A8Unorm => A8,
        Bc1 => four(b"DXT1"),
        Bc2 => four(if pm { b"DXT2" } else { b"DXT3" }),
        Bc3 => four(if pm { b"DXT4" } else { b"DXT5" }),
        Bc4Snorm => four(b"BC4S"),
        Bc5Snorm => four(b"BC5S"),
        Bc4Unorm => four(if dx9 { b"ATI1" } else { b"BC4U" }),
        Bc5Unorm => four(if dx9 { b"ATI2" } else { b"BC5U" }),
        B5g6r5Unorm => R5G6B5,
        B5g5r5a1Unorm => A1R5G5B5,
        Rg8Snorm => V8U8,
        Rgba8Snorm => Q8W8V8U8,
        Rg16Snorm => V16U16,
        Bgra8Unorm => A8R8G8B8,
        Bgrx8Unorm => X8R8G8B8,
        B4g4r4a4Unorm => A4R4G4B4,
        Rgba32Float => d3dfmt(116),
        Rgba16Float => d3dfmt(113),
        Rgba16Unorm => d3dfmt(36),
        Rg32Float => d3dfmt(115),
        Rg16Float => d3dfmt(112),
        R32Float => d3dfmt(114),
        R16Float => d3dfmt(111),
        Rgb10a2Unorm if dx9 => A2B10G10R10,
        _ => return None,
    })
}
