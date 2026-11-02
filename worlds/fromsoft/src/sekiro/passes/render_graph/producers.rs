//! What the selected lighting steps read that the recorded graph draws before them: for each target a selected step
//! reads (or draws over, keeping what is there), the steps that last wrote it. A clear of it is selected with the
//! steps; a recorded producer the consumer stands in for is imported through its port; every other recorded step the
//! lighting depends on, directly or through the steps it reads from, is not executed and is counted by name.
use primase_core::render_graph::{
    BufferId, DrawBucket, LightingPort, PassWork, RenderGraph, RenderPass, TargetId, TargetView,
};
use primase_core::shader::{BufferSource, StageBinding, TextureSource};
use std::collections::{BTreeMap, BTreeSet};

use super::UNEXECUTED;

/// How a port's notice ends, and how the light lists' notice begins.
const PORT_NOTICE: &str = "it and the steps it reads from are not executed";
const REPLACED_NOTICE: &str = "the consumer's light lists replace what the recorded";

/// Whether `notice` is one [`Producers::notices`] states.
pub(super) fn is_notice(notice: &str) -> bool {
    notice.starts_with(UNEXECUTED) || notice.starts_with(REPLACED_NOTICE) || notice.ends_with(PORT_NOTICE)
}

/// The consumer's port for what a recorded step it does not execute draws, by the step's name: the sun's screen-space
/// shadow mask and the ambient occlusion, which the deferred light reads.
const PORTS: &[(&str, LightingPort)] = &[
    ("GXRend_CompositeShadowMask", LightingPort::ScreenSpaceShadows),
    ("ScreenSpaceAO2_BilateralUpScale", LightingPort::AmbientOcclusion),
];

/// What the selected steps depend on.
#[derive(Debug, Default)]
pub(super) struct Producers {
    /// The clears selected with the steps: the last before a step of a target it reads or draws over.
    pub clears: BTreeSet<u32>,
    /// The recorded producers the consumer stands in for, through their ports.
    pub ports: Vec<(LightingPort, TargetView, String)>,
    /// The recorded steps the selected ones depend on that are not executed, by name: how many.
    pub unexecuted: BTreeMap<String, u64>,
    /// The recorded steps that write the buffers the consumer supplies, by name: how many.
    pub replaced: BTreeMap<String, u64>,
}

/// A target read: its view, and whether the read is of what a draw over it keeps (rather than a texture read).
struct Read {
    view: TargetView,
    kept: bool,
}

