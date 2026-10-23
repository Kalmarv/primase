//! Unity's Animator side as typed data: an Avatar (the skeleton Mecanim animates, and for a humanoid the compiled
//! muscle axes and the HumanDescription it was built from), an AnimatorController (layers, state machines, states with
//! their blend trees and transitions, parameters) and the Animator that pairs them with a rig. Nothing here is
//! evaluated: it is what the files hold, in Unity's frame (rotations as `[x, y, z, w]`), with the names Unity keeps
//! only as CRC-32 hashes resolved through the object's own name table (`tos`) where it has one.

use primase_core::{Component, Entity, JsonError};
use serde::Serialize;

use crate::components::UnityObject;

/// A transform as Mecanim stores it (`xform`): translation, rotation as a quaternion `[x, y, z, w]` and scale.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Xform {
    pub t: [f32; 3],
    pub q: [f32; 4],
    pub s: [f32; 3],
}

/// One bone's muscle axes (`Axes`): its pre- and post-rotation, the sign of each axis, the limits of its rotation in
/// radians, its length and its type.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct AvatarAxes {
    pub pre_q: [f32; 4],
    pub post_q: [f32; 4],
    pub sgn: [f32; 3],
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub length: f32,
    pub kind: i64,
}

/// A skeleton (`Skeleton`): per node its parent (`-1` for a root) and its axes index (`-1` for none), the CRC-32 of
/// each node's path, and the axes the nodes index.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct AvatarSkeleton {
    pub nodes: Vec<(i64, i64)>,
    pub ids: Vec<u32>,
    pub axes: Vec<AvatarAxes>,
}

/// A humanoid's compiled data (`Human`): the human skeleton with its pose and axes, which skeleton bone each human bone
/// is (`bone_index`, `-1` for none), the proportions the retargeting uses.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AvatarHuman {
    pub root: Xform,
    pub skeleton: AvatarSkeleton,
    pub pose: Vec<Xform>,
    pub left_hand: Vec<i64>,
    pub right_hand: Vec<i64>,
    pub bone_index: Vec<i64>,
    pub bone_mass: Vec<f32>,
    pub scale: f32,
    pub arm_twist: f32,
    pub fore_arm_twist: f32,
    pub upper_leg_twist: f32,
    pub leg_twist: f32,
    pub arm_stretch: f32,
    pub leg_stretch: f32,
    pub feet_spacing: f32,
    pub has_left_hand: bool,
    pub has_right_hand: bool,
    pub has_translation_dof: bool,
}

/// One human bone of the HumanDescription: the skeleton bone it names, the human bone it is and its stated limits.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HumanBone {
    pub bone_name: String,
    pub human_name: String,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub value: [f32; 3],
    pub length: f32,
    pub modified: bool,
}

/// One bone of the HumanDescription's skeleton: its name, its parent's, and its local transform.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SkeletonBone {
    pub name: String,
    pub parent: String,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

/// What the avatar was built from (`HumanDescription`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HumanDescription {
    pub human: Vec<HumanBone>,
    pub skeleton: Vec<SkeletonBone>,
    pub arm_twist: f32,
    pub fore_arm_twist: f32,
    pub upper_leg_twist: f32,
    pub leg_twist: f32,
    pub arm_stretch: f32,
    pub leg_stretch: f32,
    pub feet_spacing: f32,
    pub global_scale: f32,
    pub root_motion_bone_name: String,
    pub has_translation_dof: bool,
    pub has_extra_root: bool,
    pub skeleton_has_parents: bool,
}

/// `unity.avatar`: an Avatar object as stored, on an entity of its own that the Animators using it name.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityAvatar {
    pub avatar: UnityObject,
    pub name: String,
    pub skeleton: AvatarSkeleton,
    /// The skeleton's pose (`m_AvatarSkeletonPose`) and its default pose (`m_DefaultPose`), a transform a node.
    pub skeleton_pose: Vec<Xform>,
    pub default_pose: Vec<Xform>,
    pub skeleton_name_ids: Vec<u32>,
    /// A humanoid's data; `None` for a generic avatar (its human skeleton is empty).
    pub human: Option<AvatarHuman>,
    /// Which avatar skeleton node each human skeleton node is (`m_HumanSkeletonIndexArray`) and the reverse.
    pub human_skeleton_index: Vec<i64>,
    pub human_skeleton_reverse: Vec<i64>,
    pub root_motion_bone_index: i64,
    pub root_motion_bone: Xform,
    pub root_motion_skeleton: AvatarSkeleton,
    /// Empty when the avatar stores no pose for its root motion nodes.
    pub root_motion_pose: Vec<Xform>,
    pub root_motion_skeleton_index: Vec<i64>,
    /// The CRC-32 of a path and the path (`m_TOS`).
    pub tos: Vec<(u32, String)>,
    pub description: HumanDescription,
}

