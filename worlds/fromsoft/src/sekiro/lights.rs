//! Explicit scene-light policy. BTL's transfer function, intensity calibration
//! and spot axis have not been established against the source lighting shader.
use crate::{
    Result,
    btl::{Kind, Light},
    invalid,
    msb::euler_transform,
};
use primase_core::Transform;
use primase_meshops::vector::Vec3;
use primase_skate_scene::SkateLight;
/// Exact sRGB transfer is a named interpretation of BTL RGB bytes. Watts per
/// source power unit and local +Z spotlight direction remain uncalibrated.
#[derive(Clone, Copy, Debug)]
pub struct SrgbLightPolicy {
    pub watts_per_power: f32,
}
impl Default for SrgbLightPolicy {
    fn default() -> Self {
        Self {
            watts_per_power: 100.,
        }
    }
}
impl SrgbLightPolicy {
    pub fn convert(&self, light: &Light, map: &Transform) -> Result<SkateLight> {
        if !self.watts_per_power.is_finite() || self.watts_per_power < 0. {
            return Err(invalid("BTL policy", 0, "invalid energy scale"));
        }
        if light.kind == Kind::Directional {
            return Err(invalid(
                "BTL policy",
                0,
                "directional light is not a point or spot",
            ));
        }
        let local = euler_transform(light.position, light.rotation, [1.; 3], 0)?;
        let direction = |v: [f32; 3]| -> [f32; 3] {
            let p = std::array::from_fn(|i| {
                map.cols[0][i] * v[0] + map.cols[1][i] * v[1] + map.cols[2][i] * v[2]
            });
            Vec3::from(p).normalize().into()
        };
        let spot = light.kind == Kind::Spot;
        // Frame.transform conjugates the basis. Local source +Z is scene -Z.
        let dir = if spot {
            direction(local.cols[2].map(|v| -v))
        } else {
            [0., -1., 0.]
        };
        let tangent = if spot {
            direction(local.cols[0])
        } else {
            [1., 0., 0.]
        };
        let energy = light.diffuse_power * self.watts_per_power;
        if !energy.is_finite() || energy < 0. {
            return Err(invalid("BTL policy", 0, "invalid light energy"));
        }
        Ok(SkateLight {
            spot,
            position: super::point(map, local.cols[3]),
            direction: dir,
            tangent,
            color: light
                .diffuse
                .map(|c| primase_image::srgb_to_linear(f64::from(c) / 255.) as f32),
            energy,
            range: light.radius.max(0.5),
            radius: 0.1,
            spot_size: if spot {
                (2. * light.cone_angle).clamp(1., 179.).to_radians()
            } else {
                0.
            },
            spot_blend: if spot { 0.15 } else { 0. },
            // Bank-zero lights stay enabled in every target time slot.
            time_of_day: 127,
            cast_shadow: light.cast_shadows,
            priority: None,
            group: String::new(),
            unknown_flags: 0,
        })
    }
}
