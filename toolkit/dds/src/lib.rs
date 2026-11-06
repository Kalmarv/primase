//! DDS files: every layout Direct3D stores in one (1D, 2D, cube and volume textures, arrays of them, with mips), under
//! the legacy header or the DX10 one. Payloads are retained verbatim, never decoded.
//!
//! A header means what it means to DirectXTex (Microsoft's reference, may2026), whose `DecodeDDSHeader` and
//! `EncodeDDSHeader` this follows field by field: [`parse`] reads every layout texconv reads, and [`write()`] writes the
//! header bytes texconv writes. The differences are deliberate and listed in the README: this reader is stricter (the
//! payload must be exactly the layout's size; typeless formats, unknown DX10 misc flags and the legacy formats
//! DirectXTex converts as it loads are refused rather than altered), it reads the face-counted DX10 cubes DirectXTex
//! reads only with `-permissive`, and it reads one legacy pixel format DirectXTex refuses (the FourCC flag with no
//! FourCC).
//!
//! Legacy DDS has no colour-space tag and reads as linear (an NVIDIA Texture Tools sRGB flag aside); callers may
//! override it. DX10 sRGB tags are preserved. Role and normal convention are not DDS metadata: reads use `Other` and
//! `None`.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod formats;
mod read;

use primase_core::{ColorSpace, TexFormat, TexRole, Texture, TextureLayers, TextureShape};

pub use formats::{PixelFormat, from_dxgi, legacy_format, legacy_pixel_format, to_dxgi};
pub use read::parse;

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

/// A level's extent along one axis: halved `level` times, down to 1.
fn extent(size: u32, level: u32) -> u32 {
    size.checked_shr(level).unwrap_or(0).max(1)
}

/// The resource a DDS file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// `layers` 1D textures (a DX10 header only); the height is 1.
    D1 { layers: u32 },
    /// `layers` 2D textures.
    D2 { layers: u32 },
    /// `cubes` cube maps, six faces each: +X, -X, +Y, -Y, +Z, -Z.
    Cube { cubes: u32 },
    /// One volume `depth` texels deep; each level halves the depth with the width and height (down to 1).
    D3 { depth: u32 },
}

impl Shape {
    /// The images each level holds: the layers, six per cube, or the level's depth slices.
    pub fn images_at(self, level: u32) -> u64 {
        match self {
            Shape::D1 { layers } | Shape::D2 { layers } => u64::from(layers),
            Shape::Cube { cubes } => u64::from(cubes) * 6,
            Shape::D3 { depth } => u64::from(extent(depth, level)),
        }
    }
}

/// How the alpha channel is to be read (the DX10 header's `miscFlags2`, DirectXTex's `TEX_ALPHA_MODE`). A legacy
/// header states only premultiplied alpha (`DXT2`, `DXT4`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AlphaMode {
    #[default]
    Unknown,
    Straight,
    Premultiplied,
    /// The alpha channel is 1 everywhere, or to be read as 1.
    Opaque,
    /// The alpha channel holds something other than transparency.
    Custom,
}

impl AlphaMode {
    /// The alpha mode a DX10 `miscFlags2` states.
    pub fn from_misc2(value: u32) -> Result<Self> {
        Ok(match value {
            0 => AlphaMode::Unknown,
            1 => AlphaMode::Straight,
            2 => AlphaMode::Premultiplied,
            3 => AlphaMode::Opaque,
            4 => AlphaMode::Custom,
            v => return Err(Error::Unsupported(format!("DX10 miscFlags2 {v:#x}"))),
        })
    }
    /// The DX10 `miscFlags2` value.
    pub fn misc2(self) -> u32 {
        match self {
            AlphaMode::Unknown => 0,
            AlphaMode::Straight => 1,
            AlphaMode::Premultiplied => 2,
            AlphaMode::Opaque => 3,
            AlphaMode::Custom => 4,
        }
    }
}

/// A DDS file's resource: its shape, format and every image of every level, as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct Dds {
    pub shape: Shape,
    pub width: u32,
    /// 1 for [`Shape::D1`].
    pub height: u32,
    pub format: TexFormat,
    pub color_space: ColorSpace,
    pub alpha_mode: AlphaMode,
    /// `levels[level][image]`, largest level first, as [`TextureLayers`] holds them: each level has
    /// [`Shape::images_at`] images (layers, cube faces cube after cube, or depth slices), each exactly the format's
    /// size for the level's width and height.
    pub levels: Vec<Vec<Vec<u8>>>,
}

/// Which header a file had.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderKind {
    /// The Direct3D 9 header, with the pixel format it states the format by.
    Legacy(PixelFormat),
    Dx10,
}

