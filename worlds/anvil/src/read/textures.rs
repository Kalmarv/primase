//! TextureMaps and TextureSets read into a world: a TextureMap as a core texture of its levels (its CompiledMips', then
//! its own) with its [`AnvilTextureMap`]. The stored transfer flag determines colour space; cubes, arrays and volumes
//! keep their shape in `core.texture_layers`, and the role stays other until a lowering states it.

use primase_core::{
    ColorSpace, Entity, Name, TexFormat, TexRole, Texture, TextureLayers, TextureShape, World,
};

use crate::components::AnvilTextureMap;
use anvil_formats::archive::{Archive, At};
use anvil_formats::material::{StoredTextureSet, TEXTURE_MAP, TEXTURE_SET};
use anvil_formats::mip::StoredMip;
use anvil_formats::texture::{self, StoredTextureMap};

/// Why a TextureMap or TextureSet can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Archive(#[from] anvil_formats::archive::Error),
    #[error("{at:?}: {source}")]
    Texture {
        at: At,
        #[source]
        source: anvil_formats::texture::Error,
    },
    #[error("{at:?}: {source}")]
    TextureSet {
        at: At,
        #[source]
        source: anvil_formats::material::TextureSetError,
    },
    #[error("{at:?}: unsupported or inconsistent layered texture layout ({layers} layers)")]
    Layers { at: At, layers: u32 },
    #[error("{at:?}: format code {code}: its codec isn't established")]
    Codec { at: At, code: u32 },
    #[error("{at:?}: its CompiledMip for level {level} isn't found, or is another's")]
    Mip { at: At, level: u32 },
    #[error("{at:?}: {source}")]
    MipBody {
        at: At,
        #[source]
        source: anvil_formats::mip::Error,
    },
}

/// Core's texture format for a TextureMap's format.
pub fn tex_format(format: texture::Format) -> TexFormat {
    match format {
        texture::Format::Bgra8Unorm => TexFormat::Bgra8Unorm,
        texture::Format::Rgba8Snorm => TexFormat::Rgba8Snorm,
        texture::Format::Bc1 => TexFormat::Bc1,
        texture::Format::Bc2 => TexFormat::Bc2,
        texture::Format::Bc3 => TexFormat::Bc3,
        texture::Format::Bc4Unorm => TexFormat::Bc4Unorm,
        texture::Format::Bc5Unorm => TexFormat::Bc5Unorm,
        texture::Format::Bc6hUfloat => TexFormat::Bc6hUfloat,
        texture::Format::Bc6hSfloat => TexFormat::Bc6hSfloat,
        texture::Format::Bc7 => TexFormat::Bc7,
        texture::Format::A8Unorm => TexFormat::A8Unorm,
        texture::Format::R8Unorm => TexFormat::R8Unorm,
        texture::Format::Rgba16Float => TexFormat::Rgba16Float,
    }
}

/// The TextureMap `id` names as a new entity: `None` where `id` is in no forge or isn't a TextureMap.
pub fn read_texture_map(archive: &mut Archive, w: &mut World, id: u64) -> Result<Option<Entity>, Error> {
    let Some(at) = archive.find(id) else {
        return Ok(None);
    };
    let r = archive.resource(at)?;
    if r.type_id != TEXTURE_MAP {
        return Ok(None);
    }
    let t = StoredTextureMap::parse(&r.body).map_err(|source| Error::Texture { at, source })?;
    if !t.has_image() {
        return Ok(None);
    }
    let Some(format) = texture::format(t.format).map(tex_format) else {
        return Err(Error::Codec { at, code: t.format });
    };
    let mut levels = Vec::with_capacity(usize::try_from(t.mip_count).unwrap_or(0));
    for (level, m) in (0u32..).zip(&t.references) {
        let mip_at = archive.find(m.id).ok_or(Error::Mip { at, level })?;
        let body = archive.resource(mip_at)?.body;
        let mip = StoredMip::parse(&body).map_err(|source| Error::MipBody { at: mip_at, source })?;
        if mip.parent != id || mip.level != level {
            return Err(Error::Mip { at, level });
        }
        levels.push(mip.image);
    }
    levels.extend(
        t.levels()
            .map_err(|source| Error::Texture { at, source })?
            .into_iter()
            .map(<[u8]>::to_vec),
    );
    let color_space = if t.field_25 == 1 {
        ColorSpace::Srgb
    } else {
        ColorSpace::Linear
    };
    let layered = if t.dimension == 1 && t.array_size == 1 && t.depth == 1 {
        None
    } else {
        Some(match t.dimension {
            2 if t.depth == 1 => TextureShape::Cube { cubes: t.array_size },
            1 if t.depth == 1 => TextureShape::Array { layers: t.array_size },
            3 if levels.len() == 1 && t.array_size == 1 => TextureShape::Volume { depth: t.depth },
            _ => {
                return Err(Error::Layers {
                    at,
                    layers: t.layers(),
                });
            }
        })
    };
    // Validate every layer before publishing an entity. A malformed map must not leave a named empty object.
    let split = layered
        .map(|shape| split_layers(t.width, t.height, shape, format, &levels))
        .transpose()
        .map_err(|_| Error::Layers {
            at,
            layers: t.layers(),
        })?;
    let e = w.spawn();
    if let Some(name) = archive.forge(at.forge).and_then(|f| f.names.get(at.entry)) {
        w.insert(e, Name(name.name.clone()));
    }
    if let (Some(shape), Some(levels)) = (layered, split) {
        w.insert(
            e,
            TextureLayers {
                width: t.width,
                height: t.height,
                shape,
                format,
                levels,
                color_space,
                role: TexRole::Other,
                normal: None,
            },
        );
    } else {
        w.insert(
            e,
            Texture {
                width: t.width,
                height: t.height,
                format,
                levels,
                color_space,
                role: TexRole::Other,
                normal: None,
            },
        );
    }
    w.insert(
        e,
        AnvilTextureMap {
            index_id: id,
            code: t.format,
            field_25: t.field_25,
        },
    );
    Ok(Some(e))
}

