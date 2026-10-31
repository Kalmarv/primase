//! Which of the frame's scene steps draw a surface, and through which program: `core.step_bindings` (on materials)
//! and `core.step_draws` (on placements).
//!
//! A frame's scene steps ([`crate::render_graph::PassWork::Scene`]) each make a draw the source names (Unity's
//! `LightMode` passes of the step, Sekiro's buckets), and a renderer draws a surface in some of them, each time
//! through a program of its material: its main draw's (`core.shader_binding`), or another pass of it (a depth
//! prepass draws a material through its depth pass). A source states each part where it lies:
//!
//! - on a material, each draw that draws it and the program it is drawn through there ([`StepBindings`]): a material
//!   without a step binding for a draw is not drawn in it, and a draw that draws it through several of its passes (a
//!   Unity draw draws every pass whose `LightMode` is one of the draw's) binds it once for each pass;
//! - on a placement, the draws of the steps that pick their placements one by one ([`StepDraws`],
//!   [`crate::render_graph::ScenePlacements::Listed`]): a renderer chooses them per drawn object (Unity by the layer of
//!   the object's GameObject), not per material.
//!
//! A material a draw draws whose program the source could not bind says why ([`StepProgram::Unbound`]): the draw then
//! draws something no binding states, and a consumer says what it does there.

use serde::Serialize;
use serde_json::Value;

use crate::render_graph::{RenderGraph, SceneMembers, ScenePlacements};
use crate::shader::ShaderBinding;
use crate::{Component, Entity, Instance, JsonError, Mesh, World};

/// One draw of the frame's scene steps that draws a material, and the program it draws it through.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StepBinding {
    /// The draw, as the scene steps name it ([`crate::render_graph::PassWork::Scene::draw`]).
    pub draw: String,
    /// The source's name for the material's pass the draw goes through (Unity's `LightMode` tag of the subshader's
    /// pass, `DepthNormalsOnly`), where the source names its passes.
    pub pass: Option<String>,
    pub program: StepProgram,
}

/// The program a draw draws a material through.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepProgram {
    /// The material's main draw's, its `core.shader_binding`.
    Main,
    /// Another pass's, bound here and held to what a material's `core.shader_binding` is held to.
    Bound(ShaderBinding),
    /// The draw draws the material, and the source could not bind the program: why.
    Unbound { why: String },
}

impl StepBinding {
    /// The binding the draw draws the material through, where the source bound one: the material's own
    /// `core.shader_binding` for [`StepProgram::Main`].
    pub fn binding<'a>(&'a self, material: Entity, w: &'a World) -> Option<&'a ShaderBinding> {
        match &self.program {
            StepProgram::Main => w.get::<ShaderBinding>(material),
            StepProgram::Bound(b) => Some(b),
            StepProgram::Unbound { .. } => None,
        }
    }
}

/// `core.step_bindings`, on a material entity: the draws of the frame's scene steps that draw it and the program each
/// draws it through: a draw once, or once for each pass of the material it draws, in the order it draws them.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct StepBindings(pub Vec<StepBinding>);

impl StepBindings {
    /// How `draw` draws the material: none where it does not, one binding for each pass it draws it through.
    pub fn of<'a>(&'a self, draw: &'a str) -> impl Iterator<Item = &'a StepBinding> + 'a {
        self.0.iter().filter(move |b| b.draw == draw)
    }
}

