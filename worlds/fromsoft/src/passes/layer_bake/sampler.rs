use std::sync::Arc;

use primase_image::sampling::{Address, Decoded, Levels, Subtexel, Transfer};

use crate::{Result, invalid};

/// Named offline policy: base mip, bilinear filtering as Direct3D 11 filters (the texel coordinate truncated to 8
/// sub-texel bits, [`Subtexel::D3d11`], as the GPU and WARP do; the arithmetic in f32, [`primase_image::sampling`]'s
/// mechanism), the addressing along each axis the binding states (wrap where it states none). Albedo RGB is linearized with the shared
/// sRGB transfer before filtering; alpha and data textures remain linear. Runtime derivatives are not inferred from the
/// shader resource names.
///
/// It keeps the stored texels it was made from ([`BaseMipBilinearWrap::rgba`]), so a bake's pipeline state
/// ([`super::PipelineState`]) carries the very pixels it samples.
#[derive(Clone, Debug)]
pub struct BaseMipBilinearWrap {
    decoded: Decoded,
    address: [Address; 2],
    rgba: Arc<[u8]>,
    srgb: bool,
}

impl BaseMipBilinearWrap {
    /// Wrapping along both axes.
    pub fn new(width: u32, height: u32, rgba: &[u8], albedo_srgb: bool) -> Result<Self> {
        Self::addressed(width, height, rgba, albedo_srgb, [Address::Wrap; 2])
    }
    /// With the addressing `address` along u and v.
    pub fn addressed(
        width: u32,
        height: u32,
        rgba: &[u8],
        albedo_srgb: bool,
        address: [Address; 2],
    ) -> Result<Self> {
        let levels = Levels::one(width, height, rgba.to_vec())
            .map_err(|e| invalid("layer sampler", 0, e.to_string()))?;
        let transfer = if albedo_srgb {
            Transfer::SrgbColour
        } else {
            Transfer::Stored
        };
        Ok(Self {
            decoded: levels.decoded(0, transfer),
            address,
            rgba: rgba.into(),
            srgb: albedo_srgb,
        })
    }
    pub fn dimensions(&self) -> [usize; 2] {
        self.decoded.dimensions()
    }
    /// The bytes it holds: the decoded texels and the stored ones.
    pub fn decoded_bytes(&self) -> usize {
        self.decoded.decoded_bytes() + self.rgba.len()
    }
    /// The stored texels, RGBA8 in rows, as given.
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
    /// Whether the colour lanes are sRGB-encoded (linearized before filtering); alpha is linear either way.
    pub fn srgb(&self) -> bool {
        self.srgb
    }
    /// The addressing along u and v.
    pub fn address(&self) -> [Address; 2] {
        self.address
    }
    pub fn sample(&self, uv: [f32; 2]) -> Result<[f32; 4]> {
        self.decoded
            .bilinear_at(uv, self.address, Subtexel::D3d11)
            .map_err(|e| invalid("layer sampler", 0, e.to_string()))
    }
}

/// The sampling address a binding's addressing is. A binding that states none wraps (`None`: the caller counts it); a
/// border needs the sampler's border colour, which the source does not store.
pub(crate) fn address(mode: Option<primase_core::shader::AddressMode>) -> Result<Address> {
    use primase_core::shader::AddressMode;
    Ok(match mode {
        None | Some(AddressMode::Repeat) => Address::Wrap,
        Some(AddressMode::Mirror) => Address::Mirror,
        Some(AddressMode::Clamp) => Address::Clamp,
        Some(AddressMode::MirrorOnce) => Address::MirrorOnce,
        Some(AddressMode::Border) => {
            return Err(invalid(
                "layer sampler",
                0,
                "border addressing: the source stores no border colour",
            ));
        }
    })
}

/// The policy's sampling whatever level of detail the program states: the base mip.
impl primase_shader::Image for BaseMipBilinearWrap {
    fn sample(&self, uv: [f32; 2], _lod: primase_shader::Lod) -> std::result::Result<[f32; 4], String> {
        BaseMipBilinearWrap::sample(self, uv).map_err(|e| e.to_string())
    }
}
