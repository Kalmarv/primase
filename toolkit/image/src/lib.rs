//! PNG, JPEG ([`read_jpeg`]) and QOI ([`read_qoi`]) to straight RGBA pixels, piecewise sRGB transfer, and integer box mips.
//! No implicit gamma conversion: filtering averages the stored channel values.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod atlas;
pub use atlas::{GUTTER, PACK_TEXTURE_ATLAS, PackTextureAtlas};
mod bleed;
pub use bleed::{BLEED_ALPHA_COLORS, BleedAlphaColors, nearest_visible};
mod cap;
pub use cap::{Beyond, CAP_TEXTURES, CapMode, CapRole, CapTextures, Capped, NormalDownscale, cap_texture};
mod channels;
mod cutout_field;
mod cutouts;
pub use cutouts::{CLIP_ALPHA_CUTOUTS, ClipAlphaCutouts};
mod frames;
mod jpeg;
pub use channels::{Rgba8Texel, TextureMetadata, map_rgba8_levels, map_rgba8_levels_with};
pub use frames::{Bitangent, UV_FRAME_NORMALS, UvFrameNormals};
pub use jpeg::{Jpeg, JpegColor, read_jpeg};
mod layers;
pub use layers::{BAKE_LAYER_CHANNELS, BakeLayerChannels, BakeLayerChannelsPixelAlpha, LayerAlpha};
mod modes;
pub use modes::{SET_MATERIAL_MODES, SetMaterialModes};
mod normals;
mod orm;
pub use orm::{MissingLevels, OrmSize, PACK_ORM, PackOrm};
mod qoi;
mod rgba8;
pub use qoi::{Qoi, QoiChannels, QoiColorspace, read_qoi};
pub use rgba8::{LOWER_TEXTURES_RGBA8, LowerTexturesRgba8, SignedData, rgba8_exact};
mod texture;
mod upscale;
pub use normals::{NormalFilter, normal_mip_chain, rebuild_normal_z, resize_normals, scale_normal};
use primase_foundations::color::{linear_to_srgb8, srgb8_to_linear};
use std::io::Cursor;
pub use texture::{
    BAKE_NORMAL_SCALE, BakeNormalScale, NORMALIZE_NORMALS, NormalizeNormals, TextureLoadReport,
    bake_normal_scale, decode_texture_file, load_textures, normalize_normals, texture_level_png,
    texture_level_rgba8, texture_level_rgba8_with,
};
use thiserror::Error;
pub use upscale::{MAX_UPSCALED, UPSCALE_NEAREST, UpscaleBy, UpscaleNearest, enlarge_nearest};
/// This crate's source files, for the keys of disk caches whose values its code computes (`primase-cache`): editing
/// any of them retires those entries. A test keeps the list equal to the `src` folder.
pub const CACHE_SOURCES: &[(&str, &[u8])] = &[
    ("image/atlas.rs", include_bytes!("atlas.rs")),
    ("image/bleed.rs", include_bytes!("bleed.rs")),
    ("image/cap.rs", include_bytes!("cap.rs")),
    ("image/channels.rs", include_bytes!("channels.rs")),
    ("image/cutout_field.rs", include_bytes!("cutout_field.rs")),
    ("image/cutouts.rs", include_bytes!("cutouts.rs")),
    ("image/frames.rs", include_bytes!("frames.rs")),
    ("image/jpeg.rs", include_bytes!("jpeg.rs")),
    ("image/layers.rs", include_bytes!("layers.rs")),
    ("image/lib.rs", include_bytes!("lib.rs")),
    ("image/modes.rs", include_bytes!("modes.rs")),
    ("image/normals.rs", include_bytes!("normals.rs")),
    ("image/orm.rs", include_bytes!("orm.rs")),
    ("image/qoi.rs", include_bytes!("qoi.rs")),
    ("image/rgba8.rs", include_bytes!("rgba8.rs")),
    ("image/texture.rs", include_bytes!("texture.rs")),
    ("image/upscale.rs", include_bytes!("upscale.rs")),
];
/// Register this crate's passes by their declarations.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    for spec in [
        &PACK_TEXTURE_ATLAS,
        &modes::SET_MATERIAL_MODES,
        &BLEED_ALPHA_COLORS,
        &CAP_TEXTURES,
        &CLIP_ALPHA_CUTOUTS,
        &BAKE_LAYER_CHANNELS,
        &rgba8::LOWER_TEXTURES_RGBA8,
        &NORMALIZE_NORMALS,
        &frames::UV_FRAME_NORMALS,
        &orm::PACK_ORM,
        &BAKE_NORMAL_SCALE,
        &upscale::UPSCALE_NEAREST,
    ] {
        reg.add(spec);
    }
}
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
    #[error("JPEG decoding: {0}")]
    Jpeg(String),
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
    // the first chunk is the header (the PNG specification's rule), whose colour type is its ninth byte: indexed
    // images keep their indices here, the others are expanded
    if data.get(12..16) != Some(b"IHDR".as_slice()) {
        return Err(Error::Invalid("PNG: the first chunk is not IHDR".into()));
    }
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
/// A colour a caller states, as an option's `R,G,B,A`: sRGB bytes and a linear alpha byte, each 0 to 255 (a flat colour
/// standing in for a shader a lowering does not draw, which is the program's choice, not the source's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Srgba8(pub [u8; 4]);
impl Srgba8 {
    /// `R,G,B,A`, four bytes.
    pub fn parse(text: &str) -> std::result::Result<Srgba8, String> {
        let bytes: Vec<u8> = text
            .split(',')
            .map(|v| v.trim().parse::<u8>())
            .collect::<std::result::Result<_, _>>()
            .map_err(|_| format!("{text}: four bytes R,G,B,A (0 to 255)"))?;
        let bytes: [u8; 4] = bytes
            .try_into()
            .map_err(|_| format!("{text}: four bytes R,G,B,A (0 to 255)"))?;
        Ok(Srgba8(bytes))
    }