/// What [`parse`] read: the resource, and the facts about the header it was read from.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub dds: Dds,
    pub header: HeaderKind,
    /// The DX10 `arraySize` counted cube faces (6 per cube) rather than cubes, as some writers do (FromSoftware's
    /// environment cubes): DirectXTex reads these only with `DDS_FLAGS_PERMISSIVE` (texconv `-permissive`).
    pub faces_counted: bool,
}

/// Which header [`write()`] writes. Each matches a texconv choice byte for byte where they can.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteHeader {
    /// The legacy header where it states everything the [`Dds`] holds, else DX10: texconv's default, except that
    /// texconv also writes a lone 1D texture under a legacy header (reading back as 2D) and writes no alpha mode
    /// without `-dx10`, where this writes the DX10 header to keep them.
    Auto,
    /// The legacy header, as texconv `-dx9` writes it: for one image, one cube or one volume. The colour space and
    /// alpha mode are dropped where the pixel format can't state them (DXT2/DXT4 are premultiplied BC2/BC3); a format
    /// with no legacy pixel format is refused.
    Legacy,
    /// The DX10 header with the alpha mode, as texconv `-dx10` writes it.
    Dx10,
}

/// A [`Dds`]'s description and images, borrowed: what the writer needs, from a [`Dds`], [`Texture`] or
/// [`TextureLayers`] without copying their pixels.
struct View<'a> {
    shape: Shape,
    width: u32,
    height: u32,
    format: TexFormat,
    color_space: ColorSpace,
    alpha_mode: AlphaMode,
    images: Images<'a>,
}

enum Images<'a> {
    /// One image per level.
    One(&'a [Vec<u8>]),
    Many(&'a [Vec<Vec<u8>>]),
}

impl Images<'_> {
    fn levels(&self) -> usize {
        match self {
            Images::One(l) => l.len(),
            Images::Many(l) => l.len(),
        }
    }
    fn count(&self, level: usize) -> usize {
        match self {
            Images::One(_) => 1,
            Images::Many(l) => l[level].len(),
        }
    }
    fn get(&self, level: usize, image: usize) -> &[u8] {
        match self {
            Images::One(l) => &l[level],
            Images::Many(l) => &l[level][image],
        }
    }
}

impl View<'_> {
    /// Check that the description is a resource Direct3D can hold and the images are exactly its layout.
    fn validate(&self) -> Result<u32> {
        let (count, extent_z) = match self.shape {
            Shape::D1 { layers } | Shape::D2 { layers } => (layers, 1),
            Shape::Cube { cubes } => (cubes, 1),
            Shape::D3 { depth } => (depth, depth),
        };
        if self.width == 0 || self.height == 0 || count == 0 {
            return Err(invalid(
                0,
                format!("{} x {} {:?}", self.width, self.height, self.shape),
            ));
        }
        if matches!(self.shape, Shape::D1 { .. }) && self.height != 1 {
            return Err(invalid(0, format!("1D texture with height {}", self.height)));
        }
        let most = self.width.max(self.height).max(extent_z).ilog2() + 1;
        let levels = self.images.levels();
        if levels == 0 || levels > most as usize {
            return Err(invalid(
                0,
                format!(
                    "{levels} levels for {} x {} {:?} (1 to {most})",
                    self.width, self.height, self.shape
                ),
            ));
        }
        for level in 0..levels {
            let l = level as u32;
            let (w, h) = (extent(self.width, l), extent(self.height, l));
            let images = self.shape.images_at(l);
            if self.images.count(level) as u64 != images {
                return Err(invalid(
                    0,
                    format!(
                        "level {level}: {} images, a {:?} has {images}",
                        self.images.count(level),
                        self.shape
                    ),
                ));
            }
            let size = self
                .format
                .checked_level_size(w, h)
                .ok_or_else(|| invalid(0, format!("level {level} size overflow")))?;
            for i in 0..self.images.count(level) {
                let len = self.images.get(level, i).len();
                if len != size {
                    return Err(invalid(
                        0,
                        format!("level {level} image {i}: {len} bytes, expected {size}"),
                    ));
                }
            }
        }
        Ok(levels as u32)
    }
}

