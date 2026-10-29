//! Source light conversion with explicit units and colour/cone choices; never derive lamps from name matches.
use crate::components::AnvilSpotLight;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Light, LightKind, World};

#[derive(Clone, Debug)]
pub struct LowerAnvilLights {
    pub candela_per_unit: f32,
    pub srgb: bool,
    pub full_cone: bool,
}
pub const LOWER_ANVIL_LIGHTS: PassSpec = PassSpec {
    name: "lower-anvil-lights",
    options: &[
        OptionSpec::required(
            "candela-per-unit",
            "1",
            "positive calibration of the stored intensity to candela; the engine's unit is not established",
        ),
        OptionSpec::required(
            "colour",
            "linear",
            "linear or srgb: interpretation of the stored RGB",
        ),
        OptionSpec::required(
            "cone",
            "half",
            "half or full: whether stored cone angles are half angles",
        ),
    ],
    make: |g: &Given| {
        let scale: f32 = g
            .required("candela-per-unit")
            .parse()
            .map_err(|_| "candela-per-unit must be a number")?;
        if !scale.is_finite() || scale <= 0.0 {
            return Err("candela-per-unit must be finite and positive".into());
        }
        let srgb = match g.required("colour") {
            "linear" => false,
            "srgb" => true,
            _ => return Err("colour must be linear or srgb".into()),
        };
        let full_cone = match g.required("cone") {
            "half" => false,
            "full" => true,
            _ => return Err("cone must be half or full".into()),
        };
        Ok(Box::new(LowerAnvilLights {
            candela_per_unit: scale,
            srgb,
            full_cone,
        }))
    },
};
impl Pass for LowerAnvilLights {
    fn name(&self) -> &'static str {
        "lower-anvil-lights"
    }
    fn options(&self) -> Options {
        Options::from([
            ("candela-per-unit".into(), self.candela_per_unit.to_string()),
            ("colour".into(), if self.srgb { "srgb" } else { "linear" }.into()),
            ("cone".into(), if self.full_cone { "full" } else { "half" }.into()),
        ])
    }
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilSpotLight::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Light::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilSpotLight::NAME]
    }
    fn approximates(&self) -> &'static str {
        "stored light intensity calibrated by caller; colour and cone interpretation stated; shadow flags unknown"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let ids: Vec<_> = world.iter::<AnvilSpotLight>().map(|(e, _)| e).collect();
        for entity in ids {
            let source = world
                .get::<AnvilSpotLight>(entity)
                .ok_or("listed light missing")?;
            let angle = if self.full_cone { 0.5 } else { 1.0 };
            let inner = source.stored.inner * angle;
            let outer = source.stored.outer * angle;
            if outer > std::f32::consts::FRAC_PI_2 {
                return Err(format!("{entity:?}: chosen cone interpretation exceeds pi/2"));
            }
            let decode = |v: f32| {
                if !self.srgb {
                    v
                } else if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            let intensity = source.stored.intensity * self.candela_per_unit;
            if !intensity.is_finite() {
                return Err(format!("{entity:?}: calibrated intensity overflow"));
            }
            let light = Light {
                kind: LightKind::Spot {
                    inner_cone: inner,
                    outer_cone: outer,
                },
                color: [
                    source.stored.color[0],
                    source.stored.color[1],
                    source.stored.color[2],
                ]
                .map(decode),
                intensity,
                range: Some(source.stored.range),
                cast_shadows: None,
                radius: None,
                specular: None,
            };
            world.insert(entity, light);
            world.remove::<AnvilSpotLight>(entity);
            log.count("authored spot lights lowered", 1);
            log.approximate(
                "intensity/colour/cone are the caller's stated calibration; shadow flags unread",
                1,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anvil_formats::light::StoredSpotLight;
    use primase_core::Transform;

    #[test]
    fn light_lowering_uses_explicit_units_and_cone_interpretation_without_moving_the_owner() {
        let mut world = World::new();
        let entity = world.spawn();
        let mut transform = Transform::IDENTITY;
        transform.cols[3] = [2.0, 3.0, 4.0];
        world.insert(entity, transform);
        world.insert(
            entity,
            AnvilSpotLight {
                index_id: 4,
                stored: StoredSpotLight {
                    color: [0.5, 1.0, 0.0, 1.0],
                    intensity: 20.0,
                    luminance: 30.0,
                    inner: 0.4,
                    outer: 0.8,
                    range: 7.0,
                    near: 0.1,
                },
            },
        );
        LowerAnvilLights {
            candela_per_unit: 2.0,
            srgb: true,
            full_cone: true,
        }
        .run(&mut world, &mut PassLog::default())
        .unwrap();
        let light = world.get::<Light>(entity).unwrap();
        assert_eq!(light.intensity, 40.0);
        assert_eq!(
            light.kind,
            LightKind::Spot {
                inner_cone: 0.2,
                outer_cone: 0.4
            }
        );
        assert_eq!(light.range, Some(7.0));
        assert!((light.color[0] - 0.21404114).abs() < 1e-6);
        assert_eq!(world.get::<Transform>(entity), Some(&transform));
        assert!(!world.has::<AnvilSpotLight>(entity));
    }
}
