//! TextureMaps and TextureSets read into a world: a TextureMap as a core texture of its levels (its CompiledMips', then
//! its own) with its [`AnvilTextureMap`], colour space linear and role other until a lowering states them.

use primase_core::{ColorSpace, Entity, Name, TexRole, Texture, World};

use crate::components::AnvilTextureMap;
use crate::read::archive::{Archive, At};
use crate::read::material::{StoredTextureSet, TEXTURE_MAP, TEXTURE_SET};
use crate::read::mip::StoredMip;
use crate::read::texture::{self, StoredTextureMap};

/// Why a TextureMap or TextureSet can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Archive(#[from] crate::read::archive::Error),
    #[error("{at:?}: {source}")]
    Texture {
        at: At,
        #[source]
        source: crate::read::texture::Error,
    },
    #[error("{at:?}: {source}")]
    TextureSet {
        at: At,
        #[source]
        source: crate::read::material::TextureSetError,
    },
    #[error("{at:?}: format code {code}: its codec isn't established")]
    Codec { at: At, code: u32 },
    #[error("{at:?}: its CompiledMip for level {level} isn't found, or is another's")]
    Mip { at: At, level: u32 },
    #[error("{at:?}: {source}")]
    MipBody {
        at: At,
        #[source]
        source: crate::read::mip::Error,
    },
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
    let Some(format) = texture::format(t.format) else {
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
    let e = w.spawn();
    if let Some(name) = archive.forge(at.forge).and_then(|f| f.names.get(at.entry)) {
        w.insert(e, Name(name.name.clone()));
    }
    w.insert(
        e,
        Texture {
            width: t.width,
            height: t.height,
            format,
            levels,
            color_space: ColorSpace::Linear,
            role: TexRole::Other,
            normal: None,
        },
    );
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
