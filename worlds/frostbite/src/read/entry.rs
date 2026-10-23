//! One entry of a bundle read into a world: an EBX document as typed values ([`FrostbiteEbx`]), a texture as a core
//! texture of the levels its chunk holds with its header ([`FrostbiteTexture`]), a ShaderBlockDepot as its keys,
//! blocks and parameters ([`FrostbiteDepot`]), a CompiledBytecode resource as one core shader a DXBC container of it
//! holds (a DXIL container, its interface read from it; [`FrostbiteBytecode`] keeps the bytes around it). Every entity
//! also names where it came from ([`FrostbiteEntry`]). An entry of any other kind or form is refused, named: nothing
//! is guessed.

use primase_core::shader::Shader;
use primase_core::{ColorSpace, Entity, Name, TexFormat, TexRole, Texture, World};

use super::folder::Source;
use super::listing::{Bundle, Listed};
use crate::bundle::Kind;
use crate::components::{
    FrostbiteBytecode, FrostbiteDepot, FrostbiteEbx, FrostbiteEntry, FrostbiteMeshSet, FrostbitePrograms,
    FrostbiteTexture,
};
use crate::texture::{self, Storage};
use crate::{Error, Result, bytecode, depot, lookup, meshset, permutation};

/// The core layout of a Frostbite texture format ordinal (the colour formats the texture reader decodes: 18 and 20
/// RGBA8, 23 and 24 BGRA8, 54-57 BC1, 58 and 59 BC2, 60 and 61 BC3, 66 and 67 BC7); `None` for any other.
pub fn tex_format(format: u32) -> Option<TexFormat> {
    Some(match format {
        18 | 20 => TexFormat::Rgba8Unorm,
        23 | 24 => TexFormat::Bgra8Unorm,
        54..=57 => TexFormat::Bc1,
        58 | 59 => TexFormat::Bc2,
        60 | 61 => TexFormat::Bc3,
        66 | 67 => TexFormat::Bc7,
        _ => return None,
    })
}

/// The colour space a Frostbite format ordinal states, `None` for an ordinal the texture reader does not decode. The
/// game's `RenderFormat` name table (`Skate.exe`, build sha256 `fbce74d5e28ef525`: file offset 0x64d61d8, RVA
/// 0x64d6bd8; 130 names, the ordinal being the position) names each format `_UNORM` or `_SRGB`: 18 `R8G8B8A8_UNORM`,
/// 20 `R8G8B8A8_SRGB`, 23 `B8G8R8A8_UNORM`, 24 `B8G8R8A8_SRGB`, 54 and 55 `BC1`, 56 and 57 `BC1A`, 58 and 59 `BC2`,
/// 60 and 61 `BC3`, 66 and 67 `BC7` (unorm, then sRGB). Every ordinal ReSkate decodes lands on those names, which is
/// how the table's order was checked against an independent source. An sRGB format is [`ColorSpace::Srgb`], the others
/// [`ColorSpace::Linear`]; there is no decoded format whose name does not say.
pub fn format_colour_space(format: u32) -> Option<ColorSpace> {
    Some(match format {
        20 | 24 | 55 | 57 | 59 | 61 | 67 => ColorSpace::Srgb,
        18 | 23 | 54 | 56 | 58 | 60 | 66 => ColorSpace::Linear,
        _ => return None,
    })
}

/// A new entity named `name` that records it was read from `entry`.
pub(super) fn spawn_entity(w: &mut World, entry: &Listed, name: String) -> Entity {
    let e = w.spawn();
    w.insert(e, Name(name));
    w.insert(
        e,
        FrostbiteEntry {
            toc: entry.toc.clone(),
            bundle: entry.bundle.clone(),
            kind: entry.kind,
            name: entry.name.clone(),
            resource_type: entry.resource_type,
            size: entry.size,
            file: entry.file,
        },
    );
    e
}

fn unsupported(entry: &Listed, what: impl std::fmt::Display) -> Error {
    Error::Unsupported(format!("{} ({}): {what}", entry.name, entry.bundle))
}

