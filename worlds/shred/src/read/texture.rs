//! Texture headers and all packaged copies, without selecting a residency or quality.
use super::pak::Pak;
use primase_core::{ColorSpace, TexRole, Texture};
use primase_texcodec::gcm::CellGcmTexture;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Archive(#[from] crate::Error),
    #[error("texture header at {offset:#x}: {detail}")]
    Header { offset: usize, detail: &'static str },
    #[error(transparent)]
    Gcm(#[from] primase_texcodec::gcm::Error),
    #[error("texture {texture}: {detail}")]
    Copy { texture: usize, detail: &'static str },
}
pub type Result<T> = std::result::Result<T, Error>;

/// Exact source header. Unknown words are explicitly exposed, never interpreted
/// as resource identifiers, names or flags. `raw` also retains the mip-end table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureHeader {
    pub raw: [u8; 84],
    pub gcm: CellGcmTexture,
    /// Unresolved words at 0x10 (two u16s), 0x1c and 0x20.
    pub opaque_words: [(usize, u32); 3],
}

impl TextureHeader {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let raw: [u8; 84] = bytes.try_into().map_err(|_| Error::Header {
            offset: 0,
            detail: "expected exactly 84 bytes",
        })?;
        let word = |i| u32::from_be_bytes([raw[i], raw[i + 1], raw[i + 2], raw[i + 3]]);
        for (offset, expected) in [
            (0, 0xafaf0003),
            (4, 84),
            (8, 0x003c0000),
            (12, 0xbeefface),
            (52, 0),
            (56, 60),
        ] {
            if word(offset) != expected {
                return Err(Error::Header {
                    offset,
                    detail: "header constant mismatch",
                });
            }
        }
        let gcm = CellGcmTexture::parse(&raw[60..])?;
        if u16::from_be_bytes([raw[20], raw[21]]) != gcm.width
            || u16::from_be_bytes([raw[22], raw[23]]) != gcm.height
        {
            return Err(Error::Header {
                offset: 20,
                detail: "header and GCM extents differ",
            });
        }
        let opaque_words = [(16, word(16)), (28, word(28)), (32, word(32))];
        Ok(Self {
            raw,
            gcm,
            opaque_words,
        })
    }

    /// Prefetch keeps the first mip whose smaller axis is at most 128, then
    /// every remaining mip. A single-mip texture keeps that mip regardless of size.
    pub fn tail_first_mip(&self) -> u8 {
        let g = self.gcm;
        if g.mipmap <= 1 {
            return 0;
        }
        (0..g.mipmap.min(16))
            .find(|&m| (g.width >> m).max(1).min((g.height >> m).max(1)) <= 128)
            .unwrap_or(g.mipmap)
    }
}

/// Where the source packaged this copy; no preference is implied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Residency {
    Streamed,
    Prefetch,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelSource {
    pub entry: usize,
    pub first_mip: u8,
}

#[derive(Clone, Debug)]
pub struct TextureRecord {
    pub archive: usize,
    pub header_entry: usize,
    pub header: TextureHeader,
    /// Source path without extension, retaining original bytes and separators.
    pub path: Vec<u8>,
    pub section: Option<usize>,
    pub residency: Residency,
    /// None is a header-only entry, not an empty or silently substituted texture.
    pub pixels: Option<PixelSource>,
    /// A cube's separately packaged `_m1.txp` (same tail as prefetch).
    pub cube_tail: Option<PixelSource>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Payload {
    Primary,
    CubeTail,
}

/// Index over exactly the archives the caller supplies. Duplicate paths and
/// duplicate packages remain separate records. Labels identify caller inputs.
#[derive(Debug)]
pub struct TextureCatalog<'a> {
    archives: Vec<(&'a str, &'a Pak)>,
    records: Vec<TextureRecord>,
}

impl<'a> TextureCatalog<'a> {
    pub fn new(archives: &[(&'a str, &'a Pak)]) -> Result<Self> {
        let mut records = Vec::new();
        for (archive, (_, pak)) in archives.iter().enumerate() {
            for (header_entry, entry) in pak.entries().iter().enumerate() {
                let Some(path) = entry.name.strip_suffix(b".thp") else {
                    continue;
                };
                let header = TextureHeader::parse(&pak.payload(header_entry)?)?;
                let section_name = entry.section.map(|s| pak.sections()[s].name.as_slice());
                let residency = match section_name {
                    Some(b"TEXTURES_STREAMED" | b"ACTORS_STREAMED") => Residency::Streamed,
                    Some(b"TEXTURES_PREFETCH" | b"ACTORS_PREFETCH") => Residency::Prefetch,
                    _ => Residency::Other,
                };
                let first_mip = if residency == Residency::Prefetch {
                    header.tail_first_mip()
                } else {
                    0
                };
                let mut pixel_name = path.to_vec();
                pixel_name.extend_from_slice(b".txp");
                let mut tail_name = path.to_vec();
                tail_name.extend_from_slice(b"_m1.txp");
                // Flash headers are in MAIN/CACHE_MAIN and pixels in LOCAL/CACHE_LOCAL.
                // These are named source sections, not a fallback to another residency.
                let local = match section_name {
                    Some(b"MAIN") => Some(b"LOCAL".as_slice()),
                    Some(b"CACHE_MAIN") => Some(b"CACHE_LOCAL".as_slice()),
                    _ => None,
                };
                let matches_section = |section: Option<usize>| {
                    section == entry.section
                        || local.is_some_and(|name| section.is_some_and(|s| pak.sections()[s].name == name))
                };
                let find = |name: &[u8], first_mip| -> Result<Option<PixelSource>> {
                    let mut found = pak
                        .entries()
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| e.name == name && matches_section(e.section));
                    let result = found.next().map(|(entry, _)| PixelSource { entry, first_mip });
                    if found.next().is_some() {
                        return Err(Error::Copy {
                            texture: records.len(),
                            detail: "ambiguous pixel entries in matching sections",
                        });
                    }
                    Ok(result)
                };
                let pixels = find(&pixel_name, first_mip)?;
                let cube_tail = if header.gcm.cubemap == 1 && residency == Residency::Streamed {
                    find(&tail_name, header.tail_first_mip())?
                } else {
                    None
                };
                records.push(TextureRecord {
                    archive,
                    header_entry,
                    header,
                    path: path.to_vec(),
                    section: entry.section,
                    residency,
                    pixels,
                    cube_tail,
                });
            }
        }
        Ok(Self {
            archives: archives.to_vec(),
            records,
        })
    }

    pub fn records(&self) -> &[TextureRecord] {
        &self.records
    }
    pub fn archive_label(&self, index: usize) -> Option<&str> {
        self.archives.get(index).map(|a| a.0)
    }

    /// All source records of this extensionless material texture path. `/` and
    /// `\` compare equally; case and all other source bytes remain significant.
    pub fn resolve(&self, path: &str) -> impl Iterator<Item = (usize, &TextureRecord)> {
        let path: Vec<u8> = path.bytes().map(|b| if b == b'\\' { b'/' } else { b }).collect();
        self.records.iter().enumerate().filter(move |(_, r)| {
            r.path
                .iter()
                .map(|&b| if b == b'\\' { b'/' } else { b })
                .eq(path.iter().copied())
        })
    }

    /// Caller explicitly names a packaged payload and the texture's colour meaning.
    /// Volumes are refused here; use `decode_volume` to retain their depth.
    pub fn decode(
        &self,
        texture: usize,
        payload: Payload,
        color_space: ColorSpace,
        role: TexRole,
    ) -> Result<Vec<Texture>> {
        let (gcm, first_mip, bytes) = self.pixel_data(texture, payload)?;
        Ok(gcm.decode(&bytes, first_mip, color_space, role)?)
    }

    /// Decode an explicitly selected 3D copy without flattening it into core images.
    pub fn decode_volume(
        &self,
        texture: usize,
        payload: Payload,
        color_space: ColorSpace,
        role: TexRole,
    ) -> Result<primase_texcodec::gcm::Volume> {
        let (gcm, first_mip, bytes) = self.pixel_data(texture, payload)?;
        Ok(gcm.decode_volume(&bytes, first_mip, color_space, role)?)
    }

    fn pixel_data(
        &self,
        texture: usize,
        payload: Payload,
    ) -> Result<(CellGcmTexture, u8, std::borrow::Cow<'_, [u8]>)> {
        let r = self.records.get(texture).ok_or(Error::Copy {
            texture,
            detail: "record index out of bounds",
        })?;
        let pixels = match payload {
            Payload::Primary => r.pixels,
            Payload::CubeTail => r.cube_tail,
        }
        .ok_or(Error::Copy {
            texture,
            detail: "requested pixel payload is absent",
        })?;
        let bytes = self.archives[r.archive].1.payload(pixels.entry)?;
        Ok((r.header.gcm, pixels.first_mip, bytes))
    }
}
