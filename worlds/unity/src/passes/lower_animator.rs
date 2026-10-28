//! Unity Animator controllers lowered to `core.controller` and `core.controller_binding` (pass
//! `lower-unity-animator`): the state machine a controller states, the layers it runs and the motions its states choose,
//! for the rig each Animator drives.
//!
//! What is mapped, and from where:
//! - parameters: the stored kind (1 float, 3 int, 4 bool, 9 trigger, read from `m_ValueArray`) and its default in the
//!   kind's own array (`m_DefaultValues`), as [`crate::blend_tree::Parameters::defaults`] already reads them. The name
//!   is the parameter's CRC-32 entry in the controller's name table (`m_TOS`);
//! - layers: the state machine the layer runs, its `m_DefaultWeight` and its `m_LayerBlendingMode`. Unity's
//!   `AnimatorLayerBlendingMode` declares `Override` then `Additive`, so 0 and 1 are those; any other code stays
//!   `LayerBlending::Other(n)`, never guessed as an override. The layer's name is its binding hash's entry in the name
//!   table, or `layer {i}` where the table has none (the lab's name for it before this pass). A layer whose state
//!   machine has no states (PEAK's `Scout` and `Ambience` controllers have one: the entry node of its machine leads
//!   nowhere) is lowered as an empty layer, `default_state` `None`: it plays nothing, and the layers after it keep
//!   their indices;
//! - states: their motion, their `m_Speed` with the speed parameter they name, their `m_CycleOffset` with its
//!   parameter. A state name the table lacks is `state {i}`, as the lab named it;
//! - motion: a tree of one leaf is the `core.clip` that leaf's clip object is bound to on this rig (`core.clip_binding`
//!   with the rig: a clip that drives none of the rig's joints is not made, so such a leaf is named instead); a 1D tree
//!   (`blend_type` 0) is `Motion::Blend1D` with the node's blend parameter (a numeric parameter, its index) and its
//!   children's thresholds; every other tree (1 simple directional 2D, 2 freeform directional 2D, 3 freeform cartesian
//!   2D, 4 direct) is `Motion::Unsupported` naming its kind — as is a state whose time a parameter sets, which is
//!   mirrored, whose speed or cycle-offset parameter is not numeric, whose motion sets differ (a synchronised layer's
//!   overrides), whose 1D thresholds repeat (core's ascend strictly, Unity's need not), or whose blend tree contains
//!   itself.
//!
//! What stays in the dialect, as [`primase_core::Controller`] states it: transitions, any-state transitions, entry and
//! exit selectors, the body and skeleton masks, synchronised layers, tags, IK, looping and `m_WriteDefaultValues` are
//! facts of `unity.animator_controller`; the pass neither applies them nor drops them. The controller keeps its
//! `unity.animator_controller` and the rig its `unity.animator`.
//!
//! Placement: one `core.controller` per (controller, rig) pair, on an entity of its own — a controller shared by two
//! rigs states a clip entity per rig, so the same state's motion is not the same entity for both — and its
//! `core.controller_binding` on the rig (an Animator drives one rig), which needs the rig's `core.rig_instance`.
//! `m_Name` names the controller; a rig whose Animator is disabled or has no controller is not bound, each counted.
//!
//! A controller missing a fact core requires is not lowered at all, and is counted in the pass log instead of guessed: a
//! parameter without a name in the file's table (a parameter cannot be set by a name the file does not have), a parameter
//! kind other than the four, a default that is missing or not finite, two parameters of one name, a controller without a
//! name, a layer whose state machine is absent, a default state outside the states of a layer that has some, or a
//! weight, speed or cycle offset that is not finite.

use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{
    Clip, ClipBinding, Controller, ControllerBinding, ControllerLayer, ControllerParameter,
    ControllerParameterKind, ControllerState, Entity, LayerBlending, Motion, RigInstance, World,
};

use crate::animator::{UnityAnimator, UnityAnimatorController};
use crate::components::UnityObject;
use unity_formats::Result;
use unity_formats::animator::{BlendNode, Parameter, State};

/// The kind of a stored parameter (1 float, 3 int, 4 bool, 9 trigger) as core's; `None` for a kind core has no word
/// for.
fn parameter_kind(kind: i64) -> Option<ControllerParameterKind> {
    match kind {
        1 => Some(ControllerParameterKind::Float),
        3 => Some(ControllerParameterKind::Int),
        4 => Some(ControllerParameterKind::Bool),
        9 => Some(ControllerParameterKind::Trigger),
        _ => None,
    }
}

