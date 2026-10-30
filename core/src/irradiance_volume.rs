//! Spatial first-order spherical-harmonic irradiance, on box-region entities.
use crate::{
    ColorSpace, Component, Entity, Geometry, JsonError, TextureLayers, TextureShape, Transform, World,
};
use serde::Serialize;
use serde_json::Value;

/// Four 3D coefficient textures, in X/Y/Z/constant order. For each sampled RGBA value,
/// decode RGB as `(rgb - 0.5) * 2 * exp2((a - 0.5) * 20)`, then dot the four
/// terms with `(dot(n,basis[0]), dot(n,basis[1]), dot(n,basis[2]), 1)` and take max(0).
/// The entity's transform maps the unit box [-1,1] to its blend region in world metres.
/// Regions blend in increasing priority; their interior excludes lower-priority regions,
/// while their fade shell blends over them. Equal-priority ordering is source-defined.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct IrradianceVolume {
    pub coefficients: [Entity; 4],
    /// Unit directions in world space for the three directional coefficient terms.
    pub basis: [[f32; 3]; 3],
    /// Sampling box relative to the blend box. Identity when both domains coincide.
    pub sampling_from_region: Transform,
    /// Offset the sample towards the surface normal by one normalized sampling-space texel.
    pub normal_offset: bool,
    /// Authored metric half-extents, retained separately from the rounded matrix basis.
    pub half_extents: [f32; 3],
    pub fade_distance: f32,
    pub diffuse_scale: f32,
    pub priority: u8,
}
impl Component for IrradianceVolume {
    const NAME: &'static str = "core.irradiance_volume";
    const GEOMETRY: Geometry = Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.coefficients);
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        let transform = w
            .get::<Transform>(e)
            .ok_or("irradiance volume has no transform")?;
        if crate::Linear::of(transform).singular() || crate::Linear::of(&self.sampling_from_region).singular()
        {
            return Err("irradiance volume has a singular transform".into());
        }
        if !self.basis.iter().flatten().all(|x| x.is_finite()) {
            return Err("irradiance basis is not finite".into());
        }
        if crate::Linear::new(self.basis).singular() {
            return Err("irradiance coefficient basis is singular".into());
        }
        if !self
            .sampling_from_region
            .cols
            .iter()
            .flatten()
            .all(|x| x.is_finite())
        {
            return Err("irradiance sampling transform is not finite".into());
        }
        if !self.half_extents.iter().all(|v| v.is_finite() && *v > 0.) {
            return Err("irradiance half-extents must be finite and positive".into());
        }
        if !self.fade_distance.is_finite()
            || self.fade_distance < 0.
            || !self.diffuse_scale.is_finite()
            || self.diffuse_scale < 0.
        {
            return Err("irradiance fade or diffuse scale is invalid".into());
        }
        let mut dimensions = None;
        for texture in self.coefficients {
            let t = w
                .get::<TextureLayers>(texture)
                .ok_or("irradiance coefficient is not a layered texture")?;
            let TextureShape::Volume { depth } = t.shape else {
                return Err("irradiance coefficient is not 3D".into());
            };
            if t.color_space != ColorSpace::Linear {
                return Err("irradiance coefficient is not linear".into());
            }
            if !matches!(
                t.format,
                crate::TexFormat::Bc7
                    | crate::TexFormat::Rgba8Unorm
                    | crate::TexFormat::Rgba16Float
                    | crate::TexFormat::Rgba32Float
                    | crate::TexFormat::Rgba16Unorm
            ) {
                return Err("irradiance coefficient must store four RGBA channels".into());
            }
            let size = (t.width, t.height, depth, t.format);
            if dimensions.is_some_and(|other| other != size) {
                return Err("irradiance coefficient layouts differ".into());
            }
            dimensions = Some(size);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coefficient_references_require_matching_linear_volumes() {
        let mut w = World::new();
        let coefficients = std::array::from_fn(|_| {
            let e = w.spawn();
            w.insert(
                e,
                TextureLayers {
                    shape: TextureShape::Volume { depth: 2 },
                    width: 1,
                    height: 1,
                    format: crate::TexFormat::Rgba8Unorm,
                    levels: vec![vec![vec![0; 4]; 2]],
                    color_space: ColorSpace::Linear,
                    role: crate::TexRole::Other,
                    normal: None,
                },
            );
            e
        });
        let e = w.spawn();
        w.insert(e, Transform::IDENTITY);
        let volume = IrradianceVolume {
            coefficients,
            basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            sampling_from_region: Transform::IDENTITY,
            normal_offset: true,
            half_extents: [1.; 3],
            fade_distance: 3.,
            diffuse_scale: 1.,
            priority: 20,
        };
        assert_eq!(volume.verify(e, &w), Ok(()));
        w.insert(e, volume.clone());
        assert!(coefficients.into_iter().all(|t| w.closure([e]).contains(&t)));
        w.get_mut::<TextureLayers>(coefficients[2])
            .expect("texture")
            .color_space = ColorSpace::Srgb;
        assert_eq!(
            volume.verify(e, &w),
            Err("irradiance coefficient is not linear".into())
        );
        w.get_mut::<TextureLayers>(coefficients[2])
            .expect("texture")
            .color_space = ColorSpace::Linear;
        w.get_mut::<TextureLayers>(coefficients[2])
            .expect("texture")
            .shape = TextureShape::Array { layers: 2 };
        assert_eq!(
            volume.verify(e, &w),
            Err("irradiance coefficient is not 3D".into())
        );
    }
}
