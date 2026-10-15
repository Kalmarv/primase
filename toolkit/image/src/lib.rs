//! PNG to straight RGBA pixels, piecewise sRGB transfer, and integer box mips.
//! No implicit gamma conversion: filtering averages the stored channel values.
mod normals;
mod texture;
pub use normals::{NormalFilter, normal_mip_chain, rebuild_normal_z, resize_normals};
use std::io::Cursor;
pub use texture::{
    LoadTextures, NormalizeNormals, TextureLoadReport, load_textures, normalize_normals, texture_level_png,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("DDS: {0}")]
    Dds(#[from] primase_dds::Error),
    #[error("texture decode: {0}")]
    Texture(#[from] primase_texcodec::Error),
    #[error("invalid image: {0}")]
    Invalid(String),
    #[error("PNG decoding: {0}")]
    Decode(#[from] png::DecodingError),
    #[error("PNG encoding: {0}")]
    Encode(#[from] png::EncodingError),
    #[error("image allocation failed")]
    Allocation,
}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Samples {
    U8(Vec<u8>),
    U16(Vec<u16>),
}
/// Row-major straight RGBA. 16-bit samples are native integers, not byte pairs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub samples: Samples,
}
impl Image {
    pub fn validate(&self) -> Result<()> {
        let len = sample_count(self.width, self.height)?;
        let actual = match &self.samples {
            Samples::U8(v) => v.len(),
            Samples::U16(v) => v.len(),
        };
        if len != actual {
            return Err(Error::Invalid(format!("expected {len} samples, got {actual}")));
        }
        Ok(())
    }
}
fn sample_count(w: u32, h: u32) -> Result<usize> {
    if w == 0 || h == 0 {
        return Err(Error::Invalid("zero dimension".into()));
    }
    (w as usize)
        .checked_mul(h as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| Error::Invalid("dimensions overflow".into()))
}
fn allocate<T: Default + Clone>(len: usize) -> Result<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(len).map_err(|_| Error::Allocation)?;
    v.resize(len, T::default());
    Ok(v)
}

/// PNG ancillary chunks that are consumed but not represented in the pixel image.
/// Callers can report lost metadata explicitly. Animation is rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Png {
    pub image: Image,
    pub ignored_ancillary: Vec<[u8; 4]>,
}
fn envelope(data: &[u8]) -> Result<Vec<[u8; 4]>> {
    if !data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(Error::Invalid("PNG signature".into()));
    }
    let mut offset = 8;
    let mut ignored = Vec::new();
    loop {
        let header = data
            .get(offset..offset + 8)
            .ok_or_else(|| Error::Invalid(format!("short PNG chunk at {offset}")))?;
        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let kind = [header[4], header[5], header[6], header[7]];
        let end = offset
            .checked_add(length)
            .and_then(|n| n.checked_add(12))
            .ok_or_else(|| Error::Invalid("chunk size overflow".into()))?;
        if end > data.len() {
            return Err(Error::Invalid(format!("short PNG payload at {offset}")));
        }
        if matches!(&kind, b"acTL" | b"fcTL" | b"fdAT") {
            return Err(Error::Invalid("animated PNG is unsupported".into()));
        }
        if kind[0] & 32 != 0 && &kind != b"tRNS" {
            ignored.push(kind);
        }
        if &kind == b"IEND" {
            if length != 0 || end != data.len() {
                return Err(Error::Invalid("nonempty IEND or trailing PNG bytes".into()));
            }
            return Ok(ignored);
        }
        offset = end;
    }
}
/// Expand grayscale/palette/transparency to RGBA while retaining 16-bit precision.
/// The png crate validates chunk CRCs and decompresses/interlace-unfilters pixels.
pub fn read_png(data: &[u8]) -> Result<Png> {
    let ignored_ancillary = envelope(data)?;
    let mut decoder = png::Decoder::new(Cursor::new(data));
    if data.get(25) != Some(&3) {
        decoder.set_transformations(png::Transformations::EXPAND);
    }
    let mut reader = decoder.read_info()?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| Error::Invalid("PNG size overflow".into()))?;
    let mut bytes = allocate(size)?;
    let info = reader.next_frame(&mut bytes)?;
    let palette = reader.info().palette.clone();
    let transparency = reader.info().trns.clone();
    reader.finish()?;
    bytes.truncate(info.buffer_size());
    let n = sample_count(info.width, info.height)?;
    let channels = info.color_type.samples();
    if info.color_type == png::ColorType::Indexed {
        let palette = palette.ok_or_else(|| Error::Invalid("missing palette".into()))?;
        let trns = transparency.as_deref().unwrap_or(&[]);
        let bits = info.bit_depth as usize;
        let mut rgba = allocate(n)?;
        for y in 0..info.height as usize {
            let row = &bytes[y * info.line_size..(y + 1) * info.line_size];
            for x in 0..info.width as usize {
                let index =
                    ((row[x * bits / 8] >> (8 - bits - x * bits % 8)) & ((1u16 << bits) - 1) as u8) as usize;
                let color = palette.get(index * 3..index * 3 + 3).ok_or_else(|| {
                    Error::Invalid(format!("palette index {index} at ({x},{y}) out of range"))
                })?;
                let at = (y * info.width as usize + x) * 4;
                rgba[at..at + 3].copy_from_slice(color);
                rgba[at + 3] = trns.get(index).copied().unwrap_or(255);
            }
        }
        return Ok(Png {
            image: Image {
                width: info.width,
                height: info.height,
                samples: Samples::U8(rgba),
            },
            ignored_ancillary,
        });
    }
    let image = match info.bit_depth {
        png::BitDepth::Eight => {
            let mut rgba = allocate(n)?;
            for (p, b) in rgba
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(bytes.chunks_exact(channels))
            {
                *p = match info.color_type {
                    png::ColorType::Grayscale => [b[0], b[0], b[0], 255],
                    png::ColorType::GrayscaleAlpha => [b[0], b[0], b[0], b[1]],
                    png::ColorType::Rgb => [b[0], b[1], b[2], 255],
                    png::ColorType::Rgba => [b[0], b[1], b[2], b[3]],
                    _ => return Err(Error::Invalid("unexpanded palette".into())),
                };
            }
            Image {
                width: info.width,
                height: info.height,
                samples: Samples::U8(rgba),
            }
        }
        png::BitDepth::Sixteen => {
            let mut rgba = allocate(n)?;
            for (p, b) in rgba
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(bytes.chunks_exact(channels * 2))
            {
                let mut c = [0; 4];
                for (i, pair) in b.as_chunks::<2>().0.iter().enumerate() {
                    c[i] = u16::from_be_bytes(*pair);
                }
                *p = match info.color_type {
                    png::ColorType::Grayscale => [c[0], c[0], c[0], 65535],
                    png::ColorType::GrayscaleAlpha => [c[0], c[0], c[0], c[1]],
                    png::ColorType::Rgb => [c[0], c[1], c[2], 65535],
                    png::ColorType::Rgba => c,
                    _ => return Err(Error::Invalid("16-bit palette".into())),
                };
            }
            Image {
                width: info.width,
                height: info.height,
                samples: Samples::U16(rgba),
            }
        }
        _ => return Err(Error::Invalid("unexpanded low-bit PNG".into())),
    };
    Ok(Png {
        image,
        ignored_ancillary,
    })
}
/// Deterministic RGBA PNG (8 or 16 bit). Metadata is not synthesized or copied.
pub fn write_png(image: &Image) -> Result<Vec<u8>> {
    image.validate()?;
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, image.width, image.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(match image.samples {
            Samples::U8(_) => png::BitDepth::Eight,
            Samples::U16(_) => png::BitDepth::Sixteen,
        });
        let mut writer = encoder.write_header()?;
        match &image.samples {
            Samples::U8(v) => writer.write_image_data(v)?,
            Samples::U16(v) => {
                let mut bytes = Vec::new();
                bytes
                    .try_reserve_exact(
                        v.len()
                            .checked_mul(2)
                            .ok_or_else(|| Error::Invalid("PNG size overflow".into()))?,
                    )
                    .map_err(|_| Error::Allocation)?;
                for &value in v {
                    bytes.extend_from_slice(&value.to_be_bytes());
                }
                writer.write_image_data(&bytes)?;
            }
        }
        writer.finish()?;
    }
    Ok(out)
}
/// IEC sRGB encoded value -> linear light, f64, exact piecewise curve (no clamp).
pub fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
/// Linear light -> IEC sRGB encoded value, f64 (no clamp).
pub fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1. / 2.4) - 0.055
    }
}

