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
use crate::components::{FrostbiteBytecode, FrostbiteDepot, FrostbiteEbx, FrostbiteEntry, FrostbiteTexture};
use crate::texture::{self, Storage};
use crate::{Error, Result, bytecode, depot};

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

fn unsupported(entry: &Listed, what: impl std::fmt::Display) -> Error {
    Error::Unsupported(format!("{} ({}): {what}", entry.name, entry.bundle))
}

impl Source<'_> {
    /// `entry` of `bundle` as new entities of `w`: one, except a CompiledBytecode resource holding several containers
    /// (a shader each, named `<name>#<n>`). An EBX document, a texture resource, a ShaderBlockDepot and a
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
        let spawn = |w: &mut World, name: String| {
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
        };
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

    /// The core texture of a texture resource: its header's levels, cut from the chunk the header names (the levels
    /// the chunk holds, from level `first_mip` of the full map: the header's size is the first level's). Colour space
    /// linear and role other until a lowering states them.
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
        let (file, decoded) = bundle.chunk(self, &h.chunk)?;
        let chunk = self.payload(file, decoded)?;
        let sizes = &h.mip_sizes[..usize::from(h.mip_count)];
        if chunk.len() != sizes.iter().map(|&s| s as usize).sum::<usize>() {
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
            color_space: ColorSpace::Linear,
            role: TexRole::Other,
            normal: None,
        })
    }
}