impl Dds {
    fn view(&self) -> View<'_> {
        View {
            shape: self.shape,
            width: self.width,
            height: self.height,
            format: self.format,
            color_space: self.color_space,
            alpha_mode: self.alpha_mode,
            images: Images::Many(&self.levels),
        }
    }

    /// A single 2D texture.
    pub fn from_texture(t: &Texture) -> Dds {
        Dds {
            shape: Shape::D2 { layers: 1 },
            width: t.width,
            height: t.height,
            format: t.format,
            color_space: t.color_space,
            alpha_mode: AlphaMode::Unknown,
            levels: t.levels.iter().map(|l| vec![l.clone()]).collect(),
        }
    }

    /// An array, cube (array) or volume.
    pub fn from_layers(t: &TextureLayers) -> Dds {
        Dds {
            shape: shape_of(t.shape),
            width: t.width,
            height: t.height,
            format: t.format,
            color_space: t.color_space,
            alpha_mode: AlphaMode::Unknown,
            levels: t.levels.clone(),
        }
    }

    /// One 2D (or 1D, as a height of 1) texture. Core textures hold straight alpha, so premultiplied alpha is refused.
    pub fn into_texture(self) -> Result<Texture> {
        self.straight_alpha()?;
        if !matches!(self.shape, Shape::D1 { layers: 1 } | Shape::D2 { layers: 1 }) {
            return Err(Error::Unsupported(format!(
                "{:?} as one 2D texture (read it as layers)",
                self.shape
            )));
        }
        Ok(Texture {
            width: self.width,
            height: self.height,
            format: self.format,
            levels: self.levels.into_iter().flatten().collect(),
            color_space: self.color_space,
            role: TexRole::Other,
            normal: None,
        })
    }

    /// Layers of any shape: 1D textures become 2D ones of height 1 (core has no 1D shape), a single 2D texture an array
    /// of one. Premultiplied alpha is refused, as for [`Dds::into_texture`].
    pub fn into_layers(self) -> Result<TextureLayers> {
        self.straight_alpha()?;
        let shape = match self.shape {
            Shape::D1 { layers } | Shape::D2 { layers } => TextureShape::Array { layers },
            Shape::Cube { cubes } => TextureShape::Cube { cubes },
            Shape::D3 { depth } => TextureShape::Volume { depth },
        };
        Ok(TextureLayers {
            shape,
            width: self.width,
            height: self.height,
            format: self.format,
            levels: self.levels,
            color_space: self.color_space,
            role: TexRole::Other,
            normal: None,
        })
    }

    fn straight_alpha(&self) -> Result<()> {
        if self.alpha_mode == AlphaMode::Premultiplied {
            return Err(Error::Unsupported(
                "premultiplied alpha as a core texture (read it with `parse`)".into(),
            ));
        }
        Ok(())
    }
}

fn shape_of(shape: TextureShape) -> Shape {
    match shape {
        TextureShape::Array { layers } => Shape::D2 { layers },
        TextureShape::Cube { cubes } => Shape::Cube { cubes },
        TextureShape::Volume { depth } => Shape::D3 { depth },
    }
}

/// Read one 2D texture (or a 1D one, as a height of 1); other shapes are refused, see [`read_layers`].
pub fn read(bytes: &[u8]) -> Result<Texture> {
    parse(bytes)?.dds.into_texture()
}

/// Read a texture of any shape as layers (see [`Dds::into_layers`]).
pub fn read_layers(bytes: &[u8]) -> Result<TextureLayers> {
    parse(bytes)?.dds.into_layers()
}

/// Write a DDS file.
pub fn write(dds: &Dds, header: WriteHeader) -> Result<Vec<u8>> {
    emit(&dds.view(), header)
}

/// Write one 2D texture under the legacy header (texconv `-dx9`): the caller chooses to lose the colour space, which a
/// legacy header can't state. Mips are written as supplied.
pub fn write_legacy(t: &Texture) -> Result<Vec<u8>> {
    emit(&texture_view(t), WriteHeader::Legacy)
}

/// Write one 2D texture under the DX10 header, with its DXGI format and colour space.
pub fn write_dx10(t: &Texture) -> Result<Vec<u8>> {
    emit(&texture_view(t), WriteHeader::Dx10)
}

/// Write an array, cube (array) or volume.
pub fn write_layers(t: &TextureLayers, header: WriteHeader) -> Result<Vec<u8>> {
    emit(
        &View {
            shape: shape_of(t.shape),
            width: t.width,
            height: t.height,
            format: t.format,
            color_space: t.color_space,
            alpha_mode: AlphaMode::Unknown,
            images: Images::Many(&t.levels),
        },
        header,
    )
}

fn texture_view(t: &Texture) -> View<'_> {
    View {
        shape: Shape::D2 { layers: 1 },
        width: t.width,
        height: t.height,
        format: t.format,
        color_space: t.color_space,
        alpha_mode: AlphaMode::Unknown,
        images: Images::One(&t.levels),
    }
}

fn put(b: &mut [u8], o: usize, n: u32) {
    b[o..o + 4].copy_from_slice(&n.to_le_bytes());
}

