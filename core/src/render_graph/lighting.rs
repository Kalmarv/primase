//! A lighting subgraph's typed ports. Passes and their programs remain in the source's RenderGraph.
use serde::Serialize;

use super::{BufferId, PassWork, RenderGraph, RenderPass, TargetView};
use crate::shader::{BufferSource, TextureSource};

/// The meaning of an image at the lighting boundary, independent of engine names and register numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LightingPort {
    /// The raw material output at this attachment, without decoding or re-encoding its surface.
    GBuffer(u32),
    SceneDepth,
    DiffuseLight,
    SpecularLight,
    /// A light-space depth image sampled with a comparison sampler.
    ShadowMap,
    /// A screen-space shadow term sampled with an ordinary sampler.
    ScreenSpaceShadows,
    /// A screen-space occlusion term, independent of shadow visibility.
    AmbientOcclusion,
    /// Linear lit colour before tone mapping and display encoding.
    HdrColour,
}

/// An externally supplied graph buffer. The selected passes must not also write it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LightingBuffer {
    pub buffer: BufferId,
    pub source: BufferSource,
}

/// An explicit source approximation for geometry stencil before the lighting stages. A consumer writes `value` under
/// `write_mask` wherever the view's depth differs from far depth; lighting states whose reference is unstated use
/// `reference`. The reason is always reported, never presented as an observed producer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GeometryStencil {
    pub target: TargetView,
    pub value: u32,
    pub write_mask: u32,
    pub reference: u32,
    pub why: String,
}

/// The part of a source graph a consumer runs between its material draws and presentation.
/// The source selects passes; a consumer never chooses programs by their names. Multiple input views can read the
/// same port (a source may keep two views of scene depth), but each exported meaning has one view.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct LightingPipeline {
    pub imports: Vec<(LightingPort, TargetView)>,
    pub buffers: Vec<LightingBuffer>,
    /// Top-level RenderGraph pass indices in strictly increasing source order; groups retain their own order.
    pub passes: Vec<u32>,
    pub exports: Vec<(LightingPort, TargetView)>,
    pub geometry_stencil: Option<GeometryStencil>,
    /// Source explanations of deliberate reconstruction or approximation, shown by every consumer.
    pub notices: Vec<String>,
}

impl LightingPipeline {
    /// Verify the port types, source order and dependency closure of the selected subgraph. Every read image/buffer
    /// must be imported or produced earlier. This checks data dependencies, not whether the consumer implements each
    /// pass or can evaluate an unknown presence condition.
    pub fn check(&self, graph: &RenderGraph) -> Result<(), String> {
        let mut images = Vec::new();
        for (port, view) in self.imports.iter().chain(&self.exports) {
            graph.view_ok(view, "lighting port")?;
            let depth = graph.targets[view.target as usize].format.is_depth();
            if depth != matches!(port, LightingPort::SceneDepth | LightingPort::ShadowMap) {
                return Err(format!(
                    "lighting port {port:?} has an incompatible target format"
                ));
            }
            if *port == LightingPort::HdrColour
                && !matches!(
                    graph.targets[view.target as usize].format,
                    super::TargetFormat::Rgba16Float
                        | super::TargetFormat::Rgba32Float
                        | super::TargetFormat::Rg11B10Float
                )
            {
                return Err("lighting HDR colour needs a linear floating-point colour target".into());
            }
        }
        for (_, view) in &self.imports {
            if images.contains(view) {
                return Err("a lighting image is imported twice".into());
            }
            images.push(*view);
        }
        let mut buffers = Vec::new();
        for input in &self.buffers {
            let buffer = graph
                .buffers
                .get(input.buffer as usize)
                .ok_or_else(|| format!("lighting imports absent buffer {}", input.buffer))?;
            if buffers.contains(&input.buffer) {
                return Err(format!("lighting imports buffer {} twice", input.buffer));
            }
            match &input.source {
                BufferSource::Lights(table) => table.verify(buffer.stride)?,
                BufferSource::LightLists(part) => part.verify(buffer.stride)?,
                BufferSource::Graph(_) => {
                    return Err("a lighting buffer import refers to another graph buffer".into());
                }
                _ => {}
            }
            buffers.push(input.buffer);
        }
        for (at, index) in self.passes.iter().enumerate() {
            if at > 0 && *index <= self.passes[at - 1] {
                return Err("lighting passes are repeated or outside source order".into());
            }
            let pass = graph
                .passes
                .get(*index as usize)
                .ok_or_else(|| format!("lighting selects absent pass {index}"))?;
            dependencies(pass, &mut images, &mut buffers, &self.buffers)?;
        }
        if let Some(stencil) = &self.geometry_stencil {
            graph.view_ok(&stencil.target, "lighting geometry stencil")?;
            if !matches!(
                graph.targets[stencil.target.target as usize].format,
                super::TargetFormat::Depth32Stencil8 | super::TargetFormat::Depth24Stencil8
            ) {
                return Err("lighting geometry stencil needs a depth/stencil image".into());
            }
            if stencil.value > 255
                || stencil.write_mask > 255
                || stencil.reference > 255
                || stencil.why.trim().is_empty()
            {
                return Err("lighting geometry stencil has invalid bits or no reason".into());
            }
            image_read(
                stencil.target,
                &self.imports.iter().map(|(_, view)| *view).collect::<Vec<_>>(),
                "lighting geometry stencil",
            )?;
        }
        let mut ports = Vec::new();
        for (port, view) in &self.exports {
            if ports.contains(port) {
                return Err(format!("lighting exports {port:?} twice"));
            }
            ports.push(*port);
            image_read(*view, &images, "lighting export")?;
        }
        if !ports.contains(&LightingPort::HdrColour) {
            return Err("lighting exports no linear HDR colour".into());
        }
        Ok(())
    }
}