/// One stored parameter as core's: its name from the controller's table, its kind, and its default from the kind's own
/// array. `Err` names what a controller is not lowered for.
fn parameter(
    controller: &UnityAnimatorController,
    p: &Parameter,
) -> std::result::Result<ControllerParameter, String> {
    let kind = parameter_kind(p.kind)
        .ok_or_else(|| format!("controllers not lowered: a parameter of kind {}", p.kind))?;
    let name = controller
        .name_of(p.id)
        .ok_or("controllers not lowered: a parameter's name is not in the file's name table")?;
    let at = usize::try_from(p.index)
        .map_err(|_| "controllers not lowered: a parameter at a negative default index".to_owned())?;
    let default = match p.kind {
        1 => controller
            .default_floats
            .get(at)
            .map(|&f| f64::from(f))
            .ok_or("controllers not lowered: a float parameter's default is missing")?,
        3 => controller
            .default_ints
            .get(at)
            .map(|&i| i as f64)
            .ok_or("controllers not lowered: an int parameter's default is missing")?,
        4 | 9 => controller
            .default_bools
            .get(at)
            .map(|&b| f64::from(u8::from(b)))
            .ok_or("controllers not lowered: a bool or trigger parameter's default is missing")?,
        _ => unreachable!("the kind is one of the four"),
    };
    if !default.is_finite() {
        return Err("controllers not lowered: a parameter default is not finite".to_owned());
    }
    Ok(ControllerParameter {
        name: name.to_owned(),
        kind,
        default,
    })
}

/// The index in `controller.parameters` of the numeric parameter whose name hashes to `id`: what `core.controller`
/// states by position. `None` for an id the controller has no numeric parameter for (0: none, as Unity stores it).
fn numeric_parameter(controller: &UnityAnimatorController, id: u32) -> Option<usize> {
    controller.parameters.iter().position(|p| {
        p.id == id
            && matches!(
                parameter_kind(p.kind),
                Some(ControllerParameterKind::Float | ControllerParameterKind::Int)
            )
    })
}

/// The `core.clip` leaf `index` of `controller` names, as the one bound to `rig`: `Err` says why the leaf has none (a
/// named refusal, never another clip of the same object on another rig).
fn clip_of(
    controller: &UnityAnimatorController,
    index: u32,
    rig: Entity,
    w: &World,
) -> std::result::Result<Entity, String> {
    let Some(object) = usize::try_from(index)
        .ok()
        .and_then(|i| controller.clips.get(i))
        .ok_or_else(|| format!("it names clip {index} of {}", controller.clips.len()))?
        .as_ref()
    else {
        return Err(format!("its clip slot {index} is empty"));
    };
    w.iter::<ClipBinding>()
        .filter(|(_, b)| b.rig == Some(rig))
        .map(|(_, b)| b.clip)
        .find(|clip| w.has::<Clip>(*clip) && w.get::<UnityObject>(*clip).map(|o| &o.0) == Some(object))
        .ok_or_else(|| {
            format!(
                "the rig has no clip {}/{} bound (a clip that drives none of its joints is not made)",
                object.file, object.path_id
            )
        })
}

/// What a blend tree's kind is, in the words [`crate::blend_tree`] refuses it by.
fn blend_tree_kind(kind: i64) -> String {
    match kind {
        1 => "a simple directional 2D blend tree".to_owned(),
        2 => "a freeform directional 2D blend tree".to_owned(),
        3 => "a freeform cartesian 2D blend tree".to_owned(),
        4 => "a direct blend tree".to_owned(),
        k => format!("a blend tree of type {k}"),
    }
}

