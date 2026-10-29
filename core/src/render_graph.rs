//! `core.render_graph`: the frame a source's renderer draws, as data (a world resource): the images it makes and the
//! passes that make them, in order, each a program of the source's with the role of everything it reads and writes. A
//! consumer runs it with generic machinery: it allocates the targets, runs each pass's programs with the bindings the
//! pass states, and gives each role its value; it knows no game. Where a renderer's meaning has no core role, the pass
//! states the source's own words (`ConstantRole::Material`) that the source's lowering computed from its data.
//!
//! The scene's own draws (the G-buffer, the shadow casters, the forward and effect draws) are passes of kind
//! [`PassWork::Scene`]: they draw the scene's materials through their `core.shader_binding`s, the bucket each draw belongs
//! to stated by the source ([`DrawBucket`]). Everything else is a full-screen draw, a compute dispatch, a clear or a copy
//! of the renderer's own programs, or a slot where another feature draws ([`FeatureSlot`]).

use serde::Serialize;
use serde_json::Value;

use crate::shader::{BindingPlace, BufferSource, StageBinding, TextureSource};
use crate::{Entity, JsonError, Resource, World};
use iridia_interface::{Interface, RenderState, ResourceId, ResourceKind, TextureDim};

/// A target's index in [`RenderGraph::targets`].
pub type TargetId = u32;
/// A buffer's index in [`RenderGraph::buffers`].
pub type BufferId = u32;

/// `core.render_graph`: see the module documentation.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct RenderGraph {
    pub targets: Vec<RenderTarget>,
    /// The buffers the frame's own steps write and read (the light lists a compute step builds for the lighting steps).
    pub buffers: Vec<RenderBuffer>,
    pub passes: Vec<RenderPass>,
    /// The target that holds the finished picture, before any interface the game draws over it.
    pub picture: Option<TargetId>,
}

/// One image the frame makes (or keeps from the frame before).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RenderTarget {
    /// The renderer's name for it where the source states one, else what it holds.
    pub name: String,
    pub format: TargetFormat,
    pub size: TargetSize,
    pub shape: TargetShape,
    /// Mip levels, at least 1.
    pub mips: u32,
    /// The frame reads what the previous frame left in it (a history: temporal anti-aliasing, adapted exposure, a
    /// re-projected occlusion): the consumer keeps it from one frame to the next, for each view.
    pub history: bool,
    /// One image for the frame, shared by every view (a shadow atlas, a look-up table), rather than one per view.
    pub shared: bool,
}

/// One buffer the frame makes (or keeps from the frame before): its steps write it as an unordered-access view and read
/// it as a shader resource, the step that writes it named in the graph's order ([`PassWork::Compute::buffer_writes`]).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RenderBuffer {
    /// What it holds, as the step that writes it says (`GXLightOrganize16 u0`).
    pub name: String,
    pub size: BufferSize,
    /// The bytes of one element, where the source's views of it state one (a structured buffer's stride).
    pub stride: Option<u32>,
    /// The elements the source's views of it span, where they state a count.
    pub elements: Option<u64>,
    /// The frame reads what the previous frame left in it.
    pub history: bool,
    /// One buffer for the frame, shared by every view, rather than one per view.
    pub shared: bool,
}

/// A buffer's size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BufferSize {
    /// The same whatever the view.
    Fixed { bytes: u64 },
    /// The bytes the source's recorded frames had, drawn at the view size `view` (width, height): how the size follows
    /// the view is not established (the light lists of a tiled renderer grow with the number of screen tiles). A
    /// consumer drawing at another size states what it allocates.
    Recorded { bytes: u64, view: [u32; 2] },
}

/// How a target stores a texel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetFormat {
    Rgba32Float,
    Rgba16Float,
    Rgba16Unorm,
    Rg32Float,
    Rg16Float,
    R32Float,
    R32Uint,
    R16Float,
    R16Unorm,
    Rgb10A2Unorm,
    Rg11B10Float,
    Rgba8Unorm,
    Rgba8UnormSrgb,
    Rg8Unorm,
    R8Unorm,
    /// One unsigned 8-bit integer.
    R8Uint,
    /// Depth as a 32-bit float with an 8-bit stencil.
    Depth32Stencil8,
    /// Depth as a 32-bit float.
    Depth32,
    /// Depth as 24-bit unorm with an 8-bit stencil.
    Depth24Stencil8,
    /// Depth as 16-bit unorm (a shadow map).
    Depth16,
}

impl TargetFormat {
    /// Whether the format holds depth (and so is a depth target, not a colour one).
    pub fn is_depth(self) -> bool {
        matches!(
            self,
            Self::Depth32Stencil8 | Self::Depth32 | Self::Depth24Stencil8 | Self::Depth16
        )
    }
}