fn image_read(view: TargetView, images: &[TargetView], what: &str) -> Result<(), String> {
    if images.iter().any(|available| {
        available.target == view.target
            && available.mip == view.mip
            && available.first_layer <= view.first_layer
            && (available.layers == 0
                || (view.layers != 0
                    && u64::from(view.first_layer) + u64::from(view.layers)
                        <= u64::from(available.first_layer) + u64::from(available.layers)))
    }) {
        Ok(())
    } else {
        Err(format!(
            "{what} reads target {} mip {} before it is imported or produced",
            view.target, view.mip
        ))
    }
}

fn dependencies(
    pass: &RenderPass,
    images: &mut Vec<TargetView>,
    buffers: &mut Vec<BufferId>,
    imports: &[LightingBuffer],
) -> Result<(), String> {
    let mut writes = Vec::new();
    let mut buffer_writes = Vec::new();
    let mut stages = Vec::new();
    match &pass.work {
        PassWork::Screen {
            stages: bound,
            outputs,
            ..
        }
        | PassWork::Geometry {
            stages: bound,
            outputs,
            ..
        } => {
            stages.extend(bound);
            writes.extend(outputs.colour.iter().map(|(_, v)| *v));
            if let Some(depth) = &outputs.depth {
                if depth.read_only_depth {
                    image_read(depth.view, images, &pass.name)?;
                } else {
                    writes.push(depth.view);
                }
            }
        }
        PassWork::Compute {
            stage,
            writes: targets,
            buffer_writes: bound,
            ..
        } => {
            stages.push(stage);
            writes.extend(targets.iter().map(|(_, view)| *view));
            buffer_writes.extend(bound.iter().map(|(_, id)| *id));
        }
        PassWork::Clear { target, .. } => writes.push(*target),
        PassWork::Copy { from, to } => {
            image_read(*from, images, &pass.name)?;
            writes.push(*to);
        }
        PassWork::ClearBuffer { buffer, .. } => buffer_writes.push(*buffer),
        PassWork::CopyBuffer { from, to } => {
            if !buffers.contains(from) {
                return Err(format!("{} reads unproduced buffer {from}", pass.name));
            }
            buffer_writes.push(*to);
        }
        PassWork::Group { steps } => {
            for step in steps {
                dependencies(step, images, buffers, imports)?;
            }
        }
        PassWork::Scene { .. } | PassWork::Slot { .. } => {
            return Err(format!(
                "{} requires scene draws inside the lighting subgraph",
                pass.name
            ));
        }
    }
    for stage in stages {
        for texture in &stage.textures {
            if let TextureSource::Target { view } = texture.source {
                image_read(view, images, &pass.name)?;
            }
        }
        for buffer in &stage.buffers {
            if let BufferSource::Graph(id) = buffer.source
                && !buffers.contains(&id)
            {
                return Err(format!(
                    "{} reads buffer {id} before it is imported or produced",
                    pass.name
                ));
            }
        }
    }
    for id in buffer_writes {
        if imports.iter().any(|input| input.buffer == id) {
            return Err(format!("{} writes imported lighting buffer {id}", pass.name));
        }
        buffers.push(id);
    }
    images.extend(writes);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_graph::{
        BufferSize, Presence, RenderBuffer, RenderTarget, Runs, TargetFormat, TargetShape, TargetSize,
    };

    fn view(target: u32) -> TargetView {
        TargetView {
            target,
            mip: 0,
            first_layer: 0,
            layers: 0,
        }
    }
    fn graph() -> RenderGraph {
        let target = |name: &str, format| RenderTarget {
            name: name.into(),
            format,
            size: TargetSize::Fixed {
                width: 2,
                height: 2,
                depth: 1,
            },
            shape: TargetShape::Plane,
            mips: 1,
            history: false,
            shared: false,
        };
        let pass = |name: &str, work| RenderPass {
            name: name.into(),
            shared: false,
            presence: Presence::Always,
            runs: Runs::Once,
            work,
        };
        RenderGraph {
            targets: vec![
                target("raw material", TargetFormat::Rgba16Float),
                target("linear lit", TargetFormat::Rgba16Float),
            ],
            passes: vec![pass(
                "handoff",
                PassWork::Copy {
                    from: view(0),
                    to: view(1),
                },
            )],
            ..RenderGraph::default()
        }
    }
    fn pipeline() -> LightingPipeline {
        LightingPipeline {
            imports: vec![(LightingPort::GBuffer(0), view(0))],
            passes: vec![0],
            exports: vec![(LightingPort::HdrColour, view(1))],
            ..LightingPipeline::default()
        }
    }
    #[test]
    fn lighting_ports_require_a_closed_ordered_hdr_handoff() {
        let g = graph();
        let p = pipeline();
        assert_eq!(p.check(&g), Ok(()));
        let mut missing = p.clone();
        missing.imports.clear();
        assert!(missing.check(&g).unwrap_err().contains("before it is imported"));
        let mut order = p.clone();
        order.passes.push(0);
        assert!(order.check(&g).unwrap_err().contains("source order"));
        let mut output = p.clone();
        output.exports[0].0 = LightingPort::DiffuseLight;
        assert!(output.check(&g).unwrap_err().contains("no linear HDR"));
        let mut wrong = p.clone();
        wrong.imports[0].0 = LightingPort::SceneDepth;
        assert!(wrong.check(&g).unwrap_err().contains("incompatible target"));
        let mut invalid = p.clone();
        invalid.exports.push((LightingPort::HdrColour, view(0)));
        assert!(invalid.check(&g).unwrap_err().contains("exports HdrColour twice"));
        let mut display = g.clone();
        display.targets[1].format = TargetFormat::Rgba8UnormSrgb;
        assert!(p.check(&display).unwrap_err().contains("linear floating-point"));
    }
    #[test]
    fn a_graph_buffer_cannot_have_two_selected_producers() {
        let mut g = graph();
        g.buffers.push(RenderBuffer {
            name: "list".into(),
            size: BufferSize::Fixed { bytes: 16 },
            stride: Some(4),
            elements: Some(4),
            history: false,
            shared: false,
        });
        g.passes[0].work = PassWork::ClearBuffer {
            buffer: 0,
            value: [0; 4],
        };
        let mut p = pipeline();
        p.buffers.push(LightingBuffer {
            buffer: 0,
            source: BufferSource::Unknown {
                why: "external source".into(),
            },
        });
        assert!(
            p.check(&g)
                .unwrap_err()
                .contains("writes imported lighting buffer")
        );
        p.buffers[0].source = BufferSource::Graph(0);
        assert!(p.check(&g).unwrap_err().contains("another graph buffer"));
    }
}
