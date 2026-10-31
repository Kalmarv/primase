//! Registered preparation of a source render graph for core lighting execution.
use fromsoft_formats::Game;
use iridia_interface::{BufferKind, ResourceKind, Stage};
use primase_core::World;
use primase_core::render_graph::{
    DrawBucket, GeometryStencil, LightingBuffer, LightingFrame, LightingPipeline, LightingPort, PassWork,
    RenderGraph,
};
use primase_core::shader::{
    BufferSource, ConstantRole, InputRole, NormalEncoding, ScreenTriangleValue, Shader, TextureSource,
};
mod ambient;
mod sun;

use crate::sekiro::{PreparedLighting, source::ReadScope};
use primase_core::Resource;
use primase_core::pass::{Pass, PassLog, PassSpec};

/// Choose the supported lighting subgraph and supply its explicitly reported approximations.
/// The reader's full recorded graph and unknown inputs are retained until this pass is selected.
#[derive(Debug)]
pub struct LowerFromSoftRenderGraph;
pub const LOWER_FROMSOFT_RENDER_GRAPH: PassSpec = PassSpec {
    name: "lower-fromsoft-render-graph",
    options: &[],
    make: |_| Ok(Box::new(LowerFromSoftRenderGraph)),
};
impl Pass for LowerFromSoftRenderGraph {
    fn name(&self) -> &'static str {
        "lower-fromsoft-render-graph"
    }
    fn requires(&self) -> &[&'static str] {
        &[RenderGraph::NAME, ReadScope::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[PreparedLighting::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[RenderGraph::NAME]
    }
    fn approximates(&self) -> &'static str {
        "supported lighting stages; conservative light lists; reconstructed screen triangle; finite directional stand-ins; assumed geometry stencil; unexecuted native shadow and ambient stages named per pipeline"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        if let Some(prepared) = world.resource::<PreparedLighting>() {
            prepared.verify(world)?;
            log.count("lighting pipeline already prepared", 1);
            return Ok(());
        }
        let game = world.resource::<ReadScope>().ok_or("no source read scope")?.game;
        let mut graph = world
            .resource::<RenderGraph>()
            .ok_or("no source render graph")?
            .clone();
        if graph.lighting.is_some() {
            return Err("source graph has lighting without a prepared-lighting declaration".into());
        }
        let pipeline = describe(game, &mut graph, world, log)?;
        for notice in &pipeline.notices {
            log.approximate(notice, 1);
        }
        if let Some(stencil) = &pipeline.geometry_stencil {
            log.approximate(&stencil.why, 1);
        }
        log.count("lighting stages selected", pipeline.passes.len() as u64);
        graph.lighting = Some(pipeline);
        graph.check()?;
        world.insert_resource(graph);
        world.insert_resource(PreparedLighting);
        Ok(())
    }
}

