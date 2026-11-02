//! Registered preparation of a source render graph for core lighting execution.
use fromsoft_formats::Game;
use iridia_interface::{BufferKind, ResourceKind, Stage};
use primase_core::World;
use primase_core::render_graph::{
    DrawBucket, GeometryStencil, LightingBuffer, LightingPipeline, LightingPort, PassWork, RenderGraph,
};
use primase_core::shader::{
    BufferSource, ConstantRole, InputRole, ScreenTriangleValue, Shader, TextureSource,
};
mod producers;
mod sun;

use crate::sekiro::{PreparedLighting, source::ReadScope};
use primase_core::Resource;
use primase_core::pass::{Pass, PassLog, PassSpec};
use producers::Producers;
use std::collections::BTreeSet;

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
        "supported lighting stages; conservative light lists; reconstructed screen triangle; finite directional stand-ins; assumed geometry stencil; the recorded steps they depend on that are not executed, named and counted"
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

/// The recorded steps the consumer executes, by the source's name, and whether a lighting graph must have it.
const SELECTED: &[(&str, bool)] = &[
    (LOCAL_SHADOW_CASTERS, false),
    ("GXLightAcc_DDirLightShadowed1", false),
    ("GXRend_TileBasedLighting", true),
    ("GXLightAcc_DPointLightShadowed1", false),
    ("GXLightCmbAlbedo", true),
    ("GXLightComposite", true),
];

/// The step the reader states for the local lights' shadow casters, where the graph has their atlas.
const LOCAL_SHADOW_CASTERS: &str = "local light shadow casters";

/// The recorded steps the selected lighting depends on, directly or through what they read, that are not executed.
pub const UNEXECUTED: &str = "recorded lighting steps not executed";

/// Trace what the pipeline's steps read back to the recorded steps that last drew it ([`Producers`]): select the clears
/// and import the ports it needs, and state the recorded steps it does not execute, in place of an earlier account.
/// Every pass that changes the pipeline's steps accounts for them again.
pub(crate) fn account(
    graph: &RenderGraph,
    pipeline: &mut LightingPipeline,
    log: &mut PassLog,
) -> Result<(), String> {
    let selected: BTreeSet<u32> = pipeline.passes.iter().copied().collect();
    let atlas = graph.local_shadow_atlas.as_ref().map(|a| a.target);
    let producers = Producers::trace(graph, &selected, atlas)?;
    pipeline.passes.extend(&producers.clears);
    pipeline.passes.sort_unstable();
    pipeline.passes.dedup();
    for (port, view, _) in &producers.ports {
        if !pipeline.imports.iter().any(|(_, v)| v == view) {
            pipeline.imports.push((*port, *view));
        }
    }
    pipeline.notices.retain(|n| !producers::is_notice(n));
    pipeline.notices.extend(producers.notices());
    for (name, n) in &producers.unexecuted {
        log.omit(&format!("{UNEXECUTED}: {name}"), *n);
    }
    Ok(())
}

fn describe(
    game: Game,
    graph: &mut RenderGraph,
    world: &World,
    log: &mut primase_core::pass::PassLog,
) -> Result<LightingPipeline, String> {
    // the recorded steps the consumer executes, those the graph has
    let mut selected = BTreeSet::new();
    for &(name, required) in SELECTED {
        match graph.passes.iter().position(|p| p.name == name) {
            Some(index) => {
                selected.insert(index as u32);
            }
            None if required => return Err(format!("lighting graph has no {name}")),
            None => {}
        }
    }
    for index in &selected {
        let pass = &graph.passes[*index as usize];
        if !matches!(
            pass.work,
            PassWork::Screen { .. }
                | PassWork::Scene {
                    bucket: DrawBucket::ShadowCaster,
                    ..
                }
        ) {
            return Err(format!(
                "lighting stage {} is neither a screen draw nor a shadow caster step",
                pass.name
            ));
        }
    }
    let passes: Vec<u32> = selected.iter().copied().collect();
    let atlas = graph.local_shadow_atlas.as_ref().map(|a| a.target);
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
    let mut sun_notice = None;
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
            if pass.name == "GXLightAcc_DDirLightShadowed1" && stage.stage == Stage::Pixel {
                let (added, zero) = sun::bind(stage, shader)?;
                log.count("directional constant roles supplied", added as u64);
                sun_notice = Some(zero);
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
                if Some(view.target) == atlas {
                    // the local lights' shadow atlas: drawn by the selected caster step
                    continue;
                }
                if graph.targets[view.target as usize].format.is_depth()
                    && !imports.iter().any(|(_, v)| *v == view)
                {
                    imports.push((LightingPort::SceneDepth, view));
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
    let mut notices: Vec<String> = vec!["light lists conservatively include every light in every tile and depth slice; native culling is not reproduced".into(),
        "full-screen vertex inputs are reconstructed from the native lighting vertex program's clip-position and UV reads, not captured vertex-buffer bytes".into()];
    if selected
        .iter()
        .any(|i| graph.passes[*i as usize].name == LOCAL_SHADOW_CASTERS)
    {
        notices.push("local lights' shadows: the consumer chooses which shadow-casting lights to shadow and draws their faces' casters into the atlas; the lights it does not shadow are lit by the tile pass without one, and the packing names each".into());
    } else {
        notices.push("local lights that cast shadows are lit by the tile pass without them: no local-light shadow atlas or shadowed per-light pass is selected, so every light packs unshadowed (LightType 0) and the packing names each shadow-casting one".into());
    }
    match sun_notice {
        Some(zero) => {
            notices.push(sun::NOTICE.into());
            notices.push(zero);
        }
        None => notices.push("the recorded graph has no directional lighting step: no sun light".into()),
    }
    let mut pipeline = LightingPipeline {
        frame: None,
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
    account(graph, &mut pipeline, log)?;
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