impl Component for StepBindings {
    const NAME: &'static str = "core.step_bindings";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        for b in &self.0 {
            if let StepProgram::Bound(binding) = &b.program {
                binding.references(out);
            }
        }
    }
    /// Not empty; a draw named once, or once for each of several passes, each named (not empty) and once; each draw
    /// one of the scene steps' draws that state what they draw where the world has a `core.render_graph`; a main-draw
    /// binding on a material that has its `core.shader_binding`, another pass's verified as a material's binding, and
    /// an unbound one saying why.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        if self.0.is_empty() {
            return Err("step bindings that bind no draw".into());
        }
        let draws: Option<Vec<(&str, &SceneMembers)>> = w.resource::<RenderGraph>().map(|g| g.scene_draws());
        for (i, b) in self.0.iter().enumerate() {
            if b.draw.trim().is_empty() {
                return Err("a step binding of no draw".into());
            }
            if self.0[..i]
                .iter()
                .any(|o| o.draw == b.draw && (o.pass.is_none() || o.pass == b.pass))
            {
                return Err(format!(
                    "the draw {} bound twice{}",
                    b.draw,
                    b.pass
                        .as_deref()
                        .map(|p| format!(" through the pass {p}"))
                        .unwrap_or_default()
                ));
            }
            if b.pass.is_none() && self.0.iter().filter(|o| o.draw == b.draw).count() > 1 {
                return Err(format!(
                    "the draw {} bound several times, not each through a named pass",
                    b.draw
                ));
            }
            if let Some(draws) = &draws {
                match draws.iter().find(|(d, _)| *d == b.draw).map(|(_, m)| m) {
                    None => {
                        return Err(format!(
                            "the draw {} is no scene step's of the world's core.render_graph",
                            b.draw
                        ));
                    }
                    Some(SceneMembers::NotRead { .. }) => {
                        return Err(format!(
                            "the draw {} is a scene step's of the world's core.render_graph that does not read what \
                             it draws",
                            b.draw
                        ));
                    }
                    Some(SceneMembers::Stated { .. }) => {}
                }
            }
            if b.pass.as_deref().is_some_and(|p| p.trim().is_empty()) {
                return Err(format!("the draw {}'s pass has an empty name", b.draw));
            }
            match &b.program {
                StepProgram::Main if !w.has::<ShaderBinding>(e) => {
                    return Err(format!(
                        "the draw {} draws the material's main binding, and it has no core.shader_binding",
                        b.draw
                    ));
                }
                StepProgram::Main => {}
                StepProgram::Bound(binding) => binding
                    .verify(e, w)
                    .map_err(|why| format!("the draw {}: {why}", b.draw))?,
                StepProgram::Unbound { why } if why.trim().is_empty() => {
                    return Err(format!("the draw {} is unbound and says not why", b.draw));
                }
                StepProgram::Unbound { .. } => {}
            }
        }
        Ok(())
    }
}

/// `core.step_draws`, on a placement (a `core.instance`) or an entity that draws its own mesh: the draws of the scene
/// steps that pick their placements one by one ([`ScenePlacements::Listed`]) that draw it, each once. A step whose
/// placements are listed draws no placement without it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StepDraws(pub Vec<String>);

impl StepDraws {
    /// Whether the steps making `draw` draw it.
    pub fn draws_in(&self, draw: &str) -> bool {
        self.0.iter().any(|d| d == draw)
    }
}