/// Blend-tree node `at` of `tree` as core's motion, for `rig`. A node core has no form for is `Motion::Unsupported` with
/// its reason; nothing is replaced by one of its children.
fn motion(
    tree: &[BlendNode],
    at: usize,
    controller: &UnityAnimatorController,
    rig: Entity,
    w: &World,
    depth: usize,
) -> Motion {
    if depth > tree.len() {
        return Motion::Unsupported("a blend tree that contains itself".to_owned());
    }
    let Some(node) = tree.get(at) else {
        return Motion::Unsupported(format!("blend tree node {at} of {}", tree.len()));
    };
    if node.mirror {
        return Motion::Unsupported("a mirrored blend tree node".to_owned());
    }
    if node.children.is_empty() {
        return match clip_of(controller, node.clip, rig, w) {
            Ok(clip) => Motion::Clip(clip),
            Err(why) => Motion::Unsupported(why),
        };
    }
    if node.blend_type != 0 {
        return Motion::Unsupported(blend_tree_kind(node.blend_type));
    }
    let thresholds = &node.thresholds;
    if thresholds.len() != node.children.len() {
        return Motion::Unsupported(format!(
            "a 1D blend tree of {} children with {} thresholds",
            node.children.len(),
            thresholds.len()
        ));
    }
    if !thresholds.iter().all(|t| t.is_finite()) || !thresholds.windows(2).all(|pair| pair[0] < pair[1]) {
        return Motion::Unsupported("a 1D blend tree whose thresholds do not ascend strictly".to_owned());
    }
    let Some(parameter) = numeric_parameter(controller, node.event_id) else {
        return Motion::Unsupported(format!(
            "a 1D blend tree whose parameter {} is not a numeric controller parameter",
            node.event_id
        ));
    };
    let children = node
        .children
        .iter()
        .zip(thresholds.iter().copied())
        .map(|(&child, threshold)| {
            (
                threshold,
                motion(tree, child as usize, controller, rig, w, depth + 1),
            )
        })
        .collect();
    Motion::Blend1D { parameter, children }
}

/// The motion a state plays: its one motion set's tree, or a named refusal.
fn state_motion(s: &State, controller: &UnityAnimatorController, rig: Entity, w: &World) -> Motion {
    match s.blend_tree_index.as_slice() {
        [index] => match usize::try_from(*index).ok().and_then(|i| s.blend_trees.get(i)) {
            Some(tree) if !tree.is_empty() => motion(tree, 0, controller, rig, w, 0),
            Some(_) => Motion::Unsupported("a blend tree of no nodes".to_owned()),
            None => Motion::Unsupported(format!("its motion set {index} of {}", s.blend_trees.len())),
        },
        sets => Motion::Unsupported(format!(
            "{} motion sets (a synchronised layer's overrides)",
            sets.len()
        )),
    }
}

/// One stored state as core's, for `rig`.
fn state(
    controller: &UnityAnimatorController,
    s: &State,
    at: usize,
    rig: Entity,
    w: &World,
) -> std::result::Result<ControllerState, String> {
    if !s.speed.is_finite() || !s.cycle_offset.is_finite() {
        return Err("controllers not lowered: a state's speed or cycle offset is not finite".to_owned());
    }
    // the state's time rule where core can state it: Unity's own parameters, 0 for none
    let speed_parameter = match s.speed_param {
        0 => Ok(None),
        id => numeric_parameter(controller, id)
            .map(Some)
            .ok_or("its speed parameter is not a numeric controller parameter"),
    };
    let cycle_offset_parameter = match s.cycle_offset_param {
        0 => Ok(None),
        id => numeric_parameter(controller, id)
            .map(Some)
            .ok_or("its cycle offset parameter is not a numeric controller parameter"),
    };
    let refused = speed_parameter
        .as_ref()
        .err()
        .or(cycle_offset_parameter.as_ref().err())
        .copied();
    let motion = if s.time_param != 0 {
        Motion::Unsupported("its time is set by a parameter".to_owned())
    } else if s.mirror || s.mirror_param != 0 {
        Motion::Unsupported("it is mirrored".to_owned())
    } else if let Some(why) = refused {
        Motion::Unsupported(why.to_owned())
    } else {
        state_motion(s, controller, rig, w)
    };
    Ok(ControllerState {
        name: controller
            .name_of(s.name_id)
            .map_or_else(|| format!("state {at}"), str::to_owned),
        motion,
        speed: f64::from(s.speed),
        speed_parameter: speed_parameter.ok().flatten(),
        cycle_offset: s.cycle_offset,
        cycle_offset_parameter: cycle_offset_parameter.ok().flatten(),
    })
}

