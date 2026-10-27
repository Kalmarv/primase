//! One entry of a bundle read into a world: an EBX document as typed values ([`FrostbiteEbx`]), a texture as a core
//! texture of the levels its chunk holds (a [`Texture`] for a 2D map, a [`TextureLayers`] for an array, a cube or a
//! volume) with its header ([`FrostbiteTexture`]), a ShaderBlockDepot as its keys,
//! blocks and parameters ([`FrostbiteDepot`]), a CompiledBytecode resource as one core shader a DXBC container of it
//! holds (a DXIL container, its interface read from it; [`FrostbiteBytecode`] keeps the bytes around it). Every entity
//! also names where it came from ([`FrostbiteEntry`]). An entry of any other kind or form is refused, named: nothing
//! is guessed.

use primase_core::shader::Shader;
use primase_core::{
    ColorSpace, Entity, Name, TexFormat, TexRole, Texture, TextureLayers, TextureShape, World,
};

use crate::components::{
    FrostbiteBytecode, FrostbiteDepot, FrostbiteEbx, FrostbiteEntry, FrostbiteMeshSet, FrostbitePrograms,
    FrostbiteTexture,
};
use frostbite_formats::bundle::Kind;
use frostbite_formats::folder::Source;
use frostbite_formats::listing::{Bundle, Listed};
use frostbite_formats::texture::{self, Storage};
use frostbite_formats::{Error, Result, depot, lookup, meshset, permutation};

use super::program::{read_database, spawn_permutation};
use crate::bytecode;

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

/// The pixels of a texture resource: one 2D image, or several.
enum Pixels {
    Flat(Texture),
    Layered(TextureLayers),
}

/// Whether [`read_entry`] reads a resource of type `t`: a texture, a ShaderBlockDepot, a CompiledBytecode
/// resource, a shader permutation (raster or compute), a program lookup or a MeshSet. Every other type is an
/// [`Error::Unsupported`] naming it, so a listing that offers entries to open leaves these out.
pub fn reads_resource_type(t: u32) -> bool {
    matches!(
        t,
        frostbite_formats::TEXTURE
            | depot::RTYPE
            | frostbite_formats::bytecode::RTYPE
            | permutation::RASTER
            | permutation::COMPUTE
            | lookup::PROGRAM_LOOKUP
            | frostbite_formats::MESHSET
    )
}

fn unsupported(entry: &Listed, what: impl std::fmt::Display) -> Error {
    Error::Unsupported(format!("{} ({}): {what}", entry.name, entry.bundle))
}

