//! Bind already prepared core environment regions in the registered graph preparation pass.
use iridia_interface::{MatrixLines, Stage};
use primase_core::render_graph::{GraphPrimitive, PassWork, Presence, RenderGraph, RenderPass, Runs};
use primase_core::shader::{
    Attribute, ConstantRole, InputRole, ObjectParameter, Shader, StageBinding, TextureSource,
};
use primase_core::{
    Entity, IrradianceVolume, Name, ProbeMipPolicy, ReflectionProbeRegion, TextureLayers, TextureShape, World,
};

pub(super) const DIRECT: &str = "environment regions all use the direct two-cube blend; the game preblends some regions' cubes by a schedule not traced (9 of 56 in the Stormveil capture)";
pub(super) const LUMINANCE: &str = "ambient reflection luminance correction off: g_ivShLuminance's producer not traced; diffuse ambient unaffected; in measured Stormveil regions reflections are 0-39% dimmer than the game's (disabled/native 0.61-1.0)";
pub(super) const MASKS: &str = "the six native environment stencil region-mask draws are retained in the source graph but not executed; region draws use the assumed geometry stencil bit only, so native region exclusion is not reproduced";

pub(super) fn select(graph: &mut RenderGraph, world: &World) -> Result<Option<u32>, String> {
    let mut regions: Vec<_> = world.iter::<ReflectionProbeRegion>().collect();
    if regions.is_empty() {
        return Ok(None);
    }
    regions.sort_by_key(|(e, r)| (r.priority, *e));
    let index = graph
        .passes
        .iter()
        .position(|p| p.name == "GXEnvMapRegion")
        .ok_or("environment graph has no region group")?;
    let PassWork::Group { steps } = &graph.passes[index].work else {
        return Err("environment stage is not a region group".into());
    };
    let template = steps
        .iter()
        .find(|p| p.name == "GXEnvMapRegion")
        .ok_or("environment group has no draw")?;
    let PassWork::Screen {
        stages,
        state,
        outputs,
        ..
    } = &template.work
    else {
        return Err("environment template has no raster state".into());
    };
    let vertex = stages
        .iter()
        .find(|s| s.stage == Stage::Vertex)
        .ok_or("environment vertex stage absent")?;
    let pixel = stages
        .iter()
        .find(|s| {
            s.stage == Stage::Pixel
                && world
                    .get::<Name>(s.shader)
                    .is_some_and(|n| n.0.ends_with("GXEnvMapRegion[Box][DiffSpc][Lerp].ppo"))
        })
        .ok_or("environment direct two-cube pixel stage absent")?;
    let mut state = *state;
    // The six mask passes write region exclusion bits0..2. They are deliberately not reconstructed here.
    // Keep only the already stated geometry bit0x10, with the same comparison and no stencil writes.
    if let Some(stencil) = &mut state.stencil {
        stencil.read_mask = 0x10;
        stencil.reference = Some(0x10);
    }
    let mut draws = Vec::with_capacity(regions.len());
    for (object, region) in regions {
        let mut stages = vec![vertex.clone(), pixel.clone()];
        for stage in &mut stages {
            bind(stage, region, world)?;
        }
        draws.push(RenderPass {
            name: format!(
                "environment region {}",
                world.get::<Name>(object).map_or("unnamed", |n| n.0.as_str())
            ),
            shared: false,
            presence: Presence::Always,
            runs: Runs::Once,
            work: PassWork::Geometry {
                primitive: GraphPrimitive::UnitBox,
                object,
                stages,
                state,
                outputs: outputs.clone(),
            },
        });
    }
    graph.passes[index].runs = Runs::Once;
    graph.passes[index].presence = Presence::Always;
    graph.passes[index].work = PassWork::Group { steps: draws };
    Ok(Some(index as u32))
}