/// The controller `controller` states for `rig`: `None` when a fact [`Controller`] requires is not in the file, counted
/// in `log` with what was missing.
fn controller_for(
    controller: &UnityAnimatorController,
    rig: Entity,
    w: &World,
    log: &mut PassLog,
) -> Option<Controller> {
    if controller.name.trim().is_empty() {
        log.omit("controllers not lowered: they have no name", 1);
        return None;
    }
    let mut parameters = Vec::with_capacity(controller.parameters.len());
    for p in &controller.parameters {
        match parameter(controller, p) {
            Ok(parameter) => parameters.push(parameter),
            Err(why) => {
                log.omit(&why, 1);
                return None;
            }
        }
    }
    for (i, p) in parameters.iter().enumerate() {
        if parameters.iter().take(i).any(|q| q.name == p.name) {
            log.omit("controllers not lowered: two parameters share a name", 1);
            return None;
        }
    }
    let mut layers = Vec::with_capacity(controller.layers.len());
    for (index, layer) in controller.layers.iter().enumerate() {
        let Some(machine) = usize::try_from(layer.state_machine)
            .ok()
            .and_then(|m| controller.state_machines.get(m))
        else {
            log.omit(
                "controllers not lowered: a layer runs a state machine the file does not have",
                1,
            );
            return None;
        };
        // a machine with no states has no state to start in: its layer plays nothing, and its stored default state (0)
        // names no state
        let default_state = if machine.states.is_empty() {
            None
        } else {
            let Some(default_state) = usize::try_from(machine.default_state)
                .ok()
                .filter(|d| *d < machine.states.len())
            else {
                log.omit(
                    "controllers not lowered: a layer's default state is outside its states",
                    1,
                );
                return None;
            };
            Some(default_state)
        };
        if !layer.default_weight.is_finite() {
            log.omit(
                "controllers not lowered: a layer's default weight is not finite",
                1,
            );
            return None;
        }
        let mut states = Vec::with_capacity(machine.states.len());
        for (at, s) in machine.states.iter().enumerate() {
            match state(controller, s, at, rig, w) {
                Ok(state) => states.push(state),
                Err(why) => {
                    log.omit(&why, 1);
                    return None;
                }
            }
        }
        layers.push(ControllerLayer {
            name: controller
                .name_of(layer.binding)
                .map_or_else(|| format!("layer {index}"), str::to_owned),
            states,
            default_state,
            default_weight: layer.default_weight,
            blending: match layer.blending_mode {
                0 => LayerBlending::Override,
                1 => LayerBlending::Additive,
                other => LayerBlending::Other(other),
            },
        });
    }
    Some(Controller {
        name: controller.name.clone(),
        parameters,
        layers,
    })
}

/// Lower every Animator's controller (see the module documentation): one `core.controller` per (controller, rig) pair,
/// and its `core.controller_binding` on the rig.
pub fn lower_unity_animator(w: &mut World) -> Result<PassLog> {
    let mut log = PassLog::default();
    let animators: Vec<(Entity, UnityAnimator)> =
        w.iter::<UnityAnimator>().map(|(e, a)| (e, a.clone())).collect();
    log.count("Animators", animators.len() as u64);
    for (rig, animator) in animators {
        if !animator.enabled {
            log.count("disabled Animators (not bound)", 1);
            continue;
        }
        let Some(controller_entity) = animator.controller else {
            log.count(
                "Animators without a controller (clips chosen at run time, not bound)",
                1,
            );
            continue;
        };
        let Some(controller) = w.get::<UnityAnimatorController>(controller_entity) else {
            log.omit(
                "Animators whose controller holds no unity.animator_controller (not bound)",
                1,
            );
            continue;
        };
        if !w.has::<RigInstance>(rig) {
            log.omit(
                "Animators on an entity that is no rig (no core.rig_instance; not bound)",
                1,
            );
            continue;
        }
        let Some(lowered) = controller_for(controller, rig, w, &mut log) else {
            continue;
        };
        // a read that lowered this rig before keeps its controller entity: a second run replaces the value in place
        let existing = w
            .get::<ControllerBinding>(rig)
            .map(|binding| binding.controller)
            .filter(|controller| w.has::<Controller>(*controller));
        let entity = existing.unwrap_or_else(|| w.spawn());
        w.insert(entity, lowered);
        let binding = ControllerBinding {
            controller: entity,
            rig,
        };
        let already = w.get::<ControllerBinding>(rig) == Some(&binding);
        if !already {
            w.insert(rig, binding);
        }
        log.count("controllers lowered", 1);
        log.count("controller bindings", 1);
    }
    let mut layers = 0;
    let mut states = 0;
    let mut unsupported = 0;
    for (_, controller) in w.iter::<Controller>() {
        layers += controller.layers.len() as u64;
        for layer in &controller.layers {
            states += layer.states.len() as u64;
            unsupported += layer
                .states
                .iter()
                .filter(|s| matches!(s.motion, Motion::Unsupported(_)))
                .count() as u64;
        }
    }
    log.count("layers", layers);
    log.count("states", states);
    log.count("states whose motion is Unsupported (named)", unsupported);
    Ok(log)
}

