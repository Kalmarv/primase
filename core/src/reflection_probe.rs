//! Spatial reflection probes and explicitly selected pairs of environment states.
use crate::{
    ColorSpace, Component, Entity, Geometry, IrradianceVolume, JsonError, Linear, TextureLayers,
    TextureShape, Transform, World,
};
use serde::Serialize;
use serde_json::Value;

/// How surface roughness selects the mip of a stored reflection cube. LODs refer to the
/// cube's stored mip chain; synthesizing a different chain is a separate render-graph operation.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeMipPolicy {
    /// Sample this LOD independently of roughness.
    Fixed(f32),
    /// Map perceptual roughness in [0,1] linearly onto this inclusive LOD interval.
    LinearRoughness { minimum: f32, maximum: f32 },
}

/// One authored environment state. Its diffuse field is the existing irradiance-volume
/// entity, including that entity's transform and coefficient sampling domain.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProbeState {
    pub cube: Entity,
    pub diffuse_volume: Option<Entity>,
    /// Multiplier on the cube's linear radiance, finite and nonnegative.
    pub intensity: f32,
    pub mip_policy: ProbeMipPolicy,
}

/// The caller or source has selected both states and the interpolation factor explicitly.
/// State identifiers and clock/quality selection remain source concerns. A consumer blends
/// sampled linear radiance and decoded irradiance as A + (B - A) * factor. It must preserve
/// both original states when a source program performs this blend itself.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EnvironmentProbeBlend {
    pub state_a: ProbeState,
    pub state_b: ProbeState,
    /// Finite, inclusive [0,1]. The endpoints still retain both states and their provenance.
    pub factor: f32,
}

/// An environment probe's spatial influence. The entity's transform maps the unit box
/// [-1,1] to world metres. Regions blend in increasing priority; equal-priority order
/// belongs to the source graph. The fade shell is measured in authored box metres.
///
/// The linked diffuse volumes are states of this region, rather than additional independent
/// contributions. A consumer drawing these regions must exclude their linked volumes from
/// any independent irradiance-volume accumulation.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReflectionProbeRegion {
    pub blend: EnvironmentProbeBlend,
    pub half_extents: [f32; 3],
    pub fade_distance: f32,
    pub priority: u8,
    /// Reflection capture origin in world metres.
    pub reflection_origin: [f32; 3],
    /// Optional parallax-correction box relative to the influence region. Absence means
    /// the sampled reflection direction is used directly.
    pub parallax_from_region: Option<Transform>,
}

impl Component for ReflectionProbeRegion {
    const NAME: &'static str = "core.reflection_probe_region";
    const GEOMETRY: Geometry = Geometry::None;

