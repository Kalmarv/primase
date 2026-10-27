//! `unity.avatar`, `unity.animator_controller` and `unity.animator`: Unity's Animator side as components. The avatar and
//! the controller are the records `unity_formats::animator` reads; the Animator pairs them with a rig by entity.

use primase_core::{Component, Entity, JsonError};
use serde::Serialize;
use unity_formats::animator::{UnityAnimatorController as Controller, UnityAvatar as Avatar};

use crate::components::{Stored, UnityObject};

/// `unity.avatar`: an Avatar object as stored, on an entity of its own that the Animators using it name.
pub type UnityAvatar = Stored<Avatar>;

/// `unity.animator_controller`: an AnimatorController object as stored (the compiled `ControllerConstant`).
pub type UnityAnimatorController = Stored<Controller>;

impl Component for Stored<Avatar> {
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

impl Component for Stored<Controller> {
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
