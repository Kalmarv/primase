//! Strict 2D DDS containers. Payloads are retained verbatim, never decoded.
//!
//! Legacy DDS has no colour-space tag and reads as linear; callers may override it.
//! DX10 sRGB tags are preserved. Role and normal convention are not DDS metadata:
//! reads use `Other` and `None`. Only tightly packed rows are supported.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

use primase_core::{ColorSpace, TexFormat, TexRole, Texture};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("DDS at byte {offset}: {detail}")]
    Invalid { offset: usize, detail: String },
    #[error("DDS does not support {0}")]
    Unsupported(String),
}
pub type Result<T> = std::result::Result<T, Error>;
fn invalid(offset: usize, detail: impl Into<String>) -> Error {
    Error::Invalid {
        offset,
        detail: detail.into(),
    }
}
fn word(b: &[u8], o: usize) -> Result<u32> {
    let s = b.get(o..o + 4).ok_or_else(|| invalid(o, "truncated header"))?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}
fn put(b: &mut [u8], o: usize, n: u32) {
    b[o..o + 4].copy_from_slice(&n.to_le_bytes());
}

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
fn legacy(b: &[u8]) -> Result<TexFormat> {
    use TexFormat::*;
    let flags = word(b, 80)?;
    if flags == 4 {
        return match &b[84..88] {
            b"DXT1" => Ok(Bc1),
            b"DXT3" => Ok(Bc2),
            b"DXT5" => Ok(Bc3),
            b"ATI1" | b"BC4U" => Ok(Bc4Unorm),
            b"ATI2" | b"BC5U" => Ok(Bc5Unorm),
            b"BC4S" => Ok(Bc4Snorm),
            b"BC5S" => Ok(Bc5Snorm),
            c => Err(Error::Unsupported(format!("legacy FourCC {c:?}"))),
        };
    }
    let masks = [
        word(b, 88)?,
        word(b, 92)?,
        word(b, 96)?,
        word(b, 100)?,
        word(b, 104)?,
    ];
    match (flags, masks) {
        (0x40, [16, 0xf800, 0x7e0, 0x1f, 0]) => Ok(B5g6r5Unorm),
        (0x41, [16, 0x7c00, 0x3e0, 0x1f, 0x8000]) => Ok(B5g5r5a1Unorm),
        (0x41, [16, 0xf00, 0xf0, 0xf, 0xf000]) => Ok(B4g4r4a4Unorm),
        (0x41, [32, 0xff, 0xff00, 0xff0000, 0xff000000]) => Ok(Rgba8Unorm),
        (0x41, [32, 0xff0000, 0xff00, 0xff, 0xff000000]) => Ok(Bgra8Unorm),
        (0x40, [32, 0xff0000, 0xff00, 0xff, 0]) => Ok(Bgrx8Unorm),
        (0x41, [32, 0x3ff, 0xffc00, 0x3ff00000, 0xc0000000]) => Ok(Rgb10a2Unorm),
        (0x20000, [8, 0xff, 0, 0, 0]) => Ok(R8Unorm),
        (0x20000, [16, 0xffff, 0, 0, 0]) => Ok(R16Unorm),
        (0x20001, [16, 0xff, 0, 0, 0xff00]) => Ok(Rg8Unorm),
        (2, [8, 0, 0, 0, 0xff]) => Ok(A8Unorm),
        _ => Err(Error::Unsupported(format!(
            "legacy pixel flags {flags:#x}, masks {masks:x?}"
        ))),
    }
}
fn size(f: TexFormat, w: u32, h: u32) -> Result<usize> {
    let (bw, bh) = f.block_dims();
    let n = u64::from(w.div_ceil(bw)) * u64::from(h.div_ceil(bh));
    n.checked_mul(u64::from(f.block_bytes()))
        .and_then(|v| usize::try_from(v).ok())
        .ok_or_else(|| invalid(16, "level size overflow"))
}
fn dimensions(w: u32, h: u32, n: u32) -> Result<()> {
    if w == 0 || h == 0 {
        return Err(invalid(12, "zero dimensions"));
    }
    if n == 0 || n > 32 - w.max(h).leading_zeros() {
        return Err(invalid(28, "invalid mip count"));
    }
    Ok(())
}
/// Read one tightly packed 2D texture, consuming the exact input length.
/// Arrays, cubes, volumes, 1D resources and DX10 alpha-mode metadata are rejected.
pub fn read(bytes: &[u8]) -> Result<Texture> {
    if bytes.get(..4) != Some(b"DDS ") {
        return Err(invalid(0, "bad magic"));
    }
    if word(bytes, 4)? != 124 || word(bytes, 76)? != 32 {
        return Err(invalid(4, "invalid header size"));
    }
    if bytes.len() < 128 {
        return Err(invalid(bytes.len(), "truncated header"));
    }
    let flags = word(bytes, 8)?;
    if flags & 0x1007 != 0x1007 {
        return Err(invalid(8, "missing required header flags"));
    }
    if word(bytes, 112)? & 0xfe00 != 0 {
        return Err(Error::Unsupported("cube maps".into()));
    }
    if word(bytes, 112)? & 0x200000 != 0 || flags & 0x800000 != 0 || word(bytes, 24)? > 1 {
        return Err(Error::Unsupported("volumes".into()));
    }
    let w = word(bytes, 16)?;
    let h = word(bytes, 12)?;
    let n = word(bytes, 28)?.max(1);
    dimensions(w, h, n)?;
    let (format, color_space, start) = if word(bytes, 80)? == 4 && &bytes[84..88] == b"DX10" {
        let dim = word(bytes, 132)?;
        if dim != 3 {
            return Err(Error::Unsupported(format!(
                "resource dimension {dim} (only 2D supported)"
            )));
        }
        if word(bytes, 136)? & 4 != 0 {
            return Err(Error::Unsupported("cube maps".into()));
        }
        if word(bytes, 140)? != 1 {
            return Err(Error::Unsupported(format!(
                "arrays (arraySize={})",
                word(bytes, 140)?
            )));
        }
        if word(bytes, 136)? != 0 || word(bytes, 144)? != 0 {
            return Err(Error::Unsupported("DX10 misc flags / alpha-mode metadata".into()));
        }
        let (f, c) = from_dxgi(word(bytes, 128)?)?;
        (f, c, 148)
    } else {
        (legacy(bytes)?, ColorSpace::Linear, 128)
    };
    let top = size(format, w, h)?;
    if flags & 8 != 0 && u64::from(word(bytes, 20)?) != u64::from(w) * u64::from(format.block_bytes()) {
        return Err(invalid(20, "padded or inconsistent row pitch"));
    }
    if flags & 0x80000 != 0 && word(bytes, 20)? as usize != top {
        return Err(invalid(20, "inconsistent top-level linear size"));
    }
    let mut ranges = Vec::new();
    let mut end: usize = start;
    for i in 0..n {
        let len = size(format, (w >> i).max(1), (h >> i).max(1))?;
        let next = end
            .checked_add(len)
            .ok_or_else(|| invalid(end, "payload size overflow"))?;
        if next > bytes.len() {
            return Err(invalid(end, format!("truncated mip {i}: need {len} bytes")));
        }
        ranges.push(end..next);
        end = next;
    }
    if end != bytes.len() {
        return Err(invalid(end, format!("{} trailing bytes", bytes.len() - end)));
    }
    Ok(Texture {
        width: w,
        height: h,
        format,
        levels: ranges.into_iter().map(|r| bytes[r].to_vec()).collect(),
        color_space,
        role: TexRole::Other,
        normal: None,
    })
}
fn validate(t: &Texture) -> Result<()> {
    let n = u32::try_from(t.levels.len()).map_err(|_| invalid(28, "too many levels"))?;
    dimensions(t.width, t.height, n)?;
    for (i, l) in t.levels.iter().enumerate() {
        let expected = size(t.format, (t.width >> i).max(1), (t.height >> i).max(1))?;
        if l.len() != expected {
            return Err(invalid(
                i,
                format!("mip {i}: {} bytes, expected {expected}", l.len()),
            ));
        }
    }
    Ok(())
}
fn header(t: &Texture, dx10: bool) -> Result<Vec<u8>> {
    validate(t)?;
    let mut b = vec![0; if dx10 { 148 } else { 128 }];
    b[..4].copy_from_slice(b"DDS ");
    put(&mut b, 4, 124);
    let compressed = t.format.is_block_compressed();
    put(
        &mut b,
        8,
        0x1007 | if compressed { 0x80000 } else { 8 } | if t.levels.len() > 1 { 0x20000 } else { 0 },
    );
    put(&mut b, 12, t.height);
    put(&mut b, 16, t.width);
    let pitch = if compressed {
        size(t.format, t.width, t.height)?
    } else {
        size(t.format, t.width, 1)?
    };
    put(
        &mut b,
        20,
        u32::try_from(pitch).map_err(|_| invalid(20, "pitch exceeds u32"))?,
    );
    put(&mut b, 28, t.levels.len() as u32);
    put(&mut b, 76, 32);
    put(&mut b, 80, 4);
    put(&mut b, 108, if t.levels.len() > 1 { 0x401008 } else { 0x1000 });
    Ok(b)
}
/// Write BC1/2/3 with DDS_HEADER flags describing the supplied levels.
/// Legacy DDS cannot encode colour space; the caller explicitly chooses this loss
/// by using this function. Mips are written as supplied (no trimming/generation).
pub fn write_legacy(t: &Texture) -> Result<Vec<u8>> {
    let four = match t.format {
        TexFormat::Bc1 => b"DXT1",
        TexFormat::Bc2 => b"DXT3",
        TexFormat::Bc3 => b"DXT5",
        f => return Err(Error::Unsupported(format!("legacy writer format {f:?}"))),
    };
    let mut b = header(t, false)?;
    b[84..88].copy_from_slice(four);
    for l in &t.levels {
        b.extend_from_slice(l);
    }
    Ok(b)
}
/// Write a DX10 2D texture with a typed DXGI format and explicit colour space.
pub fn write_dx10(t: &Texture) -> Result<Vec<u8>> {
    let dxgi = to_dxgi(t.format, t.color_space)?;
    let mut b = header(t, true)?;
    b[84..88].copy_from_slice(b"DX10");
    put(&mut b, 128, dxgi);
    put(&mut b, 132, 3);
    put(&mut b, 140, 1);
    for l in &t.levels {
        b.extend_from_slice(l);
    }
    Ok(b)
}
