use crate::{Result, invalid};

/// Named offline policy: base mip, bilinear filtering, wrap on both axes.
/// Albedo RGB is linearized with the shared sRGB transfer before filtering;
/// alpha and data textures remain linear. Runtime derivatives and sampler
/// descriptors are not inferred from the shader resource names.
#[derive(Clone, Debug)]
pub struct BaseMipBilinearWrap {
    width: usize,
    height: usize,
    texels: Vec<[f32; 4]>,
}
impl BaseMipBilinearWrap {
    pub fn new(width: u32, height: u32, rgba: &[u8], albedo_srgb: bool) -> Result<Self> {
        let count = (width as usize)
            .checked_mul(height as usize)
            .filter(|&n| n > 0)
            .ok_or_else(|| invalid("layer sampler", 0, "invalid dimensions"))?;
        if count.checked_mul(4) != Some(rgba.len()) {
            return Err(invalid("layer sampler", 0, "RGBA length mismatch"));
        }
        let texels = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                std::array::from_fn(|c| {
                    if albedo_srgb && c != 3 {
                        primase_image::srgb_to_linear(p[c] as f64 / 255.0) as f32
                    } else {
                        p[c] as f32 / 255.0
                    }
                })
            })
            .collect();
        Ok(Self {
            width: width as usize,
            height: height as usize,
            texels,
        })
    }
    pub fn dimensions(&self) -> [usize; 2] {
        [self.width, self.height]
    }
    pub fn decoded_bytes(&self) -> usize {
        self.texels.len() * 16
    }
    pub fn sample(&self, uv: [f32; 2]) -> Result<[f32; 4]> {
        let x = uv[0] * self.width as f32 - 0.5;
        let y = uv[1] * self.height as f32 - 0.5;
        // Reject coordinates outside the stated CPU policy rather than saturating
        // a float-to-integer conversion and sampling an unrelated texel.
        if !x.is_finite() || !y.is_finite() || x.abs() >= 2147483520.0 || y.abs() >= 2147483520.0 {
            return Err(invalid(
                "layer sampler",
                0,
                "nonfinite or excessive texel coordinate",
            ));
        }
        let (ix, iy) = (x.floor() as i64, y.floor() as i64);
        let (tx, ty) = (x - ix as f32, y - iy as f32);
        let mut result = [0.; 4];
        for dy in 0..2 {
            for dx in 0..2 {
                let xx = (ix + dx).rem_euclid(self.width as i64) as usize;
                let yy = (iy + dy).rem_euclid(self.height as i64) as usize;
                let weight = (if dx == 1 { tx } else { 1. - tx }) * (if dy == 1 { ty } else { 1. - ty });
                for (c, out) in result.iter_mut().enumerate() {
                    *out += self.texels[yy * self.width + xx][c] * weight;
                }
            }
        }
        Ok(result)
    }
}