impl Source<'_> {
    /// `entry` of `bundle` as new entities of `w`: one, except a CompiledBytecode resource holding several containers
    /// (a shader each, named `<name>#<n>`) and a shader permutation (its own entity, then those of the programs it
    /// names). An EBX document, a texture resource, a ShaderBlockDepot and a
    /// CompiledBytecode resource are read; a chunk, another resource type, a texture that is not a 2D map of a colour
    /// format the texture reader decodes, or a container whose interface cannot be read is an [`Error::Unsupported`]
    /// naming it.
    pub fn read_entry(&self, bundle: &Bundle, entry: &Listed, w: &mut World) -> Result<Vec<Entity>> {
        let bytes = || self.payload(entry.file, entry.size as usize);
        let meta = bundle
            .manifest
            .res
            .get(entry.index)
            .filter(|_| entry.kind == Kind::Res)
            .map(|a| a.rmeta);
        // read first, so an entry that can't be read leaves nothing in the world
        enum Read {
            Ebx(Box<FrostbiteEbx>),
            Texture(FrostbiteTexture, Texture),
            Depot(FrostbiteDepot),
            Bytecode(Vec<(Shader, FrostbiteBytecode)>),
            Programs(Box<FrostbitePrograms>),
            MeshSet(Box<FrostbiteMeshSet>),
        }
        let read = match (entry.kind, entry.resource_type) {
            (Kind::Ebx, _) => Read::Ebx(Box::new(FrostbiteEbx {
                document: crate::ebx::read(&bytes()?)?,
            })),
            (Kind::Res, Some(crate::TEXTURE)) => {
                let header = texture::read(&bytes()?)?;
                let pixels = self.texture(bundle, entry, &header)?;
                Read::Texture(FrostbiteTexture { header }, pixels)
            }
            (Kind::Res, Some(depot::RTYPE)) => {
                let meta = meta.ok_or_else(|| unsupported(entry, "no resource metadata"))?;
                let depot = depot::read(&bytes()?, &meta)?;
                let parameters = (depot.blocks.iter())
                    .map(|b| depot::params(&b.data))
                    .collect::<Result<Vec<_>>>()?;
                Read::Depot(FrostbiteDepot { depot, parameters })
            }
            (Kind::Res, Some(bytecode::RTYPE)) => {
                Read::Bytecode(bytecode::read(&bytes()?).map_err(|e| unsupported(entry, e))?)
            }
            (Kind::Res, Some(permutation::RASTER | permutation::COMPUTE)) => {
                return self.spawn_permutation(bundle, entry, w);
            }
            (Kind::Res, Some(lookup::PROGRAM_LOOKUP)) => {
                Read::Programs(Box::new(self.read_database(bundle, entry)?))
            }
            (Kind::Res, Some(crate::MESHSET)) => Read::MeshSet(Box::new(FrostbiteMeshSet {
                meshset: meshset::read(&bytes()?).map_err(|e| unsupported(entry, e))?,
                materials: Default::default(),
            })),
            (Kind::Res, Some(t)) => {
                return Err(unsupported(
                    entry,
                    format_args!("resource type {t:#010x} is not read"),
                ));
            }
            (Kind::Res, None) => return Err(unsupported(entry, "a resource without a type")),
            (Kind::Chunk, _) => {
                return Err(unsupported(
                    entry,
                    "a chunk is read through the resource that names it",
                ));
            }
        };
        let spawn = |w: &mut World, name: String| spawn_entity(w, entry, name);
        let entities = match read {
            Read::Ebx(c) => {
                let e = spawn(w, entry.name.clone());
                w.insert(e, *c);
                vec![e]
            }
            Read::Texture(c, t) => {
                let e = spawn(w, entry.name.clone());
                w.insert(e, t);
                w.insert(e, c);
                vec![e]
            }
            Read::Depot(c) => {
                let e = spawn(w, entry.name.clone());
                w.insert(e, c);
                vec![e]
            }
            Read::Programs(c) => {
                let e = spawn(w, entry.name.clone());
                w.insert(e, *c);
                vec![e]
            }
            Read::MeshSet(c) => {
                let e = spawn(w, entry.name.clone());
                w.insert(e, *c);
                vec![e]
            }
            Read::Bytecode(programs) => {
                let several = programs.len() > 1;
                (programs.into_iter().enumerate())
                    .map(|(i, (shader, c))| {
                        let name = if several {
                            format!("{}#{i}", entry.name)
                        } else {
                            entry.name.clone()
                        };
                        let e = spawn(w, name);
                        w.insert(e, shader);
                        w.insert(e, c);
                        e
                    })
                    .collect()
            }
        };
        Ok(entities)
    }

    /// The core texture of a texture resource: its levels, cut from the chunk the header names. A streamed texture
    /// (`first_mip` above 0) has the whole chain in its TOC's chunk table (`chunk_size` bytes, the levels one after
    /// another from the largest) and only the levels from `first_mip` on in its bundle's manifest (a tail of that
    /// chunk, at the logical offset of the bytes of the levels before them): the whole chain is read, from the TOC, and
    /// refused by name when the TOC does not hold a chunk of that size. The colour space is the format's own
    /// ([`format_colour_space`]); the role is other until a lowering states it.
    fn texture(&self, bundle: &Bundle, entry: &Listed, h: &texture::TextureHeader) -> Result<Texture> {
        if h.kind != 0 || h.depth > 1 || h.slices > 1 {
            return Err(unsupported(
                entry,
                format_args!(
                    "texture of kind {} with depth {} and {} slices: only a 2D map is read",
                    h.kind, h.depth, h.slices
                ),
            ));
        }
        let (Some(format), Some(storage)) = (tex_format(h.format), texture::block(h.format)) else {
            return Err(unsupported(entry, format_args!("texture format {}", h.format)));
        };
        let sizes = &h.mip_sizes[..usize::from(h.mip_count)];
        let total: usize = sizes.iter().map(|&s| s as usize).sum();
        let (file, decoded) = if h.first_mip == 0 {
            bundle.chunk(self, &h.chunk)?
        } else {
            match bundle.toc_chunk(self, &h.chunk)? {
                Some(whole) if whole.1 == total => whole,
                found => {
                    return Err(unsupported(
                        entry,
                        format_args!(
                            "streamed from level {}: the whole chain of {total} bytes is in no chunk of the TOC {} ({})",
                            h.first_mip,
                            bundle.toc_path,
                            found.map_or_else(
                                || "the TOC has no such chunk".to_owned(),
                                |(_, n)| format!("its chunk is {n} bytes")
                            )
                        ),
                    ));
                }
            }
        };
        let chunk = self.payload(file, decoded)?;
        if chunk.len() != total {
            return Err(unsupported(
                entry,
                format_args!("chunk of {} bytes for levels of {sizes:?}", chunk.len()),
            ));
        }
        let (mut at, mut levels) = (0, Vec::with_capacity(sizes.len()));
        for (i, &size) in sizes.iter().enumerate() {
            let (lw, lh) = (
                (u32::from(h.width) >> i).max(1),
                (u32::from(h.height) >> i).max(1),
            );
            if texture::level_size(storage, lw, lh) != Some(size) {
                return Err(unsupported(
                    entry,
                    format_args!(
                        "level {i} of {lw}x{lh} is {size} bytes, not what format {} stores",
                        h.format
                    ),
                ));
            }
            levels.push(chunk[at..at + size as usize].to_vec());
            at += size as usize;
        }
        let _: Storage = storage;
        Ok(Texture {
            width: u32::from(h.width),
            height: u32::from(h.height),
            format,
            levels,
            color_space: format_colour_space(h.format).ok_or_else(|| {
                unsupported(
                    entry,
                    format_args!("texture format {} states no colour space", h.format),
                )
            })?,
            role: TexRole::Other,
            normal: None,
        })
    }
}