    fn references(&self, out: &mut Vec<Entity>) {
        for state in [&self.blend.state_a, &self.blend.state_b] {
            out.push(state.cube);
            out.extend(state.diffuse_volume);
        }
    }

    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }

    fn verify(&self, entity: Entity, world: &World) -> Result<(), String> {
        let transform = world
            .get::<Transform>(entity)
            .ok_or("reflection probe region has no transform")?;
        for transform in std::iter::once(transform).chain(self.parallax_from_region.as_ref()) {
            if !transform.cols.iter().flatten().all(|v| v.is_finite()) || Linear::of(transform).singular() {
                return Err("reflection probe region has a nonfinite or singular transform".into());
            }
        }
        if !self.half_extents.iter().all(|v| v.is_finite() && *v > 0.)
            || !self.fade_distance.is_finite()
            || self.fade_distance < 0.
            || !self.reflection_origin.iter().all(|v| v.is_finite())
        {
            return Err("reflection probe region has invalid bounds, fade or origin".into());
        }
        if !self.blend.factor.is_finite() || !(0. ..=1.).contains(&self.blend.factor) {
            return Err("environment probe blend factor must be finite and in [0,1]".into());
        }
        for state in [&self.blend.state_a, &self.blend.state_b] {
            if !state.intensity.is_finite() || state.intensity < 0. {
                return Err("reflection probe intensity must be finite and nonnegative".into());
            }
            let cube = world
                .get::<TextureLayers>(state.cube)
                .ok_or("reflection probe cube is not a layered texture")?;
            if cube.shape != (TextureShape::Cube { cubes: 1 })
                || cube.width != cube.height
                || cube.color_space != ColorSpace::Linear
                || cube.levels.is_empty()
            {
                return Err("reflection probe requires one square linear cube with stored mips".into());
            }
            let (minimum, maximum) = match state.mip_policy {
                ProbeMipPolicy::Fixed(lod) => (lod, lod),
                ProbeMipPolicy::LinearRoughness { minimum, maximum } => (minimum, maximum),
            };
            if !minimum.is_finite()
                || !maximum.is_finite()
                || minimum < 0.
                || maximum < minimum
                || maximum > (cube.levels.len() - 1) as f32
            {
                return Err("reflection probe LOD policy exceeds its stored mip chain".into());
            }
            if let Some(diffuse) = state.diffuse_volume
                && world.get::<IrradianceVolume>(diffuse).is_none()
            {
                return Err("reflection probe diffuse state is not an irradiance volume".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(world: &mut World) -> (Entity, ReflectionProbeRegion) {
        let cube = world.spawn();
        world.insert(
            cube,
            TextureLayers {
                shape: TextureShape::Cube { cubes: 1 },
                width: 1,
                height: 1,
                format: crate::TexFormat::Rgba16Float,
                levels: vec![vec![vec![0; 8]; 6]],
                color_space: ColorSpace::Linear,
                role: crate::TexRole::Other,
                normal: None,
            },
        );
        let state = ProbeState {
            cube,
            diffuse_volume: None,
            intensity: 1.,
            mip_policy: ProbeMipPolicy::LinearRoughness {
                minimum: 0.,
                maximum: 0.,
            },
        };
        let entity = world.spawn();
        world.insert(entity, Transform::IDENTITY);
        (
            entity,
            ReflectionProbeRegion {
                blend: EnvironmentProbeBlend {
                    state_a: state.clone(),
                    state_b: state,
                    factor: 0.5,
                },
                half_extents: [1.; 3],
                fade_distance: 0.,
                priority: 0,
                reflection_origin: [0.; 3],
                parallax_from_region: None,
            },
        )
    }

    #[test]
    fn probe_states_retain_both_cube_and_diffuse_dependencies() {
        let mut world = World::new();
        let (entity, mut probe) = region(&mut world);
        let diffuse = world.spawn();
        // The link is typed: a name-only entity cannot serve as an irradiance state.
        probe.blend.state_b.diffuse_volume = Some(diffuse);
        assert!(probe.verify(entity, &world).is_err());
        let mut references = Vec::new();
        probe.references(&mut references);
        assert_eq!(
            references,
            vec![probe.blend.state_a.cube, probe.blend.state_b.cube, diffuse]
        );
        probe.blend.state_b.diffuse_volume = None;
        world.insert(entity, probe);
        assert!(world.verify_components().is_empty());
        assert!(world.closure([entity]).contains(&references[0]));
    }

    #[test]
    fn probes_refuse_missing_mips_invalid_blends_and_array_stand_ins() {
        let mut world = World::new();
        let (entity, mut probe) = region(&mut world);
        probe.blend.state_a.mip_policy = ProbeMipPolicy::Fixed(1.);
        assert!(probe.verify(entity, &world).is_err());
        probe.blend.state_a.mip_policy = ProbeMipPolicy::Fixed(0.);
        probe.blend.factor = f32::NAN;
        assert!(probe.verify(entity, &world).is_err());
        probe.blend.factor = 1.;
        assert!(probe.verify(entity, &world).is_ok());
        world
            .get_mut::<TextureLayers>(probe.blend.state_b.cube)
            .expect("cube")
            .shape = TextureShape::Array { layers: 6 };
        assert!(probe.verify(entity, &world).is_err());
    }
}