fn split_layers(
    width: u32,
    height: u32,
    shape: TextureShape,
    format: primase_core::TexFormat,
    levels: &[Vec<u8>],
) -> Result<Vec<Vec<Vec<u8>>>, String> {
    levels
        .iter()
        .enumerate()
        .map(|(level, bytes)| {
            let shift = u32::try_from(level).map_err(|_| "mip index overflow")?;
            let width = width.checked_shr(shift).unwrap_or(0).max(1);
            let height = height.checked_shr(shift).unwrap_or(0).max(1);
            let length = format
                .checked_level_size(width, height)
                .ok_or("image length overflow")?;
            let count = shape.layers_at(level) as usize;
            if length == 0 || count == 0 || length.checked_mul(count) != Some(bytes.len()) {
                return Err(format!("mip {level}: inconsistent encoded layer length"));
            }
            Ok(bytes.chunks_exact(length).map(<[u8]>::to_vec).collect())
        })
        .collect()
}

/// The TextureMap in slot `slot` of the TextureSet `id` names, read as [`read_texture_map`] reads it: `None` where
/// `id` is in no forge or isn't a TextureSet, or the slot is empty.
pub fn read_texture_set_slot(
    archive: &mut Archive,
    w: &mut World,
    id: u64,
    slot: usize,
) -> Result<Option<Entity>, Error> {
    let Some(at) = archive.find(id) else {
        return Ok(None);
    };
    let r = archive.resource(at)?;
    if r.type_id != TEXTURE_SET {
        return Ok(None);
    }
    let set = StoredTextureSet::parse(&r.body).map_err(|source| Error::TextureSet { at, source })?;
    match set.references.get(slot).copied().flatten() {
        Some(map) => read_texture_map(archive, w, map.id),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::TexFormat;

    #[test]
    fn cube_mips_preserve_encoded_blocks_face_order_and_truncation_is_refused() {
        let faces: Vec<_> = (0..6u8).flat_map(|face| [face; 8]).collect();
        let split = split_layers(
            4,
            4,
            TextureShape::Cube { cubes: 1 },
            TexFormat::Bc1,
            &[faces.clone(), faces.clone()],
        )
        .unwrap();
        assert_eq!(split.len(), 2);
        for level in &split {
            for (face, image) in level.iter().enumerate() {
                assert_eq!(image, &[face as u8; 8]);
            }
        }
        assert!(
            split_layers(
                4,
                4,
                TextureShape::Cube { cubes: 1 },
                TexFormat::Bc1,
                &[faces[..47].to_vec()]
            )
            .is_err()
        );
    }

    #[test]
    fn volume_slices_shrink_with_their_mip_without_changing_their_bytes() {
        let split = split_layers(
            2,
            2,
            TextureShape::Volume { depth: 2 },
            TexFormat::Rgba8Unorm,
            &[vec![1; 32], vec![2; 4]],
        )
        .unwrap();
        assert_eq!(split[0], vec![vec![1; 16], vec![1; 16]]);
        assert_eq!(split[1], vec![vec![2; 4]]);
    }
}
