//! Unity textures (`Texture2D`): the pixel format, the mip chain from the object or its streamed resource, colour
//! space, how the texture is used, and its sampler state.
//!
//! Unity stores rows bottom first (row 0 is v = 0, its texture origin being bottom-left); the bytes are returned as
//! stored.

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
    /// `m_MipBias`: added to the level of detail the texture is sampled at.
    pub mip_bias: f32,
    /// `m_Aniso`: the anisotropic filtering level (above 1: anisotropic).
    pub aniso: i64,
    /// All levels, largest first, bottom row first.
    pub data: Vec<u8>,
}

/// A non-negative 32-bit integer of the texture (a negative one is no size).
fn dimension(v: &Value, k: &str) -> Result<u32> {
    let n = int(v, k)?;
    u32::try_from(n).map_err(|_| invalid(format!("texture: {k} of {n}")))
}

fn int(v: &Value, k: &str) -> Result<i64> {
    v.get(k)
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid(format!("texture: no integer {k}")))
}

fn float(v: &Value, k: &str) -> Result<f32> {
    v.get(k)
        .and_then(Value::as_f32)
        .ok_or_else(|| invalid(format!("texture: no float {k}")))
}

/// The pixel formats of Unity's `TextureFormat` this reader names. Crunched formats (28, 29, 64, 65) need Unity's
/// crunch decoder and are refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureFormat {
    /// `Alpha8` (1): one 8-bit channel.
    Alpha8,
    /// `ARGB4444` (2): one little-endian 16-bit value, A in the top 4 bits, then R, G, B.
    Argb4444,
    /// `RGB24` (3): three bytes a pixel, R, G, B.
    Rgb24,
    /// `RGBA32` (4): four bytes a pixel, R, G, B, A.
    Rgba32,
    /// `DXT1` (10): BC1, 4x4 blocks of 8 bytes.
    Dxt1,
    /// `DXT5` (12): BC3, 4x4 blocks of 16 bytes.
    Dxt5,
    /// `BC6H` (24): 4x4 blocks of 16 bytes, unsigned half floats.
    Bc6h,
    /// `BC7` (25): 4x4 blocks of 16 bytes.
    Bc7,
    /// `BC4` (26): 4x4 blocks of 8 bytes.
    Bc4,
    /// `BC5` (27): 4x4 blocks of 16 bytes.
    Bc5,
    /// `R8` (63): one 8-bit channel.
    R8,
}

impl TextureFormat {
    /// The format Unity's `TextureFormat` id `id` names.
    pub fn from_id(id: i64) -> Result<TextureFormat> {
        Ok(match id {
            1 => TextureFormat::Alpha8,
            2 => TextureFormat::Argb4444,
            3 => TextureFormat::Rgb24,
            4 => TextureFormat::Rgba32,
            10 => TextureFormat::Dxt1,
            12 => TextureFormat::Dxt5,
            24 => TextureFormat::Bc6h,
            25 => TextureFormat::Bc7,
            26 => TextureFormat::Bc4,
            27 => TextureFormat::Bc5,
            63 => TextureFormat::R8,
            28 | 29 | 64 | 65 => {
                return Err(Error::Unsupported(format!("crunched texture format {id}")));
            }
            id => return Err(Error::Unsupported(format!("texture format {id}"))),
        })
    }

    /// The size in pixels of a block and its size in bytes (one pixel for the uncompressed formats).
    fn block(self) -> (u32, usize) {
        match self {
            TextureFormat::Alpha8 | TextureFormat::R8 => (1, 1),
            TextureFormat::Argb4444 => (1, 2),
            TextureFormat::Rgb24 => (1, 3),
            TextureFormat::Rgba32 => (1, 4),
            TextureFormat::Dxt1 | TextureFormat::Bc4 => (4, 8),
            TextureFormat::Dxt5 | TextureFormat::Bc6h | TextureFormat::Bc7 | TextureFormat::Bc5 => (4, 16),
        }
    }

    /// The bytes of a `width` x `height` level, whole blocks rounded up; `None` when the size does not fit in a
    /// `usize` (the dimensions are the file's to state).
    pub fn level_size(self, width: u32, height: u32) -> Option<usize> {
        let (side, bytes) = self.block();
        let blocks_x = usize::try_from(width.div_ceil(side).max(1)).ok()?;
        let blocks_y = usize::try_from(height.div_ceil(side).max(1)).ok()?;
        blocks_x.checked_mul(blocks_y)?.checked_mul(bytes)
    }
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
            width: dimension(&t, "m_Width")?,
            height: dimension(&t, "m_Height")?,
            format: int(&t, "m_TextureFormat")?,
            mips: dimension(&t, "m_MipCount")?,
            srgb: int(&t, "m_ColorSpace")? == 1,
            usage: int(&t, "m_LightmapFormat")?,
            wrap: [int(settings, "m_WrapU")?, int(settings, "m_WrapV")?],
            filter: int(settings, "m_FilterMode")?,
            mip_bias: float(settings, "m_MipBias")?,
            aniso: int(settings, "m_Aniso")?,
            data,
        })
    }

    /// The mip chain split into its levels, largest first, each as stored (bottom row first), with the format the
    /// texture id names. Every size is checked: no more levels than halvings of a 32-bit side, no level whose size
    /// overflows or runs past the data, no data left over.
    pub fn levels(&self) -> Result<(TextureFormat, Vec<&[u8]>)> {
        if self.width == 0 || self.height == 0 || self.mips == 0 {
            return Err(Error::Unsupported(format!(
                "{}: an empty texture ({}x{}, {} levels)",
                self.name, self.width, self.height, self.mips
            )));
        }
        let format = TextureFormat::from_id(self.format)?;
        // no more levels than halvings of a 32-bit side
        if self.mips > 32 {
            return Err(invalid(format!("{}: {} mip levels", self.name, self.mips)));
        }
        let mut levels = Vec::with_capacity(self.mips as usize);
        let mut at = 0usize;
        for i in 0..self.mips {
            let (w, h) = ((self.width >> i).max(1), (self.height >> i).max(1));
            let size = format.level_size(w, h).ok_or_else(|| {
                invalid(format!(
                    "{}: level {i} of {w}x{h} does not fit in memory",
                    self.name
                ))
            })?;
            let bytes = at
                .checked_add(size)
                .and_then(|end| self.data.get(at..end))
                .ok_or_else(|| {
                    invalid(format!(
                        "{}: level {i} past the {} bytes of pixels",
                        self.name,
                        self.data.len()
                    ))
                })?;
            levels.push(bytes);
            at = at.saturating_add(size);
        }
        if at != self.data.len() {
            return Err(invalid(format!(
                "{}: {} levels use {at} of {} bytes",
                self.name,
                self.mips,
                self.data.len()
            )));
        }
        Ok((format, levels))
    }
}