/// Next mip. Each target pixel covers [x*w/w', (x+1)*w/w'), so the final
/// region includes the third leftover row/column for odd dimensions.
/// Integer means round half upwards, including straight alpha, in stored space.
pub fn half(image: &Image) -> Result<Image> {
    image.validate()?;
    let w = (image.width / 2).max(1);
    let h = (image.height / 2).max(1);
    let filter = |get: &dyn Fn(usize) -> u64| -> Result<Vec<u16>> {
        let mut out = allocate(sample_count(w, h)?)?;
        for y in 0..h {
            for x in 0..w {
                let xa = x as u64 * image.width as u64 / w as u64;
                let xb = (x as u64 + 1) * image.width as u64 / w as u64;
                let ya = y as u64 * image.height as u64 / h as u64;
                let yb = (y as u64 + 1) * image.height as u64 / h as u64;
                let mut sum = [0u64; 4];
                let n = (xb - xa) * (yb - ya);
                for sy in ya..yb {
                    for sx in xa..xb {
                        let pos = (sy as usize * image.width as usize + sx as usize) * 4;
                        for (c, v) in sum.iter_mut().enumerate() {
                            *v += get(pos + c);
                        }
                    }
                }
                let pos = (y as usize * w as usize + x as usize) * 4;
                for c in 0..4 {
                    out[pos + c] = ((sum[c] + n / 2) / n) as u16;
                }
            }
        }
        Ok(out)
    };
    let samples = match &image.samples {
        Samples::U8(v) => Samples::U8(filter(&|i| v[i] as u64)?.into_iter().map(|v| v as u8).collect()),
        Samples::U16(v) => Samples::U16(filter(&|i| v[i] as u64)?),
    };
    Ok(Image {
        width: w,
        height: h,
        samples,
    })
}
/// Complete chain including the original level, ending at 1x1.
pub fn mip_chain(image: &Image) -> Result<Vec<Image>> {
    image.validate()?;
    let mut current = image.clone();
    let mut chain = Vec::new();
    loop {
        let done = current.width == 1 && current.height == 1;
        chain.push(current.clone());
        if done {
            return Ok(chain);
        }
        current = half(&current)?;
    }
}
/// First stored level whose larger side is <= cap; otherwise the smallest stored.
/// Returns (index,width,height). Empty/excessive level counts and cap=0 are errors.
pub fn pick_level(width: u32, height: u32, levels: usize, cap: u32) -> Result<(usize, u32, u32)> {
    sample_count(width, height)?;
    let max_levels = (u32::BITS - width.max(height).leading_zeros()) as usize;
    if levels == 0 || levels > max_levels || cap == 0 {
        return Err(Error::Invalid("invalid levels or cap".into()));
    }
    let (mut w, mut h, mut i) = (width, height, 0);
    while i + 1 < levels && w.max(h) > cap {
        w = (w / 2).max(1);
        h = (h / 2).max(1);
        i += 1;
    }
    Ok((i, w, h))
}

/// Deterministic 8-bit RGB PNG (colour type 2), without an alpha channel or
/// implicit colour transfer. Input is row-major RGB and must exactly fit dimensions.
pub fn write_png_rgb(width: u32, height: u32, rgb: &[u8]) -> Result<Vec<u8>> {
    let expected = sample_count(width, height)? / 4 * 3;
    if rgb.len() != expected {
        return Err(Error::Invalid(format!(
            "expected {expected} RGB samples, got {}",
            rgb.len()
        )));
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(rgb)?;
        writer.finish()?;
    }
    Ok(out)
}
