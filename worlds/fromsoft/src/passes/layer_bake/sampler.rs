//! The layer bake's named offline sampling policy, over the executor's shared texture
//! ([`primase_shader_exec::SampledTexture`]): the base mip, bilinear filtering as Direct3D 11 filters (the texel
//! coordinate truncated to 8 sub-texel bits, [`Subtexel::D3d11`], as the GPU and WARP do; the arithmetic in f32,
//! [`primase_image::sampling`]'s mechanism), the addressing along each axis the binding states (wrap where it states
//! none: the caller counts it). Albedo RGB is linearized with the shared sRGB transfer before filtering; alpha and data
//! textures remain linear. Runtime derivatives are not inferred from the shader resource names. This is not runtime
//! sampler state.
//!
//! A texture keeps the stored texels it was made from ([`stored`]), so a bake's pipeline state
//! ([`super::PipelineState`]) carries the very pixels it samples.

use primase_core::shader::{AddressMode, SamplerState};
use primase_image::sampling::{Address, Levels, Subtexel, Transfer};
use primase_shader_exec::{Filtering, Image, Lod, SampledTexture, SamplerUse, Unstated};

use crate::{Result, invalid};

/// The policy's filtering: the base mip, bilinear with Direct3D 11's sub-texel precision, whatever level of detail
/// the program states.
pub const FILTERING: Filtering = Filtering::BaseBilinear {
    subtexel: Subtexel::D3d11,
};

/// A texture's RGBA8 base level (`width` x `height`, rows), sampled by the policy along `address` (u, v), its colour
/// lanes sRGB-decoded first where `albedo_srgb`.
pub fn base_texture(
    width: u32,
    height: u32,
    rgba: &[u8],
    albedo_srgb: bool,
    address: [Address; 2],
) -> Result<SampledTexture> {
    let levels =
        Levels::one(width, height, rgba.to_vec()).map_err(|e| invalid("layer sampler", 0, e.to_string()))?;
    let transfer = if albedo_srgb {
        Transfer::SrgbColour
    } else {
        Transfer::Stored
    };
    SampledTexture::new(levels, transfer, SamplerUse::addressed(address), FILTERING)
        .map_err(|e| invalid("layer sampler", 0, e))
}

/// [`base_texture`] wrapping along both axes.
pub fn wrapped_texture(width: u32, height: u32, rgba: &[u8], albedo_srgb: bool) -> Result<SampledTexture> {
    base_texture(width, height, rgba, albedo_srgb, [Address::Wrap; 2])
}

/// The stored texels a texture was made from, RGBA8 in rows ([`base_texture`]'s base level).
pub fn stored(texture: &SampledTexture) -> &[u8] {
    &texture.levels().levels[0]
}

/// Whether a texture's colour lanes are sRGB-encoded (linearized before filtering); alpha is linear either way.
pub fn srgb(texture: &SampledTexture) -> bool {
    texture.transfer() == Transfer::SrgbColour
}

/// A texture sampled at `uv` by the policy (the base mip, whatever the level of detail).
pub fn sample(texture: &SampledTexture, uv: [f32; 2]) -> Result<[f32; 4]> {
    texture
        .sample(uv, Lod::Implicit)
        .map_err(|e| invalid("layer sampler", 0, e))
}

/// The sampling addresses a binding's addressing is ([`SamplerUse::of`]): an axis it states none for wraps (the
/// caller counts it); a border needs the sampler's border colour, which the source does not store.
pub(crate) fn addressing(stated: [Option<AddressMode>; 2]) -> Result<[Address; 2]> {
    let state = SamplerState {
        wrap: [stated[0], stated[1], None],
        ..SamplerState::default()
    };
    SamplerUse::of(&state, Unstated::Assume(Address::Wrap))
        .map(|s| s.address)
        .map_err(|e| invalid("layer sampler", 0, e))
}