/// A target's size.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetSize {
    /// The view's size times `numerator / denominator`, each side rounded as `rounding` says, at least 1 (a
    /// half-resolution target: 1/2). A volume's depth is stated in `depth`.
    View {
        numerator: u32,
        denominator: u32,
        depth: u32,
        rounding: Rounding,
    },
    /// A fixed size whatever the view.
    Fixed { width: u32, height: u32, depth: u32 },
}

/// How a view-relative size that is not a whole number of texels is rounded, side by side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rounding {
    /// Down to the whole texel below.
    Down,
    /// Up to the whole texel above: a part of a texel counts as one (1080 / 16 is 68 texels).
    Up,
}

/// A target's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetShape {
    Plane,
    /// `layers` planes of the size.
    Array {
        layers: u32,
    },
    /// A volume `depth` deep (the size's depth).
    Volume,
    Cube,
}

/// One step of the frame.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RenderPass {
    /// The renderer's name for the step where it has one, else its program's (the name a consumer times it by).
    pub name: String,
    /// Run once a frame for every view (what the step makes does not depend on the eye: shadow cascades, look-up tables,
    /// the light lists of a view-independent volume), rather than once per view.
    pub shared: bool,
    /// Whether the frame has the step: a step the source draws only in some frames stands in its place in the order
    /// all the same, so that a frame without it skips it and one with it draws it there.
    pub presence: Presence,
    /// How many times the step runs where the frame has it.
    pub runs: Runs,
    pub work: PassWork,
}

/// Whether a frame has a step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    /// Every frame has it.
    Always,
    /// A frame has it where the condition holds, as the source's recorded frames show.
    When(Trigger),
    /// Some frames have it and what decides it is not known: the recorded frames that had it (the source's names for
    /// them), so that a consumer can say why it drew or skipped the step. Never empty.
    Unknown { frames: Vec<String> },
}

/// What makes the source draw a step that only some frames have.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// A local light that casts shadows is in view: the frame draws its shadow casters and lights with its shadow.
    LocalShadowCasters,
    /// The frame draws water: the steps that draw for it (its interaction maps, the screen-space reflections whose
    /// prepass draws the water) run.
    Water,
}

/// How many times a step runs where the frame has it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Runs {
    /// Once.
    Once,
    /// Once for each item of the frame that `items` says, each run with that item's own values (its constants are the
    /// roles its binding states); `seen` is how many runs the source's recorded frames had, one count per frame that
    /// had the step.
    Each { items: Items, seen: Vec<u32> },
}

/// What one run of a repeated step is for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Items {
    /// Not identified: the source's recorded frames run the step a varying number of times, and what one run draws
    /// for is not established; `why` says what was examined.
    Unknown { why: String },
}

/// What a step does.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PassWork {
    /// A draw of `vertices` vertices that cover the view (3: a triangle larger than the view; 4: a quad) through
    /// `stages` (a vertex and a pixel program), with `state`, into `outputs`. The vertex program makes them from their
    /// index, or reads them from the inputs its binding states as [`crate::shader::InputRole::ScreenTriangle`].
    Screen {
        vertices: u32,
        stages: Vec<StageBinding>,
        state: RenderState,
        outputs: Outputs,
        /// The rectangle of the outputs the draw covers (its viewport), where it is not the whole of them: a copy into
        /// one cascade's quarter of a shadow atlas.
        region: Option<Region>,
    },
    /// A dispatch of a compute program: `groups` thread groups, writing the targets its unordered-access resources name.
    Compute {
        stage: StageBinding,
        groups: Groups,
        writes: Vec<(ResourceId, TargetView)>,
        /// The graph's buffers it writes, by the program's unordered-access resource.
        buffer_writes: Vec<(ResourceId, BufferId)>,
    },
    /// The scene's draws of a bucket through their materials' programs, into `outputs`, seen from `view`.
    Scene {
        bucket: DrawBucket,
        view: SceneView,
        outputs: Outputs,
    },
    /// Set every texel of a target's view to a value.
    Clear { target: TargetView, value: [f32; 4] },
    /// Copy one target's view into another's of the same size and format.
    Copy { from: TargetView, to: TargetView },
    /// Set every element of a buffer to a value (its 32-bit words, as an unordered-access clear writes them).
    ClearBuffer { buffer: BufferId, value: [u32; 4] },
    /// Copy one buffer into another of the same size.
    CopyBuffer { from: BufferId, to: BufferId },
    /// Where another feature draws, into these outputs (its own programs and data).
    Slot { slot: FeatureSlot, outputs: Outputs },
    /// Steps that run together, in order: the unit a repeated step ([`Runs::Each`]) runs once per item, where the
    /// source runs several steps for each item (a copy, then a draw that reads it). Never empty.
    Group { steps: Vec<RenderPass> },
}