fn describe(
    game: Game,
    graph: &mut RenderGraph,
    world: &World,
    log: &mut primase_core::pass::PassLog,
) -> Result<LightingPipeline, String> {
    let environment = if game == Game::EldenRing {
        ambient::select(graph, world)?
    } else {
        None
    };
    let selected_names: &[&str] = if game == Game::EldenRing {
        &[
            "GXLightAcc_DDirLightShadowed1",
            "GXRend_TileBasedLighting",
            "GXLightCmbAlbedo",
            "GXLightComposite",
        ]
    } else {
        &["GXRend_TileBasedLighting", "GXLightCmbAlbedo", "GXLightComposite"]
    };
    let mut passes = Vec::new();
    for &name in selected_names {
        let index = graph
            .passes
            .iter()
            .position(|p| p.name == name)
            .ok_or_else(|| format!("lighting graph has no {name}"))?;
        passes.push(index as u32);
    }
    let mut output_views = Vec::new();
    for index in &passes {
        let PassWork::Screen { outputs, .. } = &graph.passes[*index as usize].work else {
            return Err("lighting stage is not a screen draw".into());
        };
        output_views.extend(outputs.colour.iter().map(|(_, view)| *view));
    }
    let last = *passes.last().ok_or("no lighting stage")? as usize;
    if let Some(index) = environment {
        passes.push(index);
    }
    for (index, pass) in graph.passes.iter().enumerate() {
        if let PassWork::Clear { target, .. } = &pass.work
            && output_views.contains(target)
            && index < last
        {
            passes.push(index as u32);
        }
    }
    passes.sort_unstable();
    passes.dedup();
    let mut imports = Vec::new();
    let mut scene_depth = None;
    for pass in &graph.passes {
        if let PassWork::Scene {
            bucket: DrawBucket::Deferred,
            outputs,
            ..
        } = &pass.work
        {
            for (slot, view) in &outputs.colour {
                imports.push((LightingPort::GBuffer(*slot), *view));
            }
            scene_depth = outputs.depth.as_ref().map(|depth| depth.view);
            break;
        }
    }
    let scene_depth = scene_depth.ok_or("lighting has no G-buffer scene depth")?;
    imports.push((LightingPort::SceneDepth, scene_depth));
    let mut buffers = Vec::new();
    let mut diffuse = None;
    let mut specular = None;
    let mut hdr = None;
    for index in &passes {
        let pass = &mut graph.passes[*index as usize];
        let PassWork::Screen { stages, outputs, .. } = &mut pass.work else {
            continue;
        };
        if pass.name == "GXRend_TileBasedLighting" {
            diffuse = outputs
                .colour
                .iter()
                .find(|(slot, _)| *slot == 0)
                .map(|(_, v)| *v);
            specular = outputs
                .colour
                .iter()
                .find(|(slot, _)| *slot == 1)
                .map(|(_, v)| *v);
        }
        if pass.name == "GXLightComposite" {
            hdr = outputs
                .colour
                .iter()
                .find(|(slot, _)| *slot == 0)
                .map(|(_, v)| *v);
        }
        for stage in stages {
            let shader = world
                .get::<Shader>(stage.shader)
                .ok_or("lighting program absent")?;
            if game == Game::EldenRing
                && pass.name == "GXLightAcc_DDirLightShadowed1"
                && stage.stage == Stage::Pixel
            {
                let added = sun::bind(stage, shader)?;
                log.count("directional constant roles supplied", added as u64);
            }
            if stage.stage == Stage::Vertex {
                // The native GXLightAcc vertex program copies POSITION.xy/w to clip position and TEXCOORD.xy to
                // UV (times dynamic-resolution scale). A full-screen triangle's geometry suffices; its native VB
                // bytes are unavailable. This explicitly declared reconstruction is not a captured-VB claim.
                for input in &mut stage.inputs {
                    if matches!(input.role, InputRole::Unknown { .. }) {
                        input.role = match (input.label.name.as_str(), input.label.index) {
                            ("POSITION", 0) => InputRole::ScreenTriangle(ScreenTriangleValue::Position),
                            ("TEXCOORD", 0) => {
                                InputRole::ScreenTriangle(ScreenTriangleValue::TextureCoordinate)
                            }
                            _ => continue,
                        };
                        log.count("screen triangle input roles supplied", 1);
                    }
                }
                for constant in &mut stage.constants {
                    if !matches!(constant.role, ConstantRole::Unknown { .. }) {
                        continue;
                    }
                    let buffer = shader
                        .interface
                        .resource(constant.resource)
                        .map(|r| r.provenance.name.as_str());
                    let default = match (buffer, constant.name.as_deref()) {
                        (Some("cbLightAccVtxParam"), Some("g_vtxParam")) => Some((
                            vec![0.0f32.to_bits()],
                            "full-screen lighting clip depth is far depth0; the selected draws disable depth testing; native vertex constant was not captured",
                        )),
                        (Some("cbRendererCommonParam"), Some("g_DynamicResolution_ScreenPercentage")) => {
                            Some((
                                vec![1.0f32.to_bits(); 4],
                                "full-screen lighting draws at the full consumer view size; native dynamic-resolution scale was not captured",
                            ))
                        }
                        _ => None,
                    };
                    if let Some((words, why)) = default {
                        constant.role = ConstantRole::Default {
                            words,
                            why: why.into(),
                        };
                        log.approximate(
                            &format!(
                                "{}: {}: {why}",
                                pass.name,
                                constant.name.as_deref().unwrap_or("unnamed constant")
                            ),
                            1,
                        );
                    }
                }
            }
            for texture in &stage.textures {
                let TextureSource::Target { view } = texture.source else {
                    continue;
                };
                if graph.targets[view.target as usize].format.is_depth() {
                    if !imports.iter().any(|(_, v)| *v == view) {
                        imports.push((LightingPort::SceneDepth, view));
                    }
                } else if let Some(register) = shader
                    .interface
                    .resource(texture.resource)
                    .and_then(|r| r.provenance.d3d)
                    .map(|d| d.register)
                {
                    let port = match (game, register) {
                        (Game::EldenRing, 32) => Some(LightingPort::ScreenSpaceShadows),
                        (Game::Sekiro, 33) => Some(LightingPort::AmbientOcclusion),
                        _ => None,
                    };
                    if let Some(port) = port
                        && !imports.iter().any(|(_, v)| *v == view)
                    {
                        imports.push((port, view));
                    }
                }
            }
            for binding in &stage.buffers {
                let BufferSource::Graph(buffer) = binding.source else {
                    continue;
                };
                let resource = shader
                    .interface
                    .resource(binding.resource)
                    .ok_or("lighting buffer resource absent")?;
                let ResourceKind::Buffer(BufferKind::Structured { stride }) = resource.kind else {
                    continue;
                };
                let source = crate::light_records::source(game, &resource.provenance.name, stride)
                    .ok_or_else(|| format!("unresolved lighting buffer {}", resource.provenance.name))?;
                if !buffers.iter().any(|b: &LightingBuffer| b.buffer == buffer) {
                    buffers.push(LightingBuffer { buffer, source });
                }
            }
        }
    }
    let mut notices = vec!["light lists conservatively include every light in every tile and depth slice; native culling is not reproduced".into(),
        if environment.is_some() { "native ambient occlusion and shadow producer stages remain unexecuted".into() } else { "native environment, ambient occlusion and shadow producer stages remain unexecuted".into() },
        "full-screen vertex inputs are reconstructed from the native lighting vertex program's clip-position and UV reads, not captured vertex-buffer bytes".into()];
    if game == Game::EldenRing {
        notices.push(sun::NOTICE.into());
        notices.push("sun shadows: one generic cascade fitted to the view, all opaque geometry casts; native casters and cascade splits not traced".into());
    } else {
        notices.push("native directional lighting is not yet selected for this source".into());
    }
    let frame = if environment.is_some() {
        notices.extend([
            ambient::DIRECT.into(),
            ambient::LUMINANCE.into(),
            ambient::MASKS.into(),
        ]);
        let normal = imports
            .iter()
            .find_map(|(port, v)| (*port == LightingPort::GBuffer(0)).then_some(*v))
            .ok_or("environment has no world-normal import")?;
        Some(LightingFrame {
            world_from_lighting: [[1., 0., 0.], [0., 1., 0.], [0., 0., -1.]],
            normal_inputs: vec![(
                normal,
                NormalEncoding::XyzBiased {
                    bias_bits: 0x3f00_2008,
                },
            )],
        })
    } else {
        None
    };
    let pipeline = LightingPipeline {
        frame,
        imports,
        buffers,
        passes,
        exports: vec![
            (LightingPort::DiffuseLight, diffuse.ok_or("no diffuse output")?),
            (LightingPort::SpecularLight, specular.ok_or("no specular output")?),
            (LightingPort::HdrColour, hdr.ok_or("no HDR output")?),
        ],
        notices,
        geometry_stencil: Some(GeometryStencil {
            target: scene_depth,
            value: 0x10,
            write_mask: 0x10,
            reference: 0x10,
            why: "stencil 0x10 assumed set on all geometry pixels (depth not far); producer not traced"
                .into(),
        }),
    };
    pipeline.check(graph)?;
    Ok(pipeline)
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::{PassManager, PassRegistry, RunReport};

    /// A raw graph does not satisfy a downstream prepared-lighting dependency, even when it is otherwise valid.
    #[test]
    fn the_pass_manager_requires_preparation_before_graph_augmentation() {
        #[derive(Debug)]
        struct Augment;
        impl Pass for Augment {
            fn name(&self) -> &'static str {
                "test-augment-lighting"
            }
            fn requires(&self) -> &[&'static str] {
                &[RenderGraph::NAME, PreparedLighting::NAME]
            }
            fn run(&self, _world: &mut World, _log: &mut PassLog) -> Result<(), String> {
                Ok(())
            }
        }
        const AUGMENT: PassSpec = PassSpec {
            name: "test-augment-lighting",
            options: &[],
            make: |_| Ok(Box::new(Augment)),
        };
        let mut registry = PassRegistry::new();
        registry.add(&AUGMENT);
        let manager = PassManager::new(registry);
        let mut world = World::new();
        world.insert_resource(RenderGraph::default());
        let error = manager
            .run(
                &mut world,
                &manager.registry.parse("test-augment-lighting").unwrap(),
                &mut RunReport::default(),
            )
            .unwrap_err();
        assert!(error.to_string().contains(PreparedLighting::NAME), "{error}");
        assert!(PreparedLighting.verify(&world).is_err());
    }
}
