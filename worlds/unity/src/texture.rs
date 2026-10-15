//! Unity textures (`Texture2D`): Unity's TextureFormat ids onto [`TexFormat`], the mip chain from the object or its
//! streamed resource, colour space, how the texture is used, and its sampler state.
//!
//! Unity stores rows bottom first (row 0 is v = 0, its texture origin being bottom-left); the world's textures are top
//! first with their origin top-left (and the mesh reader stores v as 1 - v), so every texture is flipped
//! ([`primase_texcodec::flip_vertical`]: exact in place for uncompressed and BC1-5 whole-block levels, decoded
//! otherwise). That gives back the image as authored, which Unity's importer flipped.

use primase_core::{
    ColorSpace, NormalConvention, NormalGreen, NormalX, NormalZ, TexFormat, TexRole, Texture,
};
use primase_texcodec::{Decoder, flip_vertical};

use crate::player::{ObjRef, Player};
use crate::resource::StreamingInfo;
use crate::value::Value;
use crate::{Error, Result, invalid};

/// Unity's TextureUsageMode for a normal map stored as DXT5nm: X in alpha, Y in green.
pub const USAGE_NORMAL_DXT5NM: i64 = 3;
/// Unity's TextureUsageMode for a normal map stored as RGB.
pub const USAGE_NORMAL_PLAIN: i64 = 4;

/// A `Texture2D` as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct UnityTexture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Unity's TextureFormat id.
    pub format: i64,
    pub mips: u32,
    /// `m_ColorSpace` 1: sampled as sRGB.
    pub srgb: bool,
    /// Unity's TextureUsageMode (`m_LightmapFormat`).
    pub usage: i64,
    /// TextureWrapMode along u and v (0 repeat, 1 clamp, 2 mirror, 3 mirror once).
    pub wrap: [i64; 2],
    /// FilterMode (0 point, 1 bilinear, 2 trilinear).
    pub filter: i64,
    /// All levels, largest first, bottom row first.
    pub data: Vec<u8>,
}

fn int(v: &Value, k: &str) -> Result<i64> {
    v.get(k)
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid(format!("texture: no integer {k}")))
}

/// How a TextureFormat is stored, as a world format: the same bytes, or RGB24 widened to RGBA8 (alpha 255).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stored {
    As(TexFormat),
    Rgb24,
}

/// The TextureFormat ids this reader decodes. Crunched formats (28, 29, 64, 65) need Unity's crunch decoder.
fn stored(id: i64) -> Result<Stored> {
    use TexFormat::*;
    Ok(Stored::As(match id {
        1 => A8Unorm,
        // ARGB4444: one little-endian 16-bit value, A in the top 4 bits, then R, G, B
        2 => B4g4r4a4Unorm,
        3 => return Ok(Stored::Rgb24),
        4 => Rgba8Unorm,
        10 => Bc1,
        12 => Bc3,
        24 => Bc6hUfloat,
        25 => Bc7,
        26 => Bc4Unorm,
        27 => Bc5Unorm,
        63 => R8Unorm,
        28 | 29 | 64 | 65 => {
            return Err(Error::Unsupported(format!("crunched texture format {id}")));
        }
        id => return Err(Error::Unsupported(format!("texture format {id}"))),
    }))
}

impl UnityTexture {
    /// Read the Texture2D `r` of `player` (its pixels in the object or streamed beside its file).
    pub fn read(player: &Player, r: &ObjRef) -> Result<Self> {
        let t = player.decode(r)?;
        let settings = t
            .get("m_TextureSettings")
            .ok_or_else(|| invalid("texture: no m_TextureSettings"))?;
        let stream = t
            .get("m_StreamData")
            .map(StreamingInfo::of)
            .transpose()?
            .flatten();
        let data = match stream {
            Some(s) => {
                let dir = player
                    .folders
                    .get(&r.file)
                    .ok_or_else(|| invalid(format!("no folder for {}", r.file)))?;
                s.read(dir)?
            }
            None => t
                .get("image data")
                .and_then(Value::as_bytes)
                .ok_or_else(|| invalid("texture: no image data"))?
                .to_vec(),
        };
        Ok(UnityTexture {
            name: t.get("m_Name").and_then(Value::as_str).unwrap_or("").to_owned(),
            width: int(&t, "m_Width")? as u32,
            height: int(&t, "m_Height")? as u32,
            format: int(&t, "m_TextureFormat")?,
            mips: int(&t, "m_MipCount")? as u32,
            srgb: int(&t, "m_ColorSpace")? == 1,
            usage: int(&t, "m_LightmapFormat")?,
            wrap: [int(settings, "m_WrapU")?, int(settings, "m_WrapV")?],
            filter: int(settings, "m_FilterMode")?,
            data,
        })
    }

    /// The normal map convention its usage mode says, if it is one: Unity's normal maps are green up with Z rebuilt
    /// (URP unpacks X from red times alpha, which is alpha for DXT5nm and red for plain maps).
    pub fn normal(&self) -> Option<NormalConvention> {
        let x = match self.usage {
            USAGE_NORMAL_DXT5NM => NormalX::Alpha,
            USAGE_NORMAL_PLAIN => NormalX::Red,
            _ => return None,
        };
        Some(NormalConvention {
            green: NormalGreen::Up,
            z: NormalZ::Rebuild,
            x,
            basis: primase_core::NormalBasis::TangentBitangent,
        })
    }

    /// As a world texture, rows top first: normal maps (by usage mode) linear with their convention, others colour
    /// (sRGB) or other (linear) until a material says more.
    pub fn texture(&self) -> Result<Texture> {
        if self.width == 0 || self.height == 0 || self.mips == 0 {
            return Err(Error::Unsupported(format!(
                "{}: an empty texture ({}x{}, {} levels)",
                self.name, self.width, self.height, self.mips
            )));
        }
        let stored = stored(self.format)?;
        let format = match stored {
            Stored::As(f) => f,
            Stored::Rgb24 => TexFormat::Rgba8Unorm,
        };
        let mut levels = Vec::with_capacity(self.mips as usize);
        let mut at = 0usize;
        for i in 0..self.mips {
            let (w, h) = ((self.width >> i).max(1), (self.height >> i).max(1));
            let size = match stored {
                Stored::As(f) => f.level_size(w, h),
                Stored::Rgb24 => 3 * w as usize * h as usize,
            };
            let bytes = self.data.get(at..at + size).ok_or_else(|| {
                invalid(format!(
                    "{}: level {i} past the {} bytes of pixels",
                    self.name,
                    self.data.len()
                ))
            })?;
            levels.push(match stored {
                Stored::As(_) => bytes.to_vec(),
                Stored::Rgb24 => bytes
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .flat_map(|&[r, g, b]| [r, g, b, 255])
                    .collect(),
            });
            at += size;
        }
        if at != self.data.len() {
            return Err(invalid(format!(
                "{}: {} levels use {at} of {} bytes",
                self.name,
                self.mips,
                self.data.len()
            )));
        }
        let normal = self.normal();
        let bottom_first = Texture {
            width: self.width,
            height: self.height,
            format,
            levels,
            color_space: if self.srgb && normal.is_none() {
                ColorSpace::Srgb
            } else {
                ColorSpace::Linear
            },
            role: match (normal, self.srgb) {
                (Some(_), _) => TexRole::Normal,
                (None, true) => TexRole::Color,
                (None, false) => TexRole::Other,
            },
            normal,
        };
        flip_vertical(&bottom_first, &Decoder::default()).map_err(|e| invalid(format!("{}: {e}", self.name)))
    }
}
