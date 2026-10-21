use primase_image::sampling::{Address, Decoded, Levels, Transfer};

use crate::{Result, invalid};

/// Named offline policy: base mip, bilinear filtering, wrap on both axes, in f32 ([`primase_image::sampling`]'s
/// mechanism). Albedo RGB is linearized with the shared sRGB transfer before filtering; alpha and data textures
/// remain linear. Runtime derivatives and sampler descriptors are not inferred from the shader resource names.
#[derive(Clone, Debug)]
pub struct BaseMipBilinearWrap(Decoded);

impl BaseMipBilinearWrap {
    pub fn new(width: u32, height: u32, rgba: &[u8], albedo_srgb: bool) -> Result<Self> {
        let levels = Levels::one(width, height, rgba.to_vec())
            .map_err(|e| invalid("layer sampler", 0, e.to_string()))?;
        let transfer = if albedo_srgb {
            Transfer::SrgbColour
        } else {
            Transfer::Stored
        };
        Ok(Self(levels.decoded(0, transfer)))
    }
    pub fn dimensions(&self) -> [usize; 2] {
        self.0.dimensions()
    }
    pub fn decoded_bytes(&self) -> usize {
        self.0.decoded_bytes()
    }
    pub fn sample(&self, uv: [f32; 2]) -> Result<[f32; 4]> {
        self.0
            .bilinear(uv, [Address::Wrap; 2])
            .map_err(|e| invalid("layer sampler", 0, e.to_string()))
    }
}

/// The policy's sampling whatever level of detail the program states: the base mip.
impl primase_shader::Image for BaseMipBilinearWrap {
    fn sample(&self, uv: [f32; 2], _lod: primase_shader::Lod) -> std::result::Result<[f32; 4], String> {
        BaseMipBilinearWrap::sample(self, uv).map_err(|e| e.to_string())
    }
}
