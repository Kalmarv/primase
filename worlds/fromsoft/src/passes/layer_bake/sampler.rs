use primase_image::sampling::{Address, Decoded, Levels, Transfer};

use crate::{Result, invalid};

/// Named offline policy: base mip, bilinear filtering, in f32 ([`primase_image::sampling`]'s mechanism), the
/// addressing along each axis the binding states (wrap where it states none). Albedo RGB is linearized with the shared
/// sRGB transfer before filtering; alpha and data textures remain linear. Runtime derivatives are not inferred from the
/// shader resource names.
#[derive(Clone, Debug)]
pub struct BaseMipBilinearWrap(Decoded, [Address; 2]);

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
        Ok(Self(levels.decoded(0, transfer), address))
    }
    pub fn dimensions(&self) -> [usize; 2] {
        self.0.dimensions()
    }
    pub fn decoded_bytes(&self) -> usize {
        self.0.decoded_bytes()
    }
    pub fn sample(&self, uv: [f32; 2]) -> Result<[f32; 4]> {
        self.0
            .bilinear(uv, self.1)
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
