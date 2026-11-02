//! How light passes through the water under a surface (`core.water_body`, on material entities).

use serde::Serialize;
use serde_json::Value;

use crate::pbr::TextureRef;
use crate::{Component, Entity, Geometry, JsonError, World};

/// `core.water_body`: the optics of the water a material's surface covers, in the world's units (metres, seconds,
/// linear RGB), on a material entity whose [`crate::MaterialUsage`] is water. What the surface itself reflects stays
/// [`crate::PbrMaterial`]'s; this is what lies under it and how the surface moves.
///
/// The model it states, which a consumer draws as far as it can: a view ray enters the water at the surface and travels
/// `d` metres through it to what is behind (the scene below, read from the frame or a closed volume). What is behind
/// arrives multiplied by `exp(-extinction * d)` per channel, and the water body adds `scatter * E / pi * (1 -
/// exp(-extinction * d))`, `E` the irradiance (sun and sky, the light a horizontal diffuse surface at the water's
/// surface receives). The sum is transmitted through the surface, weighted by `1 - F` against the reflection's `F`,
/// Schlick's `F = F0 + (1 - F0) * (1 - cos)^5` with `F0 = ((ior - 1) / (ior + 1))^2`. The surface's normal is
/// [`WaterBody::waves`] summed; its opacity at the shore is `saturate(depth / shore_fade)`, `depth` the water's depth
/// below the surface along the view axis.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WaterBody {
    /// Light lost per metre the view ray travels through the water, per channel: a path of `d` metres transmits
    /// `exp(-extinction * d)`. Zero is clear water.
    pub extinction: [f32; 3],
    /// The diffuse reflectance of the water body where it is infinitely deep, per channel within 0-1: it returns
    /// radiance `scatter * E / pi` from below the surface for an irradiance `E` arriving at the surface. A consumer that
    /// has only a body colour takes that colour, linear, as `scatter`; zero is water that returns nothing of its own,
    /// only what is behind it.
    pub scatter: [f32; 3],
    /// The surface's index of refraction, above 1: its Fresnel reflectance at normal incidence is
    /// `((ior - 1) / (ior + 1))^2` (0.02 for 1.33, 0.04 for 1.5), and for a source that states that reflectance `F0`
    /// instead, `ior = (1 + sqrt(F0)) / (1 - sqrt(F0))`.
    pub ior: f32,
    /// The moving normal maps whose tangent-space normals, summed, make the surface's normal; empty for a still flat
    /// surface.
    pub waves: Vec<WaveLayer>,
    /// The depth of water (metres, below the surface along the view axis) over which the surface fades in from fully
    /// transparent at the shore to opaque; zero is a hard edge.
    pub shore_fade: f32,
}

/// One moving normal map of a [`WaterBody`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct WaveLayer {
    /// The normal map (tangent space, as [`crate::PbrMaterial::normal_texture`]), its tiling in the reference's
    /// transform.
    pub normal: TextureRef,
    /// How far its coordinates move per second, in texture units after the transform: the map is sampled at
    /// `transformed(uv) + velocity * t`.
    pub velocity: [f32; 2],
    /// The factor on its tangent-space x and y before the layers are summed (a normal map's `scale`).
    pub strength: f32,
}

impl Component for WaterBody {
    const NAME: &'static str = "core.water_body";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// Every range its fields document.
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        if !self.extinction.iter().all(|x| x.is_finite() && *x >= 0.) {
            return Err(format!("extinction {:?} is not finite and >= 0", self.extinction));
        }
        if !self.scatter.iter().all(|x| (0.0..=1.0).contains(x)) {
            return Err(format!("scatter {:?} is not within 0-1", self.scatter));
        }
        if !(self.ior.is_finite() && self.ior > 1.) {
            return Err(format!("ior {} is not finite and above 1", self.ior));
        }
        if !(self.shore_fade.is_finite() && self.shore_fade >= 0.) {
            return Err(format!("shore fade {} is not finite and >= 0", self.shore_fade));
        }
        for (i, w) in self.waves.iter().enumerate() {
            if !w.velocity.iter().chain([&w.strength]).all(|x| x.is_finite()) {
                return Err(format!(
                    "wave {i}: velocity {:?} or strength {} is not finite",
                    w.velocity, w.strength
                ));
            }
            if let Some(t) = w.normal.transform
                && !t
                    .offset
                    .iter()
                    .chain(&t.scale)
                    .chain([&t.rotation])
                    .all(|x| x.is_finite())
            {
                return Err(format!("wave {i}: texture transform {t:?} is not finite"));
            }
        }
        Ok(())
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.waves.iter().map(|w| w.normal.texture));
    }
}