impl Component for UnityAvatar {
    const NAME: &'static str = "unity.avatar";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        // the counts, not the poses and tables
        Ok(Some(serde_json::json!({
            "avatar": self.avatar,
            "name": self.name,
            "nodes": self.skeleton.nodes.len(),
            "humanoid": self.human.is_some(),
            "human_bones": self.human.as_ref().map(|h| h.bone_index.iter().filter(|i| **i >= 0).count()),
            "described_bones": self.description.skeleton.len(),
            "tos": self.tos.len(),
        })))
    }
}

/// A transition condition (`ConditionConstant`): its mode (1 true, 2 false, 3 greater, 4 less, 6 equals, 7 not equal,
/// as Unity numbers them), the CRC-32 of its parameter's name, its threshold and, for an exit-time condition, the time.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Condition {
    pub mode: i64,
    pub parameter: u32,
    pub threshold: f32,
    pub exit_time: f32,
}

/// What a destination at or above this is: a transition's `destination` (and a selector transition's) below it names a
/// state of its machine, `SELECTOR_BASE + n` the selector node `n` (an entry or an exit).
pub const SELECTOR_BASE: i64 = 30000;

/// A transition (`TransitionConstant`) of a state or from any state.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Transition {
    pub conditions: Vec<Condition>,
    /// A state, or [`SELECTOR_BASE`] plus a selector node.
    pub destination: i64,
    pub full_path_id: u32,
    pub id: u32,
    pub user_id: u32,
    pub duration: f32,
    pub offset: f32,
    pub exit_time: f32,
    pub has_exit_time: bool,
    pub has_fixed_duration: bool,
    pub interruption_source: i64,
    pub ordered_interruption: bool,
    pub can_transition_to_self: bool,
}

/// One node of a blend tree (`BlendTreeNodeConstant`): its kind (0 1D, 1 simple 2D, 2 freeform directional, 3 freeform
/// cartesian, 4 direct, as Unity numbers them), the parameters it blends by, its children and thresholds or positions,
/// and for a leaf the clip (`clip`: an index of the controller's clips, `u32::MAX` for none).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BlendNode {
    pub blend_type: i64,
    pub event_id: u32,
    pub event_y_id: u32,
    pub children: Vec<u32>,
    pub thresholds: Vec<f32>,
    pub positions: Vec<[f32; 2]>,
    pub magnitudes: Vec<f32>,
    pub pair_vectors: Vec<[f32; 2]>,
    pub pair_avg_mag_inv: Vec<f32>,
    /// `m_ChildNeighborListArray`: per child the indices of its neighbours in the 2D tree.
    pub neighbors: Vec<Vec<u32>>,
    pub direct_events: Vec<u32>,
    pub normalized_blend_values: bool,
    pub clip: u32,
    pub duration: f32,
    pub cycle_offset: f32,
    pub mirror: bool,
}

/// A state (`StateConstant`): its transitions, its blend trees (one per motion set; a plain clip is a tree of one
/// leaf), the CRC-32 of its name and path and the parameters that drive its speed, mirror, offset and time.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct State {
    pub transitions: Vec<Transition>,
    pub blend_tree_index: Vec<i64>,
    pub blend_trees: Vec<Vec<BlendNode>>,
    pub name_id: u32,
    pub path_id: u32,
    pub full_path_id: u32,
    pub tag_id: u32,
    pub speed_param: u32,
    pub mirror_param: u32,
    pub cycle_offset_param: u32,
    pub time_param: u32,
    pub speed: f32,
    pub cycle_offset: f32,
    pub ik_on_feet: bool,
    pub write_default_values: bool,
    pub looping: bool,
    pub mirror: bool,
}