/// The header `EncodeDDSHeader` writes, then the images in file order.
fn emit(v: &View, header: WriteHeader) -> Result<Vec<u8>> {
    let mips = v.validate()?;
    let one = matches!(
        v.shape,
        Shape::D1 { layers: 1 } | Shape::D2 { layers: 1 } | Shape::Cube { cubes: 1 } | Shape::D3 { .. }
    );
    let legacy = match header {
        WriteHeader::Dx10 => None,
        WriteHeader::Legacy => {
            if !one {
                return Err(Error::Unsupported(format!("{:?} under a legacy header", v.shape)));
            }
            let pf = legacy_pixel_format(v.format, v.color_space, v.alpha_mode, true);
            Some(pf.ok_or_else(|| {
                Error::Unsupported(format!(
                    "{:?} {:?} under a legacy header",
                    v.format, v.color_space
                ))
            })?)
        }
        WriteHeader::Auto => {
            let premultiplied_dxtn = v.alpha_mode == AlphaMode::Premultiplied
                && matches!(v.format, TexFormat::Bc2 | TexFormat::Bc3);
            let states_alpha = v.alpha_mode == AlphaMode::Unknown || premultiplied_dxtn;
            if one && states_alpha && !matches!(v.shape, Shape::D1 { .. }) {
                legacy_pixel_format(v.format, v.color_space, v.alpha_mode, false)
            } else {
                None
            }
        }
    };
    let dxgi = match legacy {
        Some(_) => None,
        None => Some(to_dxgi(v.format, v.color_space)?),
    };

    let mut b = vec![0_u8; if dxgi.is_some() { 148 } else { 128 }];
    b[..4].copy_from_slice(b"DDS ");
    put(&mut b, 4, 124);
    let compressed = v.format.is_block_compressed();
    let volume = matches!(v.shape, Shape::D3 { .. });
    let cube = matches!(v.shape, Shape::Cube { .. });
    // DDSD_CAPS | HEIGHT | WIDTH | PIXELFORMAT | MIPMAPCOUNT, and DEPTH for a volume.
    let mut flags = 0x2_1007 | if compressed { 0x8_0000 } else { 0x8 };
    if volume {
        flags |= 0x80_0000;
    }
    put(&mut b, 8, flags);
    put(&mut b, 12, v.height);
    put(&mut b, 16, v.width);
    // A compressed format states the top level's size (one depth slice of a volume), others the row pitch.
    let pitch = if compressed {
        v.format.checked_level_size(v.width, v.height)
    } else {
        v.format.checked_level_size(v.width, 1)
    };
    let pitch = pitch
        .and_then(|p| u32::try_from(p).ok())
        .ok_or_else(|| invalid(20, "pitch exceeds u32"))?;
    put(&mut b, 20, pitch);
    put(
        &mut b,
        24,
        if let Shape::D3 { depth } = v.shape {
            depth
        } else {
            1
        },
    );
    put(&mut b, 28, mips);
    put(&mut b, 76, 32);
    match legacy {
        Some(pf) => {
            put(&mut b, 80, pf.flags);
            b[84..88].copy_from_slice(&pf.fourcc);
            put(&mut b, 88, pf.bits);
            for (i, m) in pf.masks.into_iter().enumerate() {
                put(&mut b, 92 + 4 * i, m);
            }
        }
        None => {
            put(&mut b, 80, formats::FOURCC);
            b[84..88].copy_from_slice(b"DX10");
        }
    }
    // DDSCAPS_TEXTURE, COMPLEX | MIPMAP with mips, COMPLEX for a cube.
    let caps = 0x1000 | if mips > 1 { 0x40_0008 } else { 0 } | if cube { 0x8 } else { 0 };
    put(&mut b, 108, caps);
    let caps2 = if cube {
        0xfe00
    } else if volume {
        0x20_0000
    } else {
        0
    };
    put(&mut b, 112, caps2);
    if let Some(dxgi) = dxgi {
        let (dimension, array) = match v.shape {
            Shape::D1 { layers } => (2, layers),
            Shape::D2 { layers } => (3, layers),
            Shape::Cube { cubes } => (3, cubes),
            Shape::D3 { .. } => (4, 1),
        };
        put(&mut b, 128, dxgi);
        put(&mut b, 132, dimension);
        put(&mut b, 136, if cube { 4 } else { 0 });
        put(&mut b, 140, array);
        put(&mut b, 144, v.alpha_mode.misc2());
    }

    let total: usize = (0..mips as usize)
        .map(|l| {
            (0..v.images.count(l))
                .map(|i| v.images.get(l, i).len())
                .sum::<usize>()
        })
        .sum();
    b.reserve_exact(total);
    if volume {
        for level in 0..mips as usize {
            for i in 0..v.images.count(level) {
                b.extend_from_slice(v.images.get(level, i));
            }
        }
    } else {
        for i in 0..v.images.count(0) {
            for level in 0..mips as usize {
                b.extend_from_slice(v.images.get(level, i));
            }
        }
    }
    Ok(b)
}
