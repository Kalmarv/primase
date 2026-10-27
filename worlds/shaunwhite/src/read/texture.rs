//! PC TextureMap bodies and their inline CompiledTextureMap, retaining BCn blocks and every authored mip and face.
//!
//! Field names and codes 2/3 (DXT1/DXT1A), 4 (DXT3), 5 (DXT5), dimension and gamma are cross-checked against
//! AnvilToolkit 1.3.7's AC2/Brotherhood readers and AC2 schema. The game's bytes establish two differences: a u32
//! user category before the compiled pointer, and no MCT byte after the compiled sampler fields. The three bytes
//! before user category are retained as flags: the first takes values up to 5, so the reference's `bool Dynamic`
//! does not faithfully describe it here.
//!
//! Code 12 is this game's BC3 normal storage, established by the complete mip sizes and the decoded content
//! (neutral blocks have alpha/green near 128 and red/blue 255/0). The AC2 schema calls 12 AutoSelect; the toolkit's
//! later generic table calls it RGBA32F and does not apply here. The channels are the shipped shader's
//! (`SampleNormalMap` in `ekcommonps.hlsl`): X is the alpha channel and Y the green one, each `(v - 0.5) * 2`, and Z is
//! rebuilt as `sqrt(1 - x^2 - y^2)`. The shader then takes `x * T + y * B + z * N` with the mesh's tangent `T`, its
//! normal `N` and `B = cross(N, T) * w` (`DecompressTangentBasis` in `ekcommonvs.hlsl`), which is core's tangent
//! frame with green along the bitangent (`NormalGreen::Up`). The reader keeps the stored pixels, sets the Normal role
//! and that convention, [`NORMAL_CONVENTION`].
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use primase_core::{
    ColorSpace, NormalBasis, NormalConvention, NormalGreen, NormalX, NormalZ, TexFormat, TexRole, Texture,
};
use primase_foundations::bytes::{Cursor, Endian, Field, FieldError};

const LE: Endian = Endian::Little;

/// How the shipped shader reads a normal map (the module documentation): X in alpha, Y in green up the bitangent,
/// Z rebuilt, the tangent frame's T and `cross(N, T) * w`.
pub const NORMAL_CONVENTION: NormalConvention = NormalConvention {
    green: NormalGreen::Up,
    z: NormalZ::Rebuild,
    x: NormalX::Alpha,
    basis: NormalBasis::TangentBitangent,
};
pub const TEXTURE_MAP: u32 = 0xA2B7_E917;
pub const TEXTURE_MAP_SPEC: u32 = 0x989D_C6B2;
pub const COMPILED_TEXTURE_MAP: u32 = 0x1323_7FE9;

/// Why a stored texture cannot be read faithfully.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Field(#[from] FieldError),
    #[error("{field}: {why}")]
    Invalid { field: &'static str, why: String },
}

fn invalid(field: &'static str, why: impl Into<String>) -> Error {
    Error::Invalid {
        field,
        why: why.into(),
    }
}

fn word(c: &mut Cursor<'_>, name: &'static str) -> Result<u32, Error> {
    Ok(c.u32(LE).field(name)?)
}

fn expect(field: &'static str, found: u32, expected: u32) -> Result<(), Error> {
    if found == expected {
        Ok(())
    } else {
        Err(invalid(field, format!("{found:#x}, expected {expected:#x}")))
    }
}

fn end(c: &Cursor<'_>, field: &'static str) -> Result<(), Error> {
    if c.remaining() == 0 {
        Ok(())
    } else {
        Err(invalid(field, format!("{} trailing bytes", c.remaining())))
    }
}

/// The duplicated texture metadata. Codes are retained as well as translated when making core textures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub format_code: u32,
    /// 1: 2D; 2: cube. Other dimensions are refused, not flattened.
    pub dimension: u32,
    /// 0: linear; 1: sRGB. No transfer is applied to the stored bytes.
    pub gamma: u32,
    /// The stored count; 0 denotes one level (as the reference reader's clamp and the real pixel lengths agree).
    pub mips: u32,
    /// U, V, W: wrap=0, mirror=1, clamp=2, border=3, mirror-once=4.
    pub address: [u32; 3],
    /// Magnification, minification, mip: point=0, linear=1, none=2, anisotropic=3, comparison-linear=4.
    pub filter: [u32; 3],
}