impl Producers {
    /// Trace the producers of the steps `selected` (top-level indices) of `graph`. The depth targets the selected steps
    /// read, other than the local lights' shadow `atlas` (which a selected step draws), are the scene's depth: the
    /// consumer supplies it whatever wrote it.
    pub(super) fn trace(
        graph: &RenderGraph,
        selected: &BTreeSet<u32>,
        atlas: Option<TargetId>,
    ) -> Result<Self, String> {
        let scene_depth: BTreeSet<TargetId> = selected
            .iter()
            .flat_map(|i| reads(&graph.passes[*i as usize]))
            .map(|read| read.view.target)
            .filter(|t| Some(*t) != atlas && graph.targets[*t as usize].format.is_depth())
            .collect();
        let skip = |target: TargetId| scene_depth.contains(&target);
        let mut out = Self::default();
        let mut counted = BTreeSet::new();
        let mut pending = Vec::new();
        let mut replaced = BTreeSet::new();
        for &index in selected {
            let pass = &graph.passes[index as usize];
            for read in reads(pass) {
                if skip(read.view.target) {
                    continue;
                }
                let mut found = false;
                for writer in (0..index as usize).rev() {
                    let w = &graph.passes[writer];
                    if !writes(w).contains(&read.view.target) {
                        continue;
                    }
                    if selected.contains(&(writer as u32)) {
                        found = true;
                        break;
                    }
                    match &w.work {
                        PassWork::Clear { .. } => {
                            out.clears.insert(writer as u32);
                            found = true;
                            break;
                        }
                        _ if gbuffer(w) => {
                            found = true;
                            break;
                        }
                        _ => {}
                    }
                    if !read.kept
                        && let Some((_, port)) = PORTS.iter().find(|(name, _)| *name == w.name)
                    {
                        if !out.ports.iter().any(|(_, v, _)| *v == read.view) {
                            out.ports.push((*port, read.view, w.name.clone()));
                        }
                        pending.push(writer);
                        found = true;
                        break;
                    }
                    // not executed: what the consumer has here is what an earlier selected step, clear or G-buffer draw left
                    pending.push(writer);
                }
                if !found && !read.kept {
                    return Err(format!(
                        "lighting step {} reads target {} that no selected step, clear, G-buffer draw or port produces",
                        pass.name, read.view.target
                    ));
                }
            }
            for buffer in buffers(pass) {
                if let Some(writer) = (0..index as usize)
                    .rev()
                    .find(|&w| buffer_writes(&graph.passes[w]).contains(&buffer))
                    && !selected.contains(&(writer as u32))
                    && !matches!(graph.passes[writer].work, PassWork::ClearBuffer { .. })
                    && replaced.insert(writer)
                {
                    *out.replaced.entry(graph.passes[writer].name.clone()).or_default() += 1;
                }
            }
        }
        // the steps the unexecuted ones read from, as far back as recorded steps draw them
        while let Some(index) = pending.pop() {
            if !counted.insert(index) {
                continue;
            }
            let pass = &graph.passes[index];
            *out.unexecuted.entry(pass.name.clone()).or_default() += 1;
            for read in reads(pass) {
                if skip(read.view.target) {
                    continue;
                }
                if let Some(writer) = (0..index)
                    .rev()
                    .find(|&w| writes(&graph.passes[w]).contains(&read.view.target))
                    && !selected.contains(&(writer as u32))
                    && !matches!(graph.passes[writer].work, PassWork::Clear { .. })
                    && !gbuffer(&graph.passes[writer])
                {
                    pending.push(writer);
                }
            }
        }
        Ok(out)
    }
}

impl Producers {
    /// The notices that state this account: the ports, the replaced light lists, the steps not executed.
    pub(super) fn notices(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .ports
            .iter()
            .map(|(port, _, writer)| {
                format!("{port:?}: the consumer supplies what the recorded {writer} draws; {PORT_NOTICE}")
            })
            .collect();
        if !self.replaced.is_empty() {
            out.push(format!("{REPLACED_NOTICE} {} write", names(&self.replaced)));
        }
        if !self.unexecuted.is_empty() {
            out.push(format!(
                "{UNEXECUTED} (what they draw is absent from the lit picture): {}",
                names(&self.unexecuted)
            ));
        }
        out
    }
}

