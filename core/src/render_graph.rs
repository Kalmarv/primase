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

use crate::shader::StageBinding;
use crate::{Entity, JsonError, Resource, World};
use iridia_interface::{RenderState, ResourceId};

/// A target's index in [`RenderGraph::targets`].
pub type TargetId = u32;

/// `core.render_graph`: see the module documentation.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct RenderGraph {
    pub targets: Vec<RenderTarget>,
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
    /// The view's size times `numerator / denominator`, each side rounded down, at least 1 (a half-resolution target:
    /// 1/2). A volume's depth is stated in `depth`.
    View {
        numerator: u32,
        denominator: u32,
        depth: u32,
    },
    /// A fixed size whatever the view.
    Fixed { width: u32, height: u32, depth: u32 },
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
    pub work: PassWork,
}

/// What a step does.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PassWork {
    /// A draw of `vertices` procedural vertices (3 or 4: a full-screen triangle or quad; the vertex program makes them
    /// from their index) through `stages` (a vertex and a pixel program), with `state`, into `outputs`.
    Screen {
        vertices: u32,
        stages: Vec<StageBinding>,
        state: RenderState,
        outputs: Outputs,
    },
    /// A dispatch of a compute program: `groups` thread groups, writing the targets its unordered-access resources name.
    Compute {
        stage: StageBinding,
        groups: Groups,
        writes: Vec<(ResourceId, TargetView)>,
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
    /// Where another feature draws, into these outputs (its own programs and data).
    Slot { slot: FeatureSlot, outputs: Outputs },
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
    /// The targets are well formed (a size and at least one mip), every view names a target and a mip it has, colour and
    /// depth go to targets of their kind, and the picture is a target.
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
                } if numerator == 0 || denominator == 0 || depth == 0 => {
                    return Err(format!("target {} has a zero size factor", t.name));
                }
                TargetSize::Fixed { width, height, depth } if width == 0 || height == 0 || depth == 0 => {
                    return Err(format!("target {} has a zero size", t.name));
                }
                _ => {}
            }
        }
        for p in &self.passes {
            let what = format!("pass {}", p.name);
            match &p.work {
                PassWork::Screen {
                    vertices, outputs, ..
                } => {
                    if *vertices == 0 {
                        return Err(format!("{what} draws no vertex"));
                    }
                    self.outputs_ok(outputs, &what)?;
                }
                PassWork::Compute { writes, groups, .. } => {
                    for (_, v) in writes {
                        self.view_ok(v, &what)?;
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
                }
                PassWork::Scene { outputs, view, .. } => {
                    self.outputs_ok(outputs, &what)?;
                    if let SceneView::ShadowCascade { region, .. } = view {
                        self.region_ok(outputs, region, &what)?;
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
            }
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

    /// The `core.shader` entities the passes' programs are on.
    fn shaders(&self) -> impl Iterator<Item = Entity> + '_ {
        self.passes.iter().flat_map(|p| -> Vec<Entity> {
            match &p.work {
                PassWork::Screen { stages, .. } => stages.iter().map(|s| s.shader).collect(),
                PassWork::Compute { stage, .. } => vec![stage.shader],
                _ => Vec::new(),
            }
        })
    }
}

impl Resource for RenderGraph {
    const NAME: &'static str = "core.render_graph";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// [`RenderGraph::check`], and every program a `core.shader` of the world.
    fn verify(&self, w: &World) -> Result<(), String> {
        self.check()?;
        if let Some(e) = self
            .shaders()
            .find(|&e| w.get::<crate::shader::Shader>(e).is_none())
        {
            return Err(format!("a pass's program {e:?} is not a core.shader"));
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
            passes: vec![RenderPass {
                name: "clear".into(),
                shared: false,
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
}