impl Info {
    fn read(c: &mut Cursor<'_>, compiled: bool) -> Result<Self, Error> {
        let width = word(c, "width")?;
        let height = word(c, "height")?;
        let depth = word(c, "depth")?;
        let compiled_mips = if compiled {
            Some(word(c, "compiled mip count")?)
        } else {
            None
        };
        let format_code = word(c, "pixel format")?;
        let dimension = word(c, "texture dimension")?;
        let gamma = word(c, "gamma settings")?;
        let mips = match compiled_mips {
            Some(n) => n,
            None => word(c, "mip count")?,
        };
        let address = [
            word(c, "address U")?,
            word(c, "address V")?,
            word(c, "address W")?,
        ];
        let filter = [
            word(c, "mag filter")?,
            word(c, "min filter")?,
            word(c, "mip filter")?,
        ];
        Ok(Self {
            width,
            height,
            depth,
            format_code,
            dimension,
            gamma,
            mips,
            address,
            filter,
        })
    }

    /// The PC codec codes established on the real corpus; unknown codes are refused by name.
    pub fn format(&self) -> Result<TexFormat, Error> {
        match self.format_code {
            2 | 3 => Ok(TexFormat::Bc1),
            4 => Ok(TexFormat::Bc2),
            5 | 12 => Ok(TexFormat::Bc3),
            code => Err(invalid("pixel format", format!("unsupported code {code}"))),
        }
    }

    fn validate(&self) -> Result<usize, Error> {
        self.format()?;
        if self.width == 0 || self.height == 0 || self.depth != 1 {
            return Err(invalid(
                "dimensions",
                format!("{} x {} x {}", self.width, self.height, self.depth),
            ));
        }
        let faces = match self.dimension {
            1 => 1,
            2 if self.width == self.height => 6,
            dimension => {
                return Err(invalid(
                    "texture dimension",
                    format!("unsupported {dimension} or nonsquare cube"),
                ));
            }
        };
        if self.mips > self.width.max(self.height).ilog2().saturating_add(1) {
            return Err(invalid("mip count", self.mips.to_string()));
        }
        if self.gamma > 1 {
            return Err(invalid("gamma settings", self.gamma.to_string()));
        }
        if self.address.iter().any(|&v| v > 4) {
            return Err(invalid("address modes", format!("{:?}", self.address)));
        }
        if self.filter.iter().any(|&v| v > 4) {
            return Err(invalid("filter modes", format!("{:?}", self.filter)));
        }
        Ok(faces)
    }

    fn face_size(&self) -> Result<usize, Error> {
        let format = self.format()?;
        (0..self.mips.max(1)).try_fold(0usize, |sum, level| {
            let (w, h) = (
                self.width.checked_shr(level).unwrap_or(0).max(1),
                self.height.checked_shr(level).unwrap_or(0).max(1),
            );
            let n = format
                .checked_level_size(w, h)
                .ok_or_else(|| invalid("mip size", "exceeds address space"))?;
            sum.checked_add(n)
                .ok_or_else(|| invalid("mip sizes", "exceed address space"))
        })
    }
}

/// An inline compiled object: one id, one pixel buffer and duplicated metadata. Cube buffers are face-major,
/// each face's mips largest first (AnvilToolkit's AC2 DDS construction; real cube mip sizes and decodes checked).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledTextureMap<'a> {
    pub id: u32,
    pub platform_version: u32,
    pub sdk_version: u32,
    pub info: Info,
    pub pixels: &'a [u8],
}

impl<'a> CompiledTextureMap<'a> {
    /// A complete compiled object body, beginning at its id/class pair and ending after its sampler fields.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        let mut c = Cursor::new(bytes);
        let result = Self::read(&mut c)?;
        end(&c, "CompiledTextureMap")?;
        Ok(result)
    }

    fn read(c: &mut Cursor<'a>) -> Result<Self, Error> {
        let id = word(c, "compiled id")?;
        expect("compiled class", word(c, "compiled class")?, COMPILED_TEXTURE_MAP)?;
        let n = c.count(LE, 1).field("compiled pixel length")?;
        let pixels = c.take(n).field("compiled pixels")?;
        let platform_version = word(c, "platform version")?;
        let sdk_version = word(c, "SDK version")?;
        expect("platform version", platform_version, 2)?;
        expect("SDK version", sdk_version, 32)?;
        let info = Info::read(c, true)?;
        let faces = info.validate()?;
        let expected = info
            .face_size()?
            .checked_mul(faces)
            .ok_or_else(|| invalid("pixel length", "exceeds address space"))?;
        if pixels.len() != expected {
            return Err(invalid(
                "compiled pixel length",
                format!("{} bytes, mip chains require {expected}", pixels.len()),
            ));
        }
        Ok(Self {
            id,
            platform_version,
            sdk_version,
            info,
            pixels,
        })
    }
}