/// The render targets and depth a draw writes.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Outputs {
    /// By render-target slot.
    pub colour: Vec<(u32, TargetView)>,
    pub depth: Option<DepthView>,
}

/// One subresource range of a target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct TargetView {
    pub target: TargetId,
    pub mip: u32,
    pub first_layer: u32,
    /// Layers (a volume's slices, a cube's faces); 0 for all.
    pub layers: u32,
}

/// A depth target as a draw binds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct DepthView {
    pub view: TargetView,
    /// The draw only reads the depth (the target can be sampled at the same time).
    pub read_only_depth: bool,
    pub read_only_stencil: bool,
}

/// How many thread groups a dispatch runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Groups {
    Fixed([u32; 3]),
    /// Enough groups of `group` threads to cover a target's mip: `ceil(size / group)` per axis.
    Cover {
        target: TargetId,
        mip: u32,
        group: [u32; 3],
    },
}

/// Which of the scene's draws a scene step draws; each draw's bucket is the source's statement, never the consumer's
/// choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawBucket {
    /// Opaque and alpha-tested surfaces into the G-buffer.
    Deferred,
    /// Decals over the G-buffer.
    Decal,
    /// Surfaces lit as they are drawn, after the deferred light (the blended ones, water).
    Forward,
    /// The sky: its dome, the sun's and moon's discs, cloud layers and far scenery drawn as part of the sky, each by its
    /// own material, after the deferred light: the draws whose material's `core.draw_order` layer is
    /// [`crate::DrawLayer::Sky`]. Where its step stands against the source's fog composite (before it, so the fog lies
    /// over the sky, or after it) is the graph's order of steps, as the source draws it.
    Sky,
    /// The shadow casters of a light.
    ShadowCaster,
    /// Surfaces that write motion only.
    Velocity,
}

/// The view a scene step draws from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneView {
    /// The frame's camera.
    Camera,
    /// Cascade `cascade` of the sun's shadow (its matrix the role `ViewParameter::ShadowCascadeMatrix(cascade)`), drawn
    /// into `region` of the depth target: an atlas holds several cascades side by side (Sekiro's four 2048 x 2048
    /// quarters of one 4096 x 4096 target).
    ShadowCascade { cascade: u8, region: Region },
    /// A local light's view of the shadow casters around it, drawn into a region of the depth target that each run
    /// places itself (its viewport): a step that runs once per region of a local lights' shadow atlas
    /// ([`Runs::Each`]), Elden Ring's 64 x 64 regions of its 4096 x 4096 atlas. Which light and which face a run is for
    /// is not stated.
    LocalShadow,
}

/// A rectangle of a target's texels at its mip 0: its top-left corner `x`, `y` and its `width` and `height`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Another feature's place in the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureSlot {
    /// Particles and effects, drawn into the lit picture after the opaque composite and the fog.
    Effects,
}