fn bind(stage: &mut StageBinding, region: &ReflectionProbeRegion, world: &World) -> Result<(), String> {
    stage.variant = None;
    if stage.stage == Stage::Vertex {
        for input in &mut stage.inputs {
            if input.label.name == "POSITION" && input.label.index == 0 {
                input.role = InputRole::Stream(Attribute::Position);
            }
        }
    }
    let shader = world
        .get::<Shader>(stage.shader)
        .ok_or("environment program missing")?;
    let states = [&region.blend.state_a, &region.blend.state_b];
    let diffuse = states.map(|state| {
        state
            .diffuse_volume
            .and_then(|e| world.get::<IrradianceVolume>(e))
            .ok_or("environment state has no diffuse volume")
    });
    let [a, b] = diffuse;
    let volumes = [a?, b?];
    let texture = world
        .get::<TextureLayers>(volumes[0].coefficients[0])
        .ok_or("environment coefficient absent")?;
    let TextureShape::Volume { depth } = texture.shape else {
        return Err("environment coefficient is not a volume".into());
    };
    let grid = [texture.width as f32, texture.height as f32, depth as f32, 0.];
    for slot in &mut stage.textures {
        let name = shader
            .interface
            .resource(slot.resource)
            .map_or("", |r| r.provenance.name.as_str());
        let entity: Option<Entity> = match name {
            "g_IBLTexture" => Some(states[0].cube),
            "g_IBLTexture2" => Some(states[1].cube),
            "g_IrradianceVolumeX" => Some(volumes[0].coefficients[0]),
            "g_IrradianceVolumeY" => Some(volumes[0].coefficients[1]),
            "g_IrradianceVolumeZ" => Some(volumes[0].coefficients[2]),
            "g_IrradianceVolumeW" => Some(volumes[0].coefficients[3]),
            "g_IrradianceVolume2X" => Some(volumes[1].coefficients[0]),
            "g_IrradianceVolume2Y" => Some(volumes[1].coefficients[1]),
            "g_IrradianceVolume2Z" => Some(volumes[1].coefficients[2]),
            "g_IrradianceVolume2W" => Some(volumes[1].coefficients[3]),
            _ => None,
        };
        if let Some(texture) = entity {
            slot.source = TextureSource::Texture {
                texture,
                sampler: Default::default(),
            };
        }
    }
    let words = |v: &[f32]| ConstantRole::Material(v.iter().map(|f| f.to_bits()).collect());
    for c in &mut stage.constants {
        let buffer = shader
            .interface
            .resource(c.resource)
            .map_or("", |r| r.provenance.name.as_str());
        if buffer != "CB_ENVMAP_REGION" {
            continue;
        }
        let name = c.name.as_deref().unwrap_or("");
        let object = match name {
            "g_envMapWorld" => Some(ObjectParameter::World),
            "g_envMapInvWorld" | "g_envMapInvIv" => Some(ObjectParameter::InverseWorld),
            "g_envMapViewToLocal" => Some(ObjectParameter::ViewToObject),
            _ => None,
        };
        if let Some(parameter) = object {
            c.role = ConstantRole::Object(parameter);
            // Both native stages dot vectors with consecutive float4 rows, as DXIL establishes.
            c.lines = Some(MatrixLines::Rows);
            continue;
        }
        c.role = match name {
            "g_envMapDrawAreaExtents" => words(&[
                region.half_extents[0],
                region.half_extents[1],
                region.half_extents[2],
                0.,
            ]),
            "g_envMapColorScale" => words(&[states[0].intensity]),
            "g_envMapDiffuseColorScale" => words(&[volumes[0].diffuse_scale]),
            "g_envMapFadeDistance" => words(&[region.fade_distance]),
            "g_envMapInvFadeDistance" => words(&[if region.fade_distance > 0. {
                region.fade_distance.recip()
            } else {
                f32::MAX
            }]),
            "g_envMapFadeRate" => words(&[1.]),
            "g_envMapIvSamplingParam" => words(&grid),
            "g_envMapInterpRatio" => words(&[region.blend.factor]),
            "g_envMapMipmapLevelNum0" | "g_envMapMipmapLevelNum1" => {
                let state = states[usize::from(name.ends_with('1'))];
                let maximum = match state.mip_policy {
                    ProbeMipPolicy::Fixed(lod) => lod,
                    ProbeMipPolicy::LinearRoughness { maximum, .. } => maximum,
                };
                words(&[maximum])
            }
            "g_ivSafeVoxelJacobian0" | "g_ivSafeVoxelJacobian1" => {
                words(&[1. - 2. / grid[0], 1. - 2. / grid[1], 1. - 2. / grid[2], 0.])
            }
            "g_ivShLuminance0" | "g_ivShLuminance1" => ConstantRole::Default {
                words: vec![0; 4],
                why: LUMINANCE.into(),
            },
            _ => continue, // Unread PCC/debug/padding remain explicitly unknown, never populated from a capture.
        };
    }
    Ok(())
}