impl WaterBody {
    /// The Fresnel reflectance at normal incidence of [`WaterBody::ior`].
    pub fn f0(&self) -> f32 {
        ((self.ior - 1.) / (self.ior + 1.)).powi(2)
    }

    /// The index of refraction whose reflectance at normal incidence is `f0` (within 0-1).
    pub fn ior_of_f0(f0: f32) -> f32 {
        let r = f0.sqrt();
        (1. + r) / (1. - r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pbr::{UvTransform, Wrap};
    use iridia_interface::Filters;

    fn lake(w: &mut World) -> WaterBody {
        let normal = w.spawn();
        WaterBody {
            extinction: [0.215, 0.115, 0.09],
            scatter: [0.; 3],
            ior: 1.5,
            waves: vec![WaveLayer {
                normal: TextureRef {
                    texture: normal,
                    texcoord: 0,
                    wrap: [Wrap::Repeat; 2],
                    transform: Some(UvTransform {
                        offset: [0.; 2],
                        scale: [4.; 2],
                        rotation: 0.,
                    }),
                    filters: Filters::default(),
                },
                velocity: [-0.0003, 0.0015],
                strength: 1.,
            }],
            shore_fade: 1. / 3.,
        }
    }

    #[test]
    fn a_water_body_shows_its_fields_and_refers_to_its_wave_maps() {
        let mut w = World::new();
        let body = lake(&mut w);
        let json = body.to_json().unwrap().unwrap();
        assert_eq!(
            json["extinction"],
            serde_json::json!([0.215f32, 0.115f32, 0.09f32])
        );
        assert_eq!(json["ior"], serde_json::json!(1.5));
        assert_eq!(
            json["waves"][0]["velocity"],
            serde_json::json!([-0.0003f32, 0.0015f32])
        );
        let mut refs = Vec::new();
        body.references(&mut refs);
        assert_eq!(refs, vec![body.waves[0].normal.texture]);
        let e = w.spawn();
        assert_eq!(body.verify(e, &w), Ok(()));
    }

    #[test]
    fn ior_and_normal_reflectance_convert_both_ways() {
        let mut w = World::new();
        let body = lake(&mut w);
        assert!((body.f0() - 0.04).abs() < 1e-6);
        assert!((WaterBody::ior_of_f0(0.04) - 1.5).abs() < 1e-5);
        assert!((WaterBody::ior_of_f0(0.02) - 1.3294).abs() < 1e-4);
    }

    #[test]
    fn a_value_outside_its_range_is_refused() {
        let mut w = World::new();
        let e = w.spawn();
        let base = lake(&mut w);
        let refused = |b: WaterBody| b.verify(e, &w).is_err();
        assert!(refused(WaterBody {
            extinction: [-0.1, 0., 0.],
            ..base.clone()
        }));
        assert!(refused(WaterBody {
            scatter: [1.5, 0., 0.],
            ..base.clone()
        }));
        assert!(refused(WaterBody {
            ior: 1.,
            ..base.clone()
        }));
        assert!(refused(WaterBody {
            shore_fade: f32::NAN,
            ..base.clone()
        }));
        let mut fast = base.clone();
        fast.waves[0].velocity = [f32::INFINITY, 0.];
        assert!(refused(fast));
    }
}