impl RenderGraph {
    fn view_ok(&self, v: &TargetView, what: &str) -> Result<(), String> {
        let t = self
            .targets
            .get(v.target as usize)
            .ok_or_else(|| format!("{what} names target {} of {}", v.target, self.targets.len()))?;
        if v.mip >= t.mips {
            return Err(format!(
                "{what} names mip {} of target {} ({} mips)",
                v.mip, t.name, t.mips
            ));
        }
        Ok(())
    }
    fn outputs_ok(&self, o: &Outputs, what: &str) -> Result<(), String> {
        for (slot, v) in &o.colour {
            let what = format!("{what} target {slot}");
            self.view_ok(v, &what)?;
            if self.targets[v.target as usize].format.is_depth() {
                return Err(format!(
                    "{what} renders colour into the depth target {}",
                    self.targets[v.target as usize].name
                ));
            }
        }
        if let Some(d) = &o.depth {
            let what = format!("{what} depth");
            self.view_ok(&d.view, &what)?;
            if !self.targets[d.view.target as usize].format.is_depth() {
                return Err(format!(
                    "{what} uses {} as depth",
                    self.targets[d.view.target as usize].name
                ));
            }
        }
        Ok(())
    }
    /// A full-screen draw's region: not empty, and inside every output whose size is fixed.
    fn screen_region_ok(&self, o: &Outputs, region: &Region, what: &str) -> Result<(), String> {
        if region.width == 0 || region.height == 0 {
            return Err(format!("{what} draws into an empty region"));
        }
        let views = o
            .colour
            .iter()
            .map(|(_, v)| v)
            .chain(o.depth.as_ref().map(|d| &d.view));
        for v in views {
            if let TargetSize::Fixed { width, height, .. } = self.targets[v.target as usize].size {
                let right = region.x.checked_add(region.width);
                let bottom = region.y.checked_add(region.height);
                if right.is_none_or(|r| r > width) || bottom.is_none_or(|b| b > height) {
                    return Err(format!(
                        "{what} draws outside its target {} ({width} x {height})",
                        self.targets[v.target as usize].name
                    ));
                }
            }
        }
        Ok(())
    }
    /// A shadow cascade's region: not empty, drawn into a depth target, and inside it where its size is fixed.
    fn region_ok(&self, o: &Outputs, region: &Region, what: &str) -> Result<(), String> {
        let depth = o
            .depth
            .as_ref()
            .ok_or_else(|| format!("{what} draws a shadow cascade with no depth target"))?;
        if region.width == 0 || region.height == 0 {
            return Err(format!("{what} draws a shadow cascade into an empty region"));
        }
        if let TargetSize::Fixed { width, height, .. } = self.targets[depth.view.target as usize].size {
            let right = region.x.checked_add(region.width);
            let bottom = region.y.checked_add(region.height);
            if right.is_none_or(|r| r > width) || bottom.is_none_or(|b| b > height) {
                return Err(format!(
                    "{what} draws a shadow cascade outside its depth target ({width} x {height})"
                ));
            }
        }
        Ok(())
    }
    fn buffer_ok(&self, b: BufferId, what: &str) -> Result<(), String> {
        if b as usize >= self.buffers.len() {
            return Err(format!("{what} names buffer {b} of {}", self.buffers.len()));
        }
        Ok(())
    }
    /// The views a stage's texture slots bind of the graph's targets name a target and a mip it has, and the graph's
    /// buffers its buffer slots bind are the graph's.
    fn stage_targets_ok(&self, stages: &[&StageBinding], what: &str) -> Result<(), String> {
        for s in stages {
            for t in &s.textures {
                if let TextureSource::Target { view, .. } = &t.source {
                    self.view_ok(
                        view,
                        &format!("{what} {:?} texture slot {}", s.stage, t.resource.0),
                    )?;
                }
            }
            for b in &s.buffers {
                if let BufferSource::Graph(id) = b.source {
                    self.buffer_ok(id, &format!("{what} {:?} buffer slot {}", s.stage, b.resource.0))?;
                }
            }
        }
        Ok(())
    }
    /// One step: its presence and runs stated, its views and outputs well formed, a group's steps each in turn.
    fn pass_ok(&self, p: &RenderPass) -> Result<(), String> {
        let what = format!("pass {}", p.name);
        if let Presence::Unknown { frames } = &p.presence
            && frames.is_empty()
        {
            return Err(format!("{what} is in some frames, and names none"));
        }
        if let Runs::Each { items, seen } = &p.runs {
            if seen.is_empty() {
                return Err(format!(
                    "{what} runs once per item and states no count it was seen with"
                ));
            }
            let Items::Unknown { why } = items;
            if why.trim().is_empty() {
                return Err(format!(
                    "{what} runs once per item of an unknown kind with no why"
                ));
            }
        }
        match &p.work {
            PassWork::Screen {
                vertices,
                outputs,
                stages,
                region,
                ..
            } => {
                if *vertices == 0 {
                    return Err(format!("{what} draws no vertex"));
                }
                self.outputs_ok(outputs, &what)?;
                if let Some(region) = region {
                    self.screen_region_ok(outputs, region, &what)?;
                }
                self.stage_targets_ok(&stages.iter().collect::<Vec<_>>(), &what)?;
            }
            PassWork::Compute {
                writes,
                groups,
                stage,
                buffer_writes,
            } => {
                for (_, v) in writes {
                    self.view_ok(v, &what)?;
                }
                for (_, b) in buffer_writes {
                    self.buffer_ok(*b, &what)?;
                }
                if let Groups::Cover { target, mip, group } = groups {
                    self.view_ok(
                        &TargetView {
                            target: *target,
                            mip: *mip,
                            first_layer: 0,
                            layers: 0,
                        },
                        &what,
                    )?;
                    if group.contains(&0) {
                        return Err(format!("{what} covers with an empty group"));
                    }
                }
                self.stage_targets_ok(&[stage], &what)?;
            }
            PassWork::Scene { outputs, view, .. } => {
                self.outputs_ok(outputs, &what)?;
                match view {
                    SceneView::ShadowCascade { region, .. } => self.region_ok(outputs, region, &what)?,
                    SceneView::LocalShadow if outputs.depth.is_none() => {
                        return Err(format!(
                            "{what} draws a local light's shadow with no depth target"
                        ));
                    }
                    SceneView::LocalShadow | SceneView::Camera => {}
                }
            }
            PassWork::Slot { outputs, .. } => self.outputs_ok(outputs, &what)?,
            PassWork::Clear { target, value } => {
                self.view_ok(target, &what)?;
                if !value.iter().all(|x| x.is_finite()) {
                    return Err(format!("{what} clears to a value that is not finite"));
                }
            }
            PassWork::Copy { from, to } => {
                self.view_ok(from, &what)?;
                self.view_ok(to, &what)?;
            }
            PassWork::ClearBuffer { buffer, .. } => self.buffer_ok(*buffer, &what)?,
            PassWork::CopyBuffer { from, to } => {
                self.buffer_ok(*from, &what)?;
                self.buffer_ok(*to, &what)?;
            }
            PassWork::Group { steps } => {
                if steps.is_empty() {
                    return Err(format!("{what} is a group of no step"));
                }
                for s in steps {
                    self.pass_ok(s)?;
                }
            }
        }
        Ok(())
    }
    /// The targets are well formed (a size and at least one mip), and the buffers (some bytes), every view names a target
    /// and a mip it has and every buffer reference a buffer, colour and depth go to targets of their kind, a step in some
    /// frames names them, a repeated step states its counts, and the picture is a target.
    pub fn check(&self) -> Result<(), String> {
        for t in &self.targets {
            if t.mips == 0 {
                return Err(format!("target {} has no mip", t.name));
            }
            match t.size {
                TargetSize::View {
                    numerator,
                    denominator,
                    depth,
                    ..
                } if numerator == 0 || denominator == 0 || depth == 0 => {
                    return Err(format!("target {} has a zero size factor", t.name));
                }
                TargetSize::Fixed { width, height, depth } if width == 0 || height == 0 || depth == 0 => {
                    return Err(format!("target {} has a zero size", t.name));
                }
                _ => {}
            }
        }
        for b in &self.buffers {
            let bytes = match b.size {
                BufferSize::Fixed { bytes } | BufferSize::Recorded { bytes, .. } => bytes,
            };
            if bytes == 0 {
                return Err(format!("buffer {} has no byte", b.name));
            }
            if let BufferSize::Recorded { view, .. } = b.size
                && view.contains(&0)
            {
                return Err(format!("buffer {} was recorded at an empty view", b.name));
            }
            if b.stride == Some(0) || b.elements == Some(0) {
                return Err(format!(
                    "buffer {} states elements of no byte or no element",
                    b.name
                ));
            }
        }
        for p in &self.passes {
            self.pass_ok(p)?;
        }
        if let Some(p) = self.picture {
            self.view_ok(
                &TargetView {
                    target: p,
                    mip: 0,
                    first_layer: 0,
                    layers: 0,
                },
                "the picture",
            )?;
        }
        Ok(())
    }

