//! Decoding the pixels of a packaged texture copy into core textures.
use primase_core::{ColorSpace, TexRole, Texture};
use primase_texcodec::gcm::{self, Volume};
use rme_formats::texture::{Payload, TextureCatalog};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Catalog(#[from] rme_formats::texture::Error),
    #[error(transparent)]
    Gcm(#[from] gcm::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

/// Caller explicitly names a packaged payload and the texture's colour meaning.
/// Volumes are refused here; use [`decode_volume`] to retain their depth.
pub fn decode(
    catalog: &TextureCatalog<'_>,
    texture: usize,
    payload: Payload,
    color_space: ColorSpace,
    role: TexRole,
) -> Result<Vec<Texture>> {
    let (descriptor, first_mip, bytes) = catalog.pixel_data(texture, payload)?;
    Ok(gcm::decode(descriptor, &bytes, first_mip, color_space, role)?)
}

/// Decode an explicitly selected 3D copy without flattening it into core images.
pub fn decode_volume(
    catalog: &TextureCatalog<'_>,
    texture: usize,
    payload: Payload,
    color_space: ColorSpace,
    role: TexRole,
) -> Result<Volume> {
    let (descriptor, first_mip, bytes) = catalog.pixel_data(texture, payload)?;
    Ok(gcm::decode_volume(
        descriptor,
        &bytes,
        first_mip,
        color_space,
        role,
    )?)
}