    /// As a linear colour factor: the colour bytes decoded from sRGB ([`srgb8_to_linear`], to f32), alpha divided by
    /// 255.
    pub fn linear(self) -> [f32; 4] {
        let [r, g, b, a] = self.0;
        [
            srgb8_to_linear(r) as f32,
            srgb8_to_linear(g) as f32,
            srgb8_to_linear(b) as f32,
            f32::from(a) / 255.0,
        ]
    }
}
impl std::fmt::Display for Srgba8 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [r, g, b, a] = self.0;
        write!(f, "{r},{g},{b},{a}")
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
/// [`mip_chain`] over straight RGBA8 bytes (`width * height * 4`, rows top first): every level as
/// `(width, height, rgba)`, the input first, down to 1x1. Each side is halved (at least 1); a target pixel
/// averages the source pixels it covers per channel, in 8-bit, rounding half up (`(sum + n/2) / n`; for a full
/// 2x2 that is `(sum + 2) / 4`). An odd side's last target pixel also covers the leftover source row/column, as
/// the independent C# reference does (not clamped). Averaging is on the stored values, with no sRGB conversion:
/// for sRGB colour that is an approximation (it darkens high-contrast detail slightly).
pub fn mip_chain_rgba8(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<(u32, u32, Vec<u8>)>> {
    let base = Image {
        width,
        height,
        samples: Samples::U8(rgba.to_vec()),
    };
    mip_chain(&base)?
        .into_iter()
        .map(|level| match level.samples {
            Samples::U8(v) => Ok((level.width, level.height, v)),
            Samples::U16(_) => Err(Error::Invalid("mip level lost its 8-bit depth".into())),
        })
        .collect()
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
#[cfg(test)]
mod registration_tests {
    use super::*;
    use primase_core::pass::PassRegistry;

    /// The crate registers exactly these passes by their declarations: those with required options are made only with
    /// them, and every one says what it takes.
    #[test]
    fn the_image_passes_register_by_name() {
        let mut reg = PassRegistry::new();
        register(&mut reg);
        let names = [
            "bake-layer-channels",
            "bake-normal-scale",
            "bleed-alpha-colors",
            "cap-textures",
            "clip-alpha-cutouts",
            "lower-textures-rgba8",
            "normalize-normals",
            "pack-orm",
            "pack-texture-atlas",
            "set-material-modes",
            "upscale-nearest",
            "uv-frame-normals",
        ];
        assert_eq!(format!("{reg:?}"), format!("{names:?}"));
        let free = [
            "bake-layer-channels",
            "bake-normal-scale",
            "bleed-alpha-colors",
            "lower-textures-rgba8",
            "normalize-normals",
        ];
        for name in names {
            assert_eq!(reg.requires_options(name), !free.contains(&name), "{name}");
            assert!(reg.describe(name).is_some_and(|d| d.starts_with(name)), "{name}");
            let bare = reg.make(name, &primase_core::pass::Options::new());
            assert_eq!(bare.is_err(), !free.contains(&name), "{name}");
        }
        assert!(
            reg.describe("upscale-nearest")
                .unwrap()
                .starts_with("upscale-nearest{(factor|size)*,textures*}\n")
        );
    }
}

/// Every stored level of `t`, decoded to RGBA8 as stored (no transfer applied), for sampling.
pub fn levels(t: &primase_core::Texture) -> Result<primase_foundations::sampling::Levels> {
    let meta = TextureMetadata {
        color_space: primase_core::ColorSpace::Linear,
        role: primase_core::TexRole::Other,
        normal: None,
    };
    let d = map_rgba8_levels(t, 0..t.levels.len(), meta, |x| Ok(x.rgba))?;
    Ok(primase_foundations::sampling::Levels {
        width: d.width,
        height: d.height,
        levels: d.levels,
    })
}

impl From<primase_foundations::sampling::Error> for Error {
    fn from(e: primase_foundations::sampling::Error) -> Error {
        match e {
            primase_foundations::sampling::Error::Invalid(why) => Error::Invalid(why),
        }
    }
}