    /// The stage bindings of the steps, a group's steps' included, each with its step's name.
    fn stages(&self) -> Vec<(&str, &StageBinding)> {
        fn walk<'a>(passes: &'a [RenderPass], out: &mut Vec<(&'a str, &'a StageBinding)>) {
            for p in passes {
                match &p.work {
                    PassWork::Screen { stages, .. } => {
                        out.extend(stages.iter().map(|s| (p.name.as_str(), s)))
                    }
                    PassWork::Compute { stage, .. } => out.push((p.name.as_str(), stage)),
                    PassWork::Group { steps } => walk(steps, out),
                    PassWork::Scene { .. }
                    | PassWork::Clear { .. }
                    | PassWork::Copy { .. }
                    | PassWork::ClearBuffer { .. }
                    | PassWork::CopyBuffer { .. }
                    | PassWork::Slot { .. } => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.passes, &mut out);
        out
    }

    /// The `core.shader` entities the passes' programs are on.
    fn shaders(&self) -> impl Iterator<Item = Entity> + '_ {
        self.stages().into_iter().map(|(_, s)| s.shader)
    }

    /// A stage's texture slots that bind a target read it as the target's shape: a plane as a 2D texture, an array of
    /// planes as an arrayed one, a volume as a 3D texture, a cube as a cube.
    fn target_dims_ok(&self, step: &str, s: &StageBinding, interface: &Interface) -> Result<(), String> {
        for t in &s.textures {
            let TextureSource::Target { view, .. } = &t.source else {
                continue;
            };
            let Some(target) = self.targets.get(view.target as usize) else {
                continue;
            };
            let Some(ResourceKind::Texture { dim, arrayed, .. }) =
                interface.resource(t.resource).map(|r| &r.kind)
            else {
                continue;
            };
            let shape = match target.shape {
                TargetShape::Plane => (TextureDim::D2, false),
                TargetShape::Array { .. } => (TextureDim::D2, true),
                TargetShape::Volume => (TextureDim::D3, false),
                TargetShape::Cube => (TextureDim::Cube, false),
            };
            if (*dim, *arrayed) != shape {
                return Err(format!(
                    "pass {step} {:?} texture slot {} is a {dim:?} texture (arrayed {arrayed}) and binds target {}, a {:?}",
                    s.stage, t.resource.0, target.name, target.shape
                ));
            }
        }
        Ok(())
    }
}