/// A managed TextureMap with its one inline compiled object. Ownership is explicit in the pointer; no id arithmetic
/// or filename pairing is used. Stored sampler fields, category and flags remain available to material readers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureMap<'a> {
    pub id: u32,
    pub info: Info,
    pub map_type: u32,
    pub category: u32,
    /// The three bytes named Dynamic, MCT and IgnoreSkipMips by the reference; retained without treating them as bools.
    pub flags: [u8; 3],
    pub user_category: u32,
    pub compiled: CompiledTextureMap<'a>,
}

impl<'a> TextureMap<'a> {
    /// An entire TextureMap body (managed byte, id/class pair, fields and inline compiled object), without its
    /// directory envelope. Unknown managed/pointer/class tags, codec, gamma, dimension, sampler enumerants and map
    /// types, disagreeing metadata, missing pixels and trailing bytes are errors naming the field.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        let mut c = Cursor::new(bytes);
        expect("managed flag", u32::from(c.u8().field("managed flag")?), 1)?;
        let id = word(&mut c, "texture id")?;
        expect("texture class", word(&mut c, "texture class")?, TEXTURE_MAP)?;
        let info = Info::read(&mut c, false)?;
        info.validate()?;
        let map_type = word(&mut c, "map type")?;
        if map_type > 10 {
            return Err(invalid("map type", map_type.to_string()));
        }
        let category = word(&mut c, "category")?;
        let flags = c.array().field("texture flags")?;
        let user_category = word(&mut c, "user category")?;
        expect(
            "compiled pointer tag",
            u32::from(c.u8().field("compiled pointer tag")?),
            0,
        )?;
        let compiled = CompiledTextureMap::read(&mut c)?;
        if compiled.info != info {
            return Err(invalid(
                "compiled metadata",
                format!("{:?} differs from TextureMap {info:?}", compiled.info),
            ));
        }
        end(&c, "TextureMap")?;
        Ok(Self {
            id,
            info,
            map_type,
            category,
            flags,
            user_category,
            compiled,
        })
    }

    /// Core textures in stored face order: one for 2D, six for cubes, each with its own complete mip chain. No BCn
    /// decoding, colour transfer, normal reconstruction or face rotation is performed. Cube direction/orientation
    /// is not established; the ordinal preserves the source. Generic and game-specific map types retain Other role.
    pub fn textures(&self) -> Result<Vec<Texture>, Error> {
        let faces = self.info.validate()?;
        let format = self.info.format()?;
        let role = match self.map_type {
            0 | 5 => TexRole::Color,
            1 => TexRole::Normal,
            2..=4 | 6 => TexRole::Mask,
            _ => TexRole::Other,
        };
        let color_space = if self.info.gamma == 1 {
            ColorSpace::Srgb
        } else {
            ColorSpace::Linear
        };
        let mut c = Cursor::new(self.compiled.pixels);
        let mut textures = Vec::with_capacity(faces);
        for _ in 0..faces {
            let mut levels = Vec::new();
            for level in 0..self.info.mips.max(1) {
                let (w, h) = (
                    self.info.width.checked_shr(level).unwrap_or(0).max(1),
                    self.info.height.checked_shr(level).unwrap_or(0).max(1),
                );
                let n = format
                    .checked_level_size(w, h)
                    .ok_or_else(|| invalid("mip size", "exceeds address space"))?;
                levels.push(c.take(n).field("mip pixels")?.to_vec());
            }
            textures.push(Texture {
                width: self.info.width,
                height: self.info.height,
                format,
                levels,
                color_space,
                role,
                normal: (role == TexRole::Normal).then_some(NORMAL_CONVENTION),
            });
        }
        end(&c, "face mip chains")?;
        Ok(textures)
    }
}