impl Component for StepDraws {
    const NAME: &'static str = "core.step_draws";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// On something drawn; not empty, each draw once, and the draw of a scene step that lists its placements where the
    /// world has a `core.render_graph`.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        if !w.has::<Instance>(e) && !w.has::<Mesh>(e) {
            return Err("step draws on an entity that draws nothing (no core.instance or core.mesh)".into());
        }
        if self.0.is_empty() {
            return Err("step draws that name no draw".into());
        }
        let listed: Option<Vec<&str>> = w.resource::<RenderGraph>().map(|g| {
            g.scene_draws()
                .into_iter()
                .filter(|(_, m)| {
                    **m == SceneMembers::Stated {
                        placements: ScenePlacements::Listed,
                    }
                })
                .map(|(d, _)| d)
                .collect()
        });
        for (i, d) in self.0.iter().enumerate() {
            if self.0[..i].contains(d) {
                return Err(format!("the draw {d} named twice"));
            }
            if let Some(listed) = &listed
                && !listed.contains(&d.as_str())
            {
                return Err(format!(
                    "the draw {d} is no scene step's of the world's core.render_graph that lists its placements"
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_graph::{
        DepthView, DrawBucket, DrawSorting, Outputs, PassWork, Presence, RenderPass, RenderTarget, Rounding,
        Runs, SceneView, TargetFormat, TargetShape, TargetSize, TargetView,
    };

    fn graph(draws: &[(&str, SceneMembers)]) -> RenderGraph {
        RenderGraph {
            targets: vec![RenderTarget {
                name: "depth".into(),
                format: TargetFormat::Depth32Stencil8,
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
            }],
            buffers: Vec::new(),
            passes: draws
                .iter()
                .map(|(draw, members)| RenderPass {
                    name: (*draw).into(),
                    shared: false,
                    presence: Presence::Always,
                    runs: Runs::Once,
                    work: PassWork::Scene {
                        bucket: DrawBucket::Forward,
                        view: SceneView::Camera,
                        outputs: Outputs {
                            colour: Vec::new(),
                            depth: Some(DepthView {
                                view: TargetView {
                                    target: 0,
                                    mip: 0,
                                    first_layer: 0,
                                    layers: 0,
                                },
                                read_only_depth: false,
                                read_only_stencil: false,
                            }),
                        },
                        draw: (*draw).into(),
                        members: members.clone(),
                        frame: Vec::new(),
                        sorting: DrawSorting::NotStated {
                            why: "a fixture".into(),
                        },
                    },
                })
                .collect(),
            picture: None,
            lighting: None,
            left_out: Vec::new(),
        }
    }

    /// A material's step bindings name each draw once, a draw of the graph's scene steps; the main draw's needs the
    /// material's binding, and an unbound one says why. A placement's step draws name listed draws of the graph, once
    /// each, on something drawn.
    #[test]
    fn step_bindings_and_draws_verify_their_own_rules() {
        let mut w = World::new();
        let material = w.spawn();
        let bound = |draw: &str, program| StepBinding {
            draw: draw.into(),
            pass: Some("DepthNormals".into()),
            program,
        };
        let unbound = |draw: &str, why: &str| bound(draw, StepProgram::Unbound { why: why.into() });
        assert!(
            StepBindings(vec![unbound("depth_normals", "not read")])
                .verify(material, &w)
                .is_ok()
        );
        let err = |b: StepBindings, w: &World| b.verify(material, w).unwrap_err();
        assert!(err(StepBindings(Vec::new()), &w).contains("no draw"));
        assert!(err(StepBindings(vec![unbound("a", "x"), unbound("a", "y")]), &w).contains("twice"));
        // one draw through two passes of the material, each named
        let through = |pass: Option<&str>| StepBinding {
            draw: "a".into(),
            pass: pass.map(str::to_owned),
            program: StepProgram::Unbound { why: "x".into() },
        };
        let two = StepBindings(vec![
            through(Some("DepthNormals")),
            through(Some("DepthNormalsOnly")),
        ]);
        assert!(two.verify(material, &w).is_ok());
        assert_eq!(two.of("a").count(), 2);
        assert!(
            err(
                StepBindings(vec![through(Some("DepthNormals")), through(None)]),
                &w
            )
            .contains("named pass")
        );
        assert!(
            err(
                StepBindings(vec![through(None), through(Some("DepthNormals"))]),
                &w
            )
            .contains("named pass")
        );
        assert!(err(StepBindings(vec![unbound("a", " ")]), &w).contains("not why"));
        assert!(err(StepBindings(vec![bound("a", StepProgram::Main)]), &w).contains("core.shader_binding"));
        w.insert(material, ShaderBinding::default());
        assert!(
            StepBindings(vec![bound("a", StepProgram::Main)])
                .verify(material, &w)
                .is_ok()
        );
        assert_eq!(
            StepBindings(vec![bound("a", StepProgram::Main)]).0[0].binding(material, &w),
            Some(&ShaderBinding::default())
        );
        // with a graph, only its scene steps' draws
        let stated = |placements| SceneMembers::Stated { placements };
        w.insert_resource(graph(&[
            ("forward", stated(ScenePlacements::All)),
            ("depth_normals", stated(ScenePlacements::Listed)),
            (
                "velocity",
                SceneMembers::NotRead {
                    why: "a fixture".into(),
                },
            ),
        ]));
        assert!(
            StepBindings(vec![bound("forward", StepProgram::Main)])
                .verify(material, &w)
                .is_ok()
        );
        assert!(err(StepBindings(vec![bound("a", StepProgram::Main)]), &w).contains("no scene step's"));
        // a draw whose surfaces the source does not read binds none
        assert!(err(StepBindings(vec![unbound("velocity", "x")]), &w).contains("does not read"));

        let placement = w.spawn();
        let draws = |d: &[&str]| StepDraws(d.iter().map(|s| (*s).to_owned()).collect());
        assert!(
            draws(&["depth_normals"])
                .verify(placement, &w)
                .unwrap_err()
                .contains("draws nothing")
        );
        let prototype = w.spawn();
        w.insert(
            placement,
            Instance {
                prototype,
                variant: 0,
            },
        );
        assert!(draws(&["depth_normals"]).verify(placement, &w).is_ok());
        assert!(draws(&["depth_normals"]).draws_in("depth_normals"));
        assert!(draws(&[]).verify(placement, &w).unwrap_err().contains("no draw"));
        assert!(
            draws(&["depth_normals", "depth_normals"])
                .verify(placement, &w)
                .unwrap_err()
                .contains("twice")
        );
        // a draw whose steps draw every placement is not listed per placement
        assert!(
            draws(&["forward"])
                .verify(placement, &w)
                .unwrap_err()
                .contains("lists its placements")
        );
    }
}