/// [`lower_unity_animator`] as a pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LowerUnityAnimator;

/// `lower-unity-animator`'s declaration ([`primase_core::pass::PassRegistry::add`]): no options, it lowers by Unity's own
/// rules.
pub const LOWER_UNITY_ANIMATOR: PassSpec = PassSpec {
    name: "lower-unity-animator",
    options: &[],
    make: |_| Ok(Box::new(LowerUnityAnimator)),
};

impl Pass for LowerUnityAnimator {
    fn name(&self) -> &'static str {
        "lower-unity-animator"
    }
    /// An Animator is what states that a controller drives a rig, so it is what this needs; a world without one has
    /// nothing to bind and the pass is left out of its pipeline.
    fn requires(&self) -> &[&'static str] {
        &["unity.animator"]
    }
    /// A `core.controller` per (controller, rig) pair, and its `core.controller_binding` on the rig.
    fn provides(&self) -> &[&'static str] {
        &["core.controller", "core.controller_binding"]
    }
    /// It lowers Unity Animators and keeps them, and the controllers they name as stored.
    fn legalizes(&self) -> &[&'static str] {
        &["unity.animator"]
    }
    /// A second run replaces the `core.controller` it wrote on its own entity in place.
    fn modifies(&self) -> &[&'static str] {
        &["core.controller"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let r = lower_unity_animator(w).map_err(|e| e.to_string())?;
        log.absorb(r);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use primase_core::pass::{Options, PassRegistry};
    use primase_core::{Component, Extrapolation, Skeleton, Transform};

    use super::*;
    use crate::components::Stored;
    use unity_formats::animator::{Layer, StateMachine};
    use unity_formats::player::ObjRef;

    const SPEED: u32 = 7;
    const OFFSET: u32 = 11;
    const FLAG: u32 = 13;
    const BASE: u32 = 21;
    const IDLE: u32 = 31;
    const WALK: u32 = 32;
    const FLY: u32 = 33;

    fn object(path_id: i64) -> ObjRef {
        ObjRef {
            file: "fixture.assets".to_owned(),
            path_id,
        }
    }

    fn node(blend_type: i64, event: u32, children: Vec<u32>, thresholds: Vec<f32>, clip: u32) -> BlendNode {
        BlendNode {
            blend_type,
            event_id: event,
            event_y_id: u32::MAX,
            children,
            thresholds,
            positions: vec![],
            magnitudes: vec![],
            pair_vectors: vec![],
            pair_avg_mag_inv: vec![],
            neighbors: vec![],
            direct_events: vec![],
            normalized_blend_values: false,
            clip,
            duration: 1.0,
            cycle_offset: 0.0,
            mirror: false,
        }
    }

    /// A blend tree leaf: no children, the clip of slot `clip`.
    fn leaf(clip: u32) -> BlendNode {
        node(0, 0, vec![], vec![], clip)
    }

    fn state(name_id: u32, tree: Vec<BlendNode>, speed: f32) -> State {
        State {
            transitions: vec![],
            blend_tree_index: vec![0],
            blend_trees: vec![tree],
            name_id,
            path_id: name_id,
            full_path_id: name_id,
            tag_id: 0,
            speed_param: 0,
            mirror_param: 0,
            cycle_offset_param: 0,
            time_param: 0,
            speed,
            cycle_offset: 0.25,
            ik_on_feet: false,
            write_default_values: true,
            looping: true,
            mirror: false,
        }
    }

    fn layer(machine: i64, name: u32, blending: i64, weight: f32) -> Layer {
        Layer {
            state_machine: machine,
            synchronized_layer: -1,
            body_mask: [0; 3],
            skeleton_mask: vec![],
            binding: name,
            blending_mode: blending,
            default_weight: weight,
            ik_pass: false,
            synced_layer_affects_timing: false,
        }
    }

    fn controller(layers: Vec<Layer>, machines: Vec<StateMachine>) -> UnityAnimatorController {
        Stored(unity_formats::animator::UnityAnimatorController {
            controller: object(1),
            name: "bird".to_owned(),
            layers,
            state_machines: machines,
            parameters: vec![
                Parameter {
                    id: SPEED,
                    kind: 1,
                    index: 0,
                },
                Parameter {
                    id: OFFSET,
                    kind: 3,
                    index: 0,
                },
                Parameter {
                    id: FLAG,
                    kind: 4,
                    index: 0,
                },
            ],
            default_floats: vec![0.5],
            default_ints: vec![3],
            default_bools: vec![true],
            clips: vec![Some(object(100)), Some(object(101)), None],
            behaviours: vec![],
            tos: vec![
                (SPEED, "speed".to_owned()),
                (OFFSET, "offset".to_owned()),
                (FLAG, "flag".to_owned()),
                (21, "base".to_owned()),
                (31, "Idle".to_owned()),
                (32, "Walk".to_owned()),
                (33, "Fly".to_owned()),
            ],
        })
    }

    /// A world with the fixture's controller, one rig its Animator drives, and its two clips bound to that rig.
    fn world() -> (World, Entity, Entity) {
        let mut w = World::new();
        let skeleton = w.spawn();
        w.insert(skeleton, Skeleton { joints: vec![] });
        let rig = w.spawn();
        w.insert(rig, Transform::IDENTITY);
        w.insert(rig, RigInstance { skeleton });
        let controller_entity = w.spawn();
        let machine = StateMachine {
            default_state: 1,
            any_state_transitions: vec![],
            selectors: vec![],
            synchronized_layer_count: 0,
            states: vec![
                // one clip
                state(IDLE, vec![leaf(0)], 1.0),
                // a 1D tree of two clips, by the speed parameter
                state(
                    WALK,
                    vec![
                        node(0, SPEED, vec![1, 2], vec![-1.0, 1.0], u32::MAX),
                        leaf(0),
                        leaf(1),
                    ],
                    1.5,
                ),
                // a 2D tree, which core has no motion for
                state(FLY, vec![node(3, 0, vec![1], vec![], u32::MAX), leaf(0)], 1.0),
            ],
        };
        w.insert(
            controller_entity,
            controller(vec![layer(0, BASE, 0, 0.75)], vec![machine]),
        );
        w.insert(
            rig,
            UnityAnimator {
                animator: Stored(object(2)),
                enabled: true,
                apply_root_motion: false,
                controller: Some(controller_entity),
                avatar: None,
            },
        );
        for (path_id, name) in [(100, "idle"), (101, "walk")] {
            let clip = w.spawn();
            w.insert(
                clip,
                Clip {
                    name: name.to_owned(),
                    duration: 1.0,
                    extrapolation: Extrapolation::Hold,
                    channels: vec![],
                    root_motion: None,
                    additive: None,
                },
            );
            w.insert(clip, Stored(object(path_id)));
            w.insert(
                clip,
                ClipBinding {
                    clip,
                    rig: Some(rig),
                    meshes: vec![],
                },
            );
        }
        (w, rig, controller_entity)
    }

    #[test]
    fn the_controller_is_lowered_with_its_binding() {
        let (mut w, rig, _) = world();
        let log = lower_unity_animator(&mut w).unwrap();
        let (entity, value) = w.iter::<Controller>().next().expect("one controller");
        assert_eq!(value.name, "bird");
        assert_eq!(value.parameters.len(), 3);
        assert_eq!(value.parameters[0].name, "speed");
        assert_eq!(value.parameters[0].kind, ControllerParameterKind::Float);
        assert_eq!(value.parameters[0].default, 0.5);
        assert_eq!(value.parameters[1].kind, ControllerParameterKind::Int);
        assert_eq!(value.parameters[1].default, 3.0);
        assert_eq!(value.parameters[2].kind, ControllerParameterKind::Bool);
        assert_eq!(value.parameters[2].default, 1.0);
        let layer = &value.layers[0];
        assert_eq!(layer.name, "base");
        assert_eq!(layer.default_state, Some(1));
        assert_eq!(layer.default_weight, 0.75);
        assert_eq!(layer.blending, LayerBlending::Override);
        assert_eq!(layer.states[0].name, "Idle");
        assert_eq!(layer.states[0].speed, 1.0);
        assert_eq!(layer.states[0].cycle_offset, 0.25);
        assert_eq!(layer.states[0].speed_parameter, None);
        let clips: Vec<Entity> = w.iter::<Clip>().map(|(e, _)| e).collect();
        match &layer.states[0].motion {
            Motion::Clip(clip) => assert!(clips.contains(clip)),
            other => panic!("{other:?}"),
        }
        match &layer.states[1].motion {
            Motion::Blend1D { parameter, children } => {
                assert_eq!(*parameter, 0);
                assert_eq!(children.len(), 2);
                assert_eq!(children[0].0, -1.0);
                assert_eq!(children[1].0, 1.0);
                assert!(matches!(children[0].1, Motion::Clip(_)));
                assert!(matches!(children[1].1, Motion::Clip(_)));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            layer.states[2].motion,
            Motion::Unsupported("a freeform cartesian 2D blend tree".to_owned())
        );
        assert_eq!(
            w.get::<ControllerBinding>(rig),
            Some(&ControllerBinding {
                controller: entity,
                rig
            })
        );
        // the components core verifies are valid as written
        w.get::<Controller>(entity).unwrap().verify(entity, &w).unwrap();
        w.get::<ControllerBinding>(rig).unwrap().verify(rig, &w).unwrap();
        assert_eq!(log.counts["states"], 3);
        assert_eq!(log.counts["states whose motion is Unsupported (named)"], 1);
        assert_eq!(log.counts["controller bindings"], 1);
    }

    /// A layer whose machine has no states lowers as an empty layer (no default state, no states) beside the others, which
    /// keep their indices; a default state outside a machine that has states still drops the controller.
    #[test]
    fn an_empty_layer_is_lowered_and_a_stray_default_state_is_not() {
        let (mut w, _, controller_entity) = world();
        let controller = w.get_mut::<UnityAnimatorController>(controller_entity).unwrap();
        // the machine PEAK's Scout and Ambience run in their empty layer: no states, a default state of 0
        controller.state_machines.push(StateMachine {
            default_state: 0,
            any_state_transitions: vec![],
            selectors: vec![],
            synchronized_layer_count: 0,
            states: vec![],
        });
        controller.layers = vec![
            layer(0, BASE, 0, 0.75),
            layer(1, 999, 0, 1.0),
            layer(0, 998, 1, 0.5),
        ];
        let log = lower_unity_animator(&mut w).unwrap();
        let (entity, value) = w.iter::<Controller>().next().expect("lowered");
        assert_eq!(value.layers.len(), 3);
        assert_eq!(value.layers[0].default_state, Some(1));
        assert_eq!(value.layers[1].name, "layer 1");
        assert!(value.layers[1].states.is_empty());
        assert_eq!(value.layers[1].default_state, None);
        assert_eq!(value.layers[1].default_weight, 1.0);
        assert_eq!(value.layers[2].default_state, Some(1));
        w.get::<Controller>(entity).unwrap().verify(entity, &w).unwrap();
        assert_eq!(log.counts["controllers lowered"], 1);
        assert_eq!(log.counts["layers"], 3);
        assert_eq!(log.counts["states"], 6);

        // a machine that has states and a default state outside them is still not lowered
        let (mut w, _, controller_entity) = world();
        w.get_mut::<UnityAnimatorController>(controller_entity)
            .unwrap()
            .state_machines[0]
            .default_state = 3;
        let log = lower_unity_animator(&mut w).unwrap();
        assert_eq!(w.iter::<Controller>().count(), 0);
        assert_eq!(
            log.omitted["controllers not lowered: a layer's default state is outside its states"],
            1
        );
    }

    #[test]
    fn a_second_run_replaces_the_controller_in_place() {
        let (mut w, rig, _) = world();
        lower_unity_animator(&mut w).unwrap();
        let first = w.iter::<Controller>().next().unwrap().0;
        lower_unity_animator(&mut w).unwrap();
        assert_eq!(w.iter::<Controller>().count(), 1);
        assert_eq!(w.get::<ControllerBinding>(rig).unwrap().controller, first);
    }

    #[test]
    fn state_times_parameters_and_mirrors_are_named_refusals() {
        let (mut w, _, controller_entity) = world();
        let controller = w.get_mut::<UnityAnimatorController>(controller_entity).unwrap();
        controller.state_machines[0].states[0].speed_param = FLAG; // a bool, not numeric
        controller.state_machines[0].states[1].cycle_offset_param = OFFSET;
        controller.state_machines[0].states[2].mirror = true;
        lower_unity_animator(&mut w).unwrap();
        let (_, value) = w.iter::<Controller>().next().unwrap();
        assert_eq!(
            value.layers[0].states[0].motion,
            Motion::Unsupported("its speed parameter is not a numeric controller parameter".to_owned())
        );
        assert_eq!(value.layers[0].states[1].cycle_offset_parameter, Some(1));
        assert_eq!(
            value.layers[0].states[2].motion,
            Motion::Unsupported("it is mirrored".to_owned())
        );
    }

    #[test]
    fn a_leaf_whose_clip_is_not_bound_is_named() {
        let (mut w, _, _) = world();
        // the first clip is not bound to the rig any more
        let clip = w.iter::<ClipBinding>().map(|(e, _)| e).next().expect("a binding");
        w.get_mut::<ClipBinding>(clip).unwrap().rig = None;
        lower_unity_animator(&mut w).unwrap();
        let (_, value) = w.iter::<Controller>().next().unwrap();
        match &value.layers[0].states[1].motion {
            Motion::Blend1D { children, .. } => assert!(
                children.iter().any(|(_, m)| matches!(m,
                Motion::Unsupported(why) if why.contains("the rig has no clip"))),
                "{children:?}"
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_controller_core_cannot_state_is_not_lowered() {
        let (mut w, _, controller_entity) = world();
        // a parameter whose name the file's table lacks
        w.get_mut::<UnityAnimatorController>(controller_entity)
            .unwrap()
            .tos
            .retain(|(hash, _)| *hash != OFFSET);
        let log = lower_unity_animator(&mut w).unwrap();
        assert_eq!(w.iter::<Controller>().count(), 0);
        assert_eq!(w.iter::<ControllerBinding>().count(), 0);
        assert_eq!(
            log.omitted["controllers not lowered: a parameter's name is not in the file's name table"],
            1
        );
    }

    #[test]
    fn a_disabled_animator_is_not_bound() {
        let (mut w, rig, _) = world();
        w.get_mut::<UnityAnimator>(rig).unwrap().enabled = false;
        let log = lower_unity_animator(&mut w).unwrap();
        assert_eq!(w.iter::<Controller>().count(), 0);
        assert_eq!(log.counts["disabled Animators (not bound)"], 1);
    }

    #[test]
    fn a_layer_keeps_its_blending_code_and_a_missing_name_its_placeholder() {
        let (mut w, _, controller_entity) = world();
        let controller = w.get_mut::<UnityAnimatorController>(controller_entity).unwrap();
        // two layers over the same machine: additive (1) and a code core has no word for, both named by no entry
        controller.layers = vec![layer(0, 999, 1, 0.5), layer(0, 998, 7, 0.25)];
        // a state whose name the table lacks
        controller.state_machines[0].states[2].name_id = 997;
        lower_unity_animator(&mut w).unwrap();
        let (_, value) = w.iter::<Controller>().next().unwrap();
        assert_eq!(value.layers.len(), 2);
        assert_eq!(value.layers[0].name, "layer 0");
        assert_eq!(value.layers[0].blending, LayerBlending::Additive);
        assert_eq!(value.layers[1].name, "layer 1");
        assert_eq!(value.layers[1].blending, LayerBlending::Other(7));
        assert_eq!(value.layers[0].states[2].name, "state 2");
        // the lab named a state the same way before this pass
        assert_eq!(value.layers[0].states[0].name, "Idle");
    }

    #[test]
    fn the_pass_registers_and_takes_no_options() {
        let mut registry = PassRegistry::new();
        registry.add(&LOWER_UNITY_ANIMATOR);
        assert!(registry.describe("lower-unity-animator").is_some());
        assert!(!registry.requires_options("lower-unity-animator"));
        assert!(registry.make("lower-unity-animator", &Options::new()).is_ok());
    }
}