/// `entry` of `bundle` as new entities of `w`: one, except a CompiledBytecode resource holding several containers
/// (a shader each, named `<name>#<n>`) and a shader permutation (its own entity, then those of the programs it
/// names). An EBX document, a texture resource, a ShaderBlockDepot and a
/// CompiledBytecode resource are read; a chunk, another resource type, a texture of a shape [`texture_shape`] does
/// not know or a colour format the texture reader does not decode, or a container whose interface cannot be read is
/// an [`Error::Unsupported`] naming it.
pub fn read_entry(src: &Source<'_>, bundle: &Bundle, entry: &Listed, w: &mut World) -> Result<Vec<Entity>> {
    let bytes = || src.payload(entry.file, entry.size as usize);
    let meta = bundle
        .manifest
        .res
        .get(entry.index)
        .filter(|_| entry.kind == Kind::Res)
        .map(|a| a.rmeta);
    // read first, so an entry that can't be read leaves nothing in the world
    enum Read {
        Ebx(Box<FrostbiteEbx>),
        Texture(FrostbiteTexture, Pixels),
        Depot(FrostbiteDepot),
        Bytecode(Vec<(Shader, FrostbiteBytecode)>),
        Programs(Box<FrostbitePrograms>),
        MeshSet(Box<FrostbiteMeshSet>),
    }
    let read = match (entry.kind, entry.resource_type) {
        (Kind::Ebx, _) => Read::Ebx(Box::new(FrostbiteEbx {
            document: frostbite_formats::ebx::read(&bytes()?)?,
        })),
        (Kind::Res, Some(frostbite_formats::TEXTURE)) => {
            let header = texture::read(&bytes()?)?;
            let pixels = texture(src, bundle, entry, &header)?;
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
        (Kind::Res, Some(frostbite_formats::bytecode::RTYPE)) => {
            Read::Bytecode(bytecode::read(&bytes()?).map_err(|e| unsupported(entry, e))?)
        }
        (Kind::Res, Some(permutation::RASTER | permutation::COMPUTE)) => {
            return spawn_permutation(src, bundle, entry, w);
        }
        (Kind::Res, Some(lookup::PROGRAM_LOOKUP)) => {
            Read::Programs(Box::new(read_database(src, bundle, entry)?))
        }
        (Kind::Res, Some(frostbite_formats::MESHSET)) => Read::MeshSet(Box::new(FrostbiteMeshSet {
            meshset: meshset::read(&bytes()?).map_err(|e| unsupported(entry, e))?,
            materials: Default::default(),
        })),
        (Kind::Res, Some(t)) => {
            debug_assert!(!reads_resource_type(t), "{t:#010x} is read above");
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
            if let Some((name, joints)) =
                frostbite_formats::skeleton::read(&c.document).map_err(|err| unsupported(entry, err))?
            {
                w.insert(e, crate::components::FrostbiteSkeleton { name, joints });
            }
            w.insert(e, *c);
            vec![e]
        }
        Read::Texture(c, pixels) => {
            let e = spawn(w, entry.name.clone());
            match pixels {
                Pixels::Flat(t) => _ = w.insert(e, t),
                Pixels::Layered(t) => _ = w.insert(e, t),
            }
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

/// The pixels of a texture resource, cut from the chunk the header names: a [`Texture`] for a 2D map, else a
/// [`TextureLayers`] (kind 3 an array, 1 a cube, 2 a volume; [`texture_shape`]). The chunk holds the levels one
/// after another from the largest and, within a level, its images one after another: for an array or a cube
/// every image of a level is `mip_sizes[level]` bytes, and the images of a level lie together (mip-major: the
/// level's images, then the next level's). That order was measured, not assumed: on 559 arrays and cubes of the
/// build (558 arrays and one cube; the others have a single layer or a format the texture reader does not decode)
/// the mip-major reading of the second image's levels 0 and 1 is a downsample of each other (mean error 0.2 to
/// 6.7 of 255), the image-major reading is not (5 to 80), and every one of the 578 non-2D chunks has the size
/// this order implies. A volume's `mip_sizes[level]` is the whole level, all its depth slices (`mip_sizes[0]` of a
/// 128 deep BC7 volume is 128 times one slice); the nine volumes of the build have one level, so a volume of
/// several levels (the order of whose levels no data shows) is refused. A streamed texture (`first_mip` above 0)
/// has the whole chain in its TOC's chunk table (every level and image) and only the levels from `first_mip` on in
/// its bundle's manifest (a tail of that chunk, at the logical offset of the bytes of the levels before them): the
/// whole chain is read, from the TOC, and refused by name when the TOC does not hold a chunk of that size. The
/// colour space is the format's own ([`format_colour_space`]); the role is other until a lowering states it.
fn texture(src: &Source<'_>, bundle: &Bundle, entry: &Listed, h: &texture::TextureHeader) -> Result<Pixels> {
    let shape = texture_shape(h).map_err(|why| unsupported(entry, why))?;
    let (Some(format), Some(storage)) = (tex_format(h.format), texture::block(h.format)) else {
        return Err(unsupported(entry, format_args!("texture format {}", h.format)));
    };
    let images = |level: usize| shape.map_or(1, |s| s.layers_at(level)) as usize;
    let sizes = &h.mip_sizes[..usize::from(h.mip_count)];
    // the bytes of one image of each level
    let mut image = Vec::with_capacity(sizes.len());
    for (i, &size) in sizes.iter().enumerate() {
        let (lw, lh) = (
            (u32::from(h.width) >> i).max(1),
            (u32::from(h.height) >> i).max(1),
        );
        // the header states a volume's whole level, an array's or a cube's one image
        let whole = if matches!(shape, Some(TextureShape::Volume { .. })) {
            images(i)
        } else {
            1
        };
        let one = texture::level_size(storage, lw, lh).map(|n| n as usize);
        if one.and_then(|n| n.checked_mul(whole)) != Some(size as usize) {
            return Err(unsupported(
                entry,
                format_args!(
                    "level {i} of {lw}x{lh} is {size} bytes, not what format {} stores",
                    h.format
                ),
            ));
        }
        image.push(one.unwrap_or_default());
    }
    let total: usize = image.iter().enumerate().map(|(i, n)| n * images(i)).sum();
    let (file, decoded) = if h.first_mip == 0 {
        bundle.chunk(src, &h.chunk)?
    } else {
        match bundle.toc_chunk(src, &h.chunk)? {
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
    let chunk = src.payload(file, decoded)?;
    if chunk.len() != total {
        return Err(unsupported(
            entry,
            format_args!("chunk of {} bytes for levels of {sizes:?}", chunk.len()),
        ));
    }
    let (mut from, mut levels) = (0, Vec::with_capacity(sizes.len()));
    for (i, &n) in image.iter().enumerate() {
        levels.push(
            (0..images(i))
                .map(|k| chunk[from + k * n..from + (k + 1) * n].to_vec())
                .collect::<Vec<_>>(),
        );
        from += n * images(i);
    }
    let _: Storage = storage;
    let color_space = format_colour_space(h.format).ok_or_else(|| {
        unsupported(
            entry,
            format_args!("texture format {} states no colour space", h.format),
        )
    })?;
    let (width, height) = (u32::from(h.width), u32::from(h.height));
    Ok(match shape {
        None => Pixels::Flat(Texture {
            width,
            height,
            format,
            levels: levels.into_iter().flatten().collect(),
            color_space,
            role: TexRole::Other,
            normal: None,
        }),
        Some(shape) => Pixels::Layered(TextureLayers {
            shape,
            width,
            height,
            format,
            levels,
            color_space,
            role: TexRole::Other,
            normal: None,
        }),
    })
}

/// What a texture header's kind, depth and slices make of the texture: `None` for a 2D map, else the shape of its
/// images. Kind 3 is an array (`slices` layers, `depth` the same number), kind 1 a cube (6 and 6), kind 2 a volume
/// (`depth` slices deep, `slices` the same number) of one level. Refused, with the reason: a count the kind does not
/// state (a 2D map with depth or slices above 1, a cube of other than six faces, an array or volume whose depth and
/// slices differ), a volume of several levels and any other kind.
fn texture_shape(h: &texture::TextureHeader) -> std::result::Result<Option<TextureShape>, String> {
    let counts = format!("kind {} with depth {} and {} slices", h.kind, h.depth, h.slices);
    Ok(match h.kind {
        0 if h.depth <= 1 && h.slices <= 1 => None,
        1 if (h.depth, h.slices) == (6, 6) => Some(TextureShape::Cube { cubes: 1 }),
        2 if h.depth == h.slices && h.depth >= 1 => {
            if h.mip_count > 1 {
                return Err(format!(
                    "texture of {counts} and {} levels: no volume of several levels is known, so the order of its levels is not",
                    h.mip_count
                ));
            }
            Some(TextureShape::Volume {
                depth: u32::from(h.depth),
            })
        }
        3 if h.depth == h.slices && h.slices >= 1 => Some(TextureShape::Array {
            layers: u32::from(h.slices),
        }),
        _ => return Err(format!("texture of {counts}: not a shape the reader knows")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The resource types the reader has an arm for are the ones it says it reads, and the three the lab's catalog sweep
    /// found refused (ShaderStateAsset, GpuDeformBaseMesh and the scalp-hair wrap list) are not among them.
    #[test]
    fn the_resource_types_read_are_the_ones_with_an_arm() {
        for t in [
            frostbite_formats::TEXTURE,
            depot::RTYPE,
            frostbite_formats::bytecode::RTYPE,
            permutation::RASTER,
            permutation::COMPUTE,
            lookup::PROGRAM_LOOKUP,
            frostbite_formats::MESHSET,
        ] {
            assert!(reads_resource_type(t), "{t:#010x}");
        }
        for t in [0x2fec_6526, 0xed27_dbda, 0xf17d_dabd, 0] {
            assert!(!reads_resource_type(t), "{t:#010x}");
        }
    }
}
