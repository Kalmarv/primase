//! The source-owned lighting boundary of the recorded frame graph. Register/program names are resolved here,
//! never by the generic consumer. Conservative lists replace only the selected subgraph's external buffer inputs.
use fromsoft_formats::Game;
use iridia_interface::{BufferKind, ResourceKind, Stage};
use primase_core::World;
use primase_core::render_graph::{
    DrawBucket, GeometryStencil, LightingBuffer, LightingPipeline, LightingPort, PassWork, RenderGraph,
};
use primase_core::shader::{
    BufferSource, ConstantRole, InputRole, ScreenTriangleValue, Shader, TextureSource,
};

pub(super) fn describe(
    game: Game,
    graph: &mut RenderGraph,
    world: &World,
    report: &mut super::Report,
) -> Result<LightingPipeline, String> {
    let selected_names = ["GXRend_TileBasedLighting", "GXLightCmbAlbedo", "GXLightComposite"];
    let mut passes = Vec::new();
    for name in selected_names {
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
                        report.unknown_inputs = report.unknown_inputs.saturating_sub(1);
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
                        report.unknown_constants = report.unknown_constants.saturating_sub(1);
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
    let pipeline=LightingPipeline {imports,buffers,passes,
        exports: vec![(LightingPort::DiffuseLight,diffuse.ok_or("no diffuse output")?),
            (LightingPort::SpecularLight,specular.ok_or("no specular output")?),
            (LightingPort::HdrColour,hdr.ok_or("no HDR output")?)],
        notices: vec!["light lists conservatively include every light in every tile and depth slice; native culling is not reproduced".into(),
            "the lighting subgraph currently selects local-light accumulation and composition; native environment, ambient occlusion, directional and shadow stages remain unexecuted".into(),
            "full-screen vertex inputs are reconstructed from the native lighting vertex program's clip-position and UV reads, not captured vertex-buffer bytes".into()],
        geometry_stencil: Some(GeometryStencil {target:scene_depth,value:0x10,write_mask:0x10,reference:0x10,
            why:"stencil 0x10 assumed set on all geometry pixels (depth not far); producer not traced".into()}),
    };
    pipeline.check(graph)?;
    Ok(pipeline)
}