impl Resource for RenderGraph {
    const NAME: &'static str = "core.render_graph";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// [`RenderGraph::check`], and every step's stage binding verified against its program as a material's is, with the
    /// graph's targets allowed as textures (a slot reads a target as the target's shape).
    fn verify(&self, w: &World) -> Result<(), String> {
        self.check()?;
        for (step, s) in self.stages() {
            s.verify_in(w, BindingPlace::GraphStep)
                .map_err(|e| format!("pass {step}: {e}"))?;
            if let Some(shader) = w.get::<crate::shader::Shader>(s.shader) {
                self.target_dims_ok(step, s, &shader.interface)?;
            }
        }
        Ok(())
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.shaders());
    }
    /// A graph whose program left the world cannot run: a part without it keeps no graph's passes (the targets stay).
    fn retain_references(&mut self, keep: &dyn Fn(Entity) -> bool) {
        if self.shaders().any(|e| !keep(e)) {
            self.passes.clear();
            self.picture = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(name: &str, format: TargetFormat) -> RenderTarget {
        RenderTarget {
            name: name.into(),
            format,
            size: TargetSize::View {
                numerator: 1,
                denominator: 1,
                depth: 1,
                rounding: Rounding::Down,
            },
            shape: TargetShape::Plane,
            mips: 1,
            history: false,
            shared: false,
        }
    }
    fn view(target: TargetId) -> TargetView {
        TargetView {
            target,
            mip: 0,
            first_layer: 0,
            layers: 0,
        }
    }

    #[test]
    fn views_name_targets_and_mips_they_have_and_depth_goes_to_depth() {
        let mut g = RenderGraph {
            targets: vec![
                target("light", TargetFormat::Rg11B10Float),
                target("depth", TargetFormat::Depth32Stencil8),
            ],
            buffers: Vec::new(),
            passes: vec![RenderPass {
                name: "clear".into(),
                shared: false,
                presence: Presence::Always,
                runs: Runs::Once,
                work: PassWork::Clear {
                    target: view(0),
                    value: [0.0; 4],
                },
            }],
            picture: Some(0),
        };
        assert_eq!(g.check(), Ok(()));
        g.passes[0].work = PassWork::Clear {
            target: TargetView { mip: 1, ..view(0) },
            value: [0.0; 4],
        };
        assert_eq!(
            g.check(),
            Err("pass clear names mip 1 of target light (1 mips)".into())
        );
        g.passes[0].work = PassWork::Slot {
            slot: FeatureSlot::Effects,
            outputs: Outputs {
                colour: vec![(0, view(1))],
                depth: None,
            },
        };
        assert_eq!(
            g.check(),
            Err("pass clear target 0 renders colour into the depth target depth".into())
        );
        g.passes[0].work = PassWork::Slot {
            slot: FeatureSlot::Effects,
            outputs: Outputs {
                colour: vec![(0, view(0))],
                depth: Some(DepthView {
                    view: view(0),
                    read_only_depth: true,
                    read_only_stencil: true,
                }),
            },
        };
        assert_eq!(g.check(), Err("pass clear depth uses light as depth".into()));
        g.picture = Some(5);
        g.passes.clear();
        assert_eq!(g.check(), Err("the picture names target 5 of 2".into()));
    }

    /// Sekiro's shadow atlas (a recorded frame: one 4096 x 4096 R16 depth target, its cascades' draws in the four
    /// 2048 x 2048 quarters): a cascade's region must be inside the atlas.
    #[test]
    fn a_shadow_cascade_draws_into_a_region_of_its_atlas() {
        let mut atlas = target("shadow", TargetFormat::Depth16);
        atlas.size = TargetSize::Fixed {
            width: 4096,
            height: 4096,
            depth: 1,
        };
        let cascade = |region: Region| RenderPass {
            name: "cascade".into(),
            shared: true,
            presence: Presence::Always,
            runs: Runs::Once,
            work: PassWork::Scene {
                bucket: DrawBucket::ShadowCaster,
                view: SceneView::ShadowCascade { cascade: 3, region },
                outputs: Outputs {
                    colour: Vec::new(),
                    depth: Some(DepthView {
                        view: view(0),
                        read_only_depth: false,
                        read_only_stencil: false,
                    }),
                },
            },
        };
        let quarter = Region {
            x: 2048,
            y: 2048,
            width: 2048,
            height: 2048,
        };
        let mut g = RenderGraph {
            targets: vec![atlas],
            buffers: Vec::new(),
            passes: vec![cascade(quarter)],
            picture: None,
        };
        assert_eq!(g.check(), Ok(()));
        g.passes[0] = cascade(Region { x: 2049, ..quarter });
        assert_eq!(
            g.check(),
            Err("pass cascade draws a shadow cascade outside its depth target (4096 x 4096)".into())
        );
        g.passes[0] = cascade(Region { width: 0, ..quarter });
        assert_eq!(
            g.check(),
            Err("pass cascade draws a shadow cascade into an empty region".into())
        );
    }

    /// A step only some frames have names them, a repeated step states the counts it was seen with and why its items
    /// are unknown, and a group holds steps, each checked in turn.
    #[test]
    fn a_step_in_some_frames_and_a_repeated_group_state_their_evidence() {
        let clear = |name: &str| RenderPass {
            name: name.into(),
            shared: false,
            presence: Presence::Always,
            runs: Runs::Once,
            work: PassWork::Clear {
                target: view(0),
                value: [0.0; 4],
            },
        };
        let mut g = RenderGraph {
            targets: vec![target("light", TargetFormat::Rg11B10Float)],
            buffers: Vec::new(),
            passes: vec![clear("a")],
            picture: Some(0),
        };
        g.passes[0].presence = Presence::Unknown { frames: Vec::new() };
        assert_eq!(g.check(), Err("pass a is in some frames, and names none".into()));
        g.passes[0].presence = Presence::Unknown {
            frames: vec!["frame 1".into()],
        };
        assert_eq!(g.check(), Ok(()));
        g.passes[0].runs = Runs::Each {
            items: Items::Unknown {
                why: "regions".into(),
            },
            seen: Vec::new(),
        };
        assert_eq!(
            g.check(),
            Err("pass a runs once per item and states no count it was seen with".into())
        );
        g.passes[0].runs = Runs::Each {
            items: Items::Unknown { why: " ".into() },
            seen: vec![3],
        };
        assert_eq!(
            g.check(),
            Err("pass a runs once per item of an unknown kind with no why".into())
        );
        g.passes[0].runs = Runs::Each {
            items: Items::Unknown {
                why: "regions".into(),
            },
            seen: vec![3, 12],
        };
        assert_eq!(g.check(), Ok(()));
        // a group: not empty, and its steps are checked as the graph's own
        g.passes[0].work = PassWork::Group { steps: Vec::new() };
        assert_eq!(g.check(), Err("pass a is a group of no step".into()));
        let mut inner = clear("b");
        inner.work = PassWork::Clear {
            target: view(4),
            value: [0.0; 4],
        };
        g.passes[0].work = PassWork::Group {
            steps: vec![clear("c"), inner],
        };
        assert_eq!(g.check(), Err("pass b names target 4 of 1".into()));
    }

    /// A step's texture slot that binds one of the graph's targets names a target and a mip the graph has.
    #[test]
    fn a_step_reads_targets_the_graph_has() {
        let mut w = World::new();
        let program = w.spawn();
        let read = |target: TargetId, mip: u32| crate::shader::TextureBinding {
            resource: ResourceId(0),
            sampler: None,
            source: TextureSource::Target {
                view: TargetView { mip, ..view(target) },
                sampler: iridia_interface::SamplerState::default(),
            },
        };
        let stage = |target: TargetId, mip: u32| StageBinding {
            stage: iridia_interface::Stage::Pixel,
            variant: None,
            shader: program,
            inputs: Vec::new(),
            constants: Vec::new(),
            textures: vec![read(target, mip)],
            buffers: Vec::new(),
            constant_buffers: Vec::new(),
            outputs: Vec::new(),
        };
        let screen = |target: TargetId, mip: u32| RenderPass {
            name: "composite".into(),
            shared: false,
            presence: Presence::Always,
            runs: Runs::Once,
            work: PassWork::Screen {
                vertices: 3,
                stages: vec![stage(target, mip)],
                state: RenderState::default(),
                outputs: Outputs {
                    colour: vec![(0, view(0))],
                    depth: None,
                },
                region: None,
            },
        };
        let mut g = RenderGraph {
            targets: vec![
                target("light", TargetFormat::Rg11B10Float),
                target("fog", TargetFormat::Rgba16Float),
            ],
            buffers: Vec::new(),
            passes: vec![screen(1, 0)],
            picture: Some(0),
        };
        assert_eq!(g.check(), Ok(()));
        g.passes[0] = screen(2, 0);
        assert_eq!(
            g.check(),
            Err("pass composite Pixel texture slot 0 names target 2 of 2".into())
        );
        g.passes[0] = screen(1, 1);
        assert_eq!(
            g.check(),
            Err("pass composite Pixel texture slot 0 names mip 1 of target fog (1 mips)".into())
        );
    }

    /// Sekiro's cascade copies (a recorded frame: four full-screen draws into the four 2048 x 2048 quarters of the 4096 x
    /// 4096 shadow atlas, each by its viewport): a full-screen draw's region is inside its fixed-size outputs.
    #[test]
    fn a_full_screen_draw_covers_a_region_inside_its_outputs() {
        let mut atlas = target("shadow", TargetFormat::Depth16);
        atlas.size = TargetSize::Fixed {
            width: 4096,
            height: 4096,
            depth: 1,
        };
        let copy = |region: Option<Region>| RenderPass {
            name: "copy cascade".into(),
            shared: true,
            presence: Presence::Always,
            runs: Runs::Once,
            work: PassWork::Screen {
                vertices: 3,
                stages: Vec::new(),
                state: RenderState::default(),
                outputs: Outputs {
                    colour: Vec::new(),
                    depth: Some(DepthView {
                        view: view(0),
                        read_only_depth: false,
                        read_only_stencil: false,
                    }),
                },
                region,
            },
        };
        let quarter = Region {
            x: 2048,
            y: 0,
            width: 2048,
            height: 2048,
        };
        let mut g = RenderGraph {
            targets: vec![atlas],
            buffers: Vec::new(),
            passes: vec![copy(None), copy(Some(quarter))],
            picture: None,
        };
        assert_eq!(g.check(), Ok(()));
        g.passes[1] = copy(Some(Region { y: 2049, ..quarter }));
        assert_eq!(
            g.check(),
            Err("pass copy cascade draws outside its target shadow (4096 x 4096)".into())
        );
        g.passes[1] = copy(Some(Region { height: 0, ..quarter }));
        assert_eq!(
            g.check(),
            Err("pass copy cascade draws into an empty region".into())
        );
    }

    /// Sekiro's light lists (a recorded frame at 1920 x 1080: GXLightOrganize16 writes four buffers, 522,240 bytes the
    /// first, that the tiled lighting reads): a buffer has bytes, a step names buffers the graph has, and a buffer's clear
    /// and copy name buffers too.
    #[test]
    fn steps_write_and_read_buffers_the_graph_has() {
        let mut w = World::new();
        let program = w.spawn();
        let lists = RenderBuffer {
            name: "GXLightOrganize16 u0".into(),
            size: BufferSize::Recorded {
                bytes: 522_240,
                view: [1920, 1080],
            },
            stride: None,
            elements: None,
            history: false,
            shared: false,
        };
        let organize = |buffer: BufferId| RenderPass {
            name: "GXLightOrganize16".into(),
            shared: false,
            presence: Presence::Always,
            runs: Runs::Once,
            work: PassWork::Compute {
                stage: StageBinding {
                    stage: iridia_interface::Stage::Compute,
                    variant: None,
                    shader: program,
                    inputs: Vec::new(),
                    constants: Vec::new(),
                    textures: Vec::new(),
                    buffers: vec![crate::shader::BufferBinding {
                        resource: ResourceId(1),
                        source: BufferSource::Graph(buffer),
                    }],
                    constant_buffers: Vec::new(),
                    outputs: Vec::new(),
                },
                groups: Groups::Fixed([120, 68, 1]),
                writes: Vec::new(),
                buffer_writes: vec![(ResourceId(0), buffer)],
            },
        };
        let step = |work: PassWork| RenderPass {
            name: "lists".into(),
            shared: false,
            presence: Presence::Always,
            runs: Runs::Once,
            work,
        };
        let mut g = RenderGraph {
            targets: Vec::new(),
            buffers: vec![lists.clone(), lists.clone()],
            passes: vec![
                step(PassWork::ClearBuffer {
                    buffer: 0,
                    value: [0; 4],
                }),
                organize(0),
                step(PassWork::CopyBuffer { from: 0, to: 1 }),
            ],
            picture: None,
        };
        assert_eq!(g.check(), Ok(()));
        g.passes[1] = organize(2);
        assert_eq!(
            g.check(),
            Err("pass GXLightOrganize16 names buffer 2 of 2".into())
        );
        g.passes[1] = organize(0);
        g.passes[2] = step(PassWork::CopyBuffer { from: 0, to: 5 });
        assert_eq!(g.check(), Err("pass lists names buffer 5 of 2".into()));
        g.passes.truncate(2);
        g.buffers[1].size = BufferSize::Fixed { bytes: 0 };
        assert_eq!(g.check(), Err("buffer GXLightOrganize16 u0 has no byte".into()));
    }
}