/// Step names with their counts, as a notice lists them.
fn names(counts: &BTreeMap<String, u64>) -> String {
    counts
        .iter()
        .map(|(name, n)| {
            if *n == 1 {
                name.clone()
            } else {
                format!("{name} x{n}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether `pass` is one of the scene's G-buffer draws (its opaque surfaces and decals), which the consumer draws and
/// the lighting imports. The scene's other draws (the sun's cascade casters among them) are recorded steps like any.
fn gbuffer(pass: &RenderPass) -> bool {
    matches!(
        pass.work,
        PassWork::Scene {
            bucket: DrawBucket::Deferred | DrawBucket::Decal,
            ..
        }
    )
}

/// The targets `pass` reads: its programs' target textures, a copy's source, the scene draws' frame images, and each
/// target it draws over keeping what is there (blended, masked, stencil- or depth-tested, or a region of it).
fn reads(pass: &RenderPass) -> Vec<Read> {
    let mut out = Vec::new();
    let textures = |stages: &[StageBinding], out: &mut Vec<Read>| {
        for stage in stages {
            for texture in &stage.textures {
                if let TextureSource::Target { view } = texture.source {
                    out.push(Read { view, kept: false });
                }
            }
        }
    };
    let kept = |state: &iridia_interface::RenderState,
                outputs: &primase_core::render_graph::Outputs,
                partial: bool,
                out: &mut Vec<Read>| {
        for (slot, view) in &outputs.colour {
            let target = state.targets.get(*slot as usize).copied().flatten();
            let blended = target.is_some_and(|t| t.blend.is_some()) || (*slot == 0 && state.blend.is_some());
            let masked = target.is_some_and(|t| t.write_mask & 0xf != 0xf);
            let tested = state.stencil.is_some() || state.depth.is_some();
            if blended || masked || tested || partial {
                out.push(Read {
                    view: *view,
                    kept: true,
                });
            }
        }
    };
    match &pass.work {
        PassWork::Screen {
            stages,
            state,
            outputs,
            region,
            ..
        } => {
            textures(stages, &mut out);
            kept(state, outputs, region.is_some(), &mut out);
        }
        // a volume covers part of the view
        PassWork::Geometry {
            stages,
            state,
            outputs,
            ..
        } => {
            textures(stages, &mut out);
            kept(state, outputs, true, &mut out);
        }
        PassWork::Compute { stage, .. } => textures(std::slice::from_ref(stage), &mut out),
        PassWork::Copy { from, .. } => out.push(Read {
            view: *from,
            kept: false,
        }),
        // the scene's draws read the frame images, and are depth-tested over what their targets hold
        PassWork::Scene { frame, outputs, .. } => {
            out.extend(frame.iter().map(|(_, view)| Read {
                view: *view,
                kept: false,
            }));
            out.extend(
                outputs
                    .colour
                    .iter()
                    .map(|(_, view)| *view)
                    .chain(outputs.depth.iter().map(|d| d.view))
                    .map(|view| Read { view, kept: true }),
            );
        }
        PassWork::Group { steps } => {
            for step in steps {
                out.extend(reads(step));
            }
        }
        PassWork::Clear { .. }
        | PassWork::ClearBuffer { .. }
        | PassWork::CopyBuffer { .. }
        | PassWork::Slot { .. } => {}
    }
    out
}

/// The targets `pass` writes.
fn writes(pass: &RenderPass) -> Vec<TargetId> {
    let mut out = Vec::new();
    match &pass.work {
        PassWork::Screen { outputs, .. }
        | PassWork::Geometry { outputs, .. }
        | PassWork::Scene { outputs, .. }
        | PassWork::Slot { outputs, .. } => {
            out.extend(outputs.colour.iter().map(|(_, v)| v.target));
            out.extend(outputs.depth.iter().map(|d| d.view.target));
        }
        PassWork::Compute { writes, .. } => out.extend(writes.iter().map(|(_, v)| v.target)),
        PassWork::Clear { target, .. } => out.push(target.target),
        PassWork::Copy { to, .. } => out.push(to.target),
        PassWork::Group { steps } => {
            for step in steps {
                out.extend(writes(step));
            }
        }
        PassWork::ClearBuffer { .. } | PassWork::CopyBuffer { .. } => {}
    }
    out
}

/// The graph buffers `pass`'s programs read.
fn buffers(pass: &RenderPass) -> Vec<BufferId> {
    let stages: Vec<&StageBinding> = match &pass.work {
        PassWork::Screen { stages, .. } | PassWork::Geometry { stages, .. } => stages.iter().collect(),
        PassWork::Compute { stage, .. } => vec![stage],
        PassWork::Group { steps } => return steps.iter().flat_map(buffers).collect(),
        _ => Vec::new(),
    };
    stages
        .iter()
        .flat_map(|s| &s.buffers)
        .filter_map(|b| match b.source {
            BufferSource::Graph(buffer) => Some(buffer),
            _ => None,
        })
        .collect()
}

/// The graph buffers `pass` writes.
fn buffer_writes(pass: &RenderPass) -> Vec<BufferId> {
    match &pass.work {
        PassWork::Compute { buffer_writes, .. } => buffer_writes.iter().map(|(_, b)| *b).collect(),
        PassWork::ClearBuffer { buffer, .. } => vec![*buffer],
        PassWork::CopyBuffer { to, .. } => vec![*to],
        PassWork::Group { steps } => steps.iter().flat_map(buffer_writes).collect(),
        _ => Vec::new(),
    }
}