/// A transition out of an entry or exit selector (`SelectorTransitionConstant`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SelectorTransition {
    /// A state, or [`SELECTOR_BASE`] plus a selector node.
    pub destination: u32,
    pub conditions: Vec<Condition>,
}

/// An entry or exit node of a state machine (`SelectorStateConstant`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SelectorState {
    pub transitions: Vec<SelectorTransition>,
    pub full_path_id: u32,
    pub is_entry: bool,
}

/// A state machine (`StateMachineConstant`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StateMachine {
    pub states: Vec<State>,
    pub any_state_transitions: Vec<Transition>,
    pub selectors: Vec<SelectorState>,
    pub default_state: i64,
    pub synchronized_layer_count: i64,
}

/// A layer (`LayerConstant`): the state machine it runs, the body and skeleton masks, how it blends in.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Layer {
    pub state_machine: i64,
    pub synchronized_layer: i64,
    /// `m_BodyMask`'s three words.
    pub body_mask: [u32; 3],
    /// `m_SkeletonMask`: the CRC-32 of a path and its weight.
    pub skeleton_mask: Vec<(u32, f32)>,
    /// The CRC-32 of the layer's name.
    pub binding: u32,
    pub blending_mode: i64,
    pub default_weight: f32,
    pub ik_pass: bool,
    pub synced_layer_affects_timing: bool,
}

/// A controller parameter (`ValueConstant`): the CRC-32 of its name, its kind (1 float, 3 int, 4 bool, 9 trigger, as
/// Unity numbers them) and its place in the default values of that kind.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Parameter {
    pub id: u32,
    pub kind: i64,
    pub index: i64,
}

/// `unity.animator_controller`: an AnimatorController object as stored (the compiled `ControllerConstant`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityAnimatorController {
    pub controller: UnityObject,
    pub name: String,
    pub layers: Vec<Layer>,
    pub state_machines: Vec<StateMachine>,
    pub parameters: Vec<Parameter>,
    pub default_floats: Vec<f32>,
    pub default_ints: Vec<i64>,
    pub default_bools: Vec<bool>,
    /// The clips the blend trees' leaves index (`m_AnimationClips`).
    pub clips: Vec<UnityObject>,
    /// The scripts of the state machine behaviours (`m_StateMachineBehaviours`), by object.
    pub behaviours: Vec<UnityObject>,
    /// The CRC-32 of a name (a state, path, layer or parameter) and the name (`m_TOS`).
    pub tos: Vec<(u32, String)>,
}

impl UnityAnimatorController {
    /// The name a hash stands for in this controller's name table.
    pub fn name_of(&self, hash: u32) -> Option<&str> {
        self.tos.iter().find(|(h, _)| *h == hash).map(|(_, n)| n.as_str())
    }
}

impl Component for UnityAnimatorController {
    const NAME: &'static str = "unity.animator_controller";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        // the structure by its names: layers with their states and the parameters
        let layers: Vec<serde_json::Value> = self
            .layers
            .iter()
            .map(|l| {
                let machine = usize::try_from(l.state_machine)
                    .ok()
                    .and_then(|i| self.state_machines.get(i));
                serde_json::json!({
                    "name": self.name_of(l.binding),
                    "weight": l.default_weight,
                    "states": machine.map(|m| m.states.iter().map(|s| self.name_of(s.name_id)).collect::<Vec<_>>()),
                })
            })
            .collect();
        Ok(Some(serde_json::json!({
            "controller": self.controller,
            "name": self.name,
            "layers": layers,
            "parameters": self.parameters.iter().map(|p| serde_json::json!({
                "name": self.name_of(p.id), "kind": p.kind,
            })).collect::<Vec<_>>(),
            "clips": self.clips.len(),
            "behaviours": self.behaviours.len(),
        })))
    }
}

/// `unity.animator`: an Animator component as stored, on the rig it animates: its object, whether it is enabled and
/// applies root motion, and the controller and avatar entities (`unity.animator_controller`, `unity.avatar`) it names.
/// A controller-less Animator has its clips chosen at run time by scripts.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityAnimator {
    pub animator: UnityObject,
    pub enabled: bool,
    pub apply_root_motion: bool,
    pub controller: Option<Entity>,
    pub avatar: Option<Entity>,
}

impl Component for UnityAnimator {
    const NAME: &'static str = "unity.animator";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.controller);
        out.extend(self.avatar);
    }
}
