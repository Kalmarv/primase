//! Havok cloth in a world: a model's cloth tagfile as read (`havok.cloth`, [`HavokCloth`], [`read_cloth`]), for a
//! program that runs the cloth (a viewer stepping it per frame) or keeps it for a destination that has cloth.
//!
//! Reading takes no choice. The file's cloth data are read whole (`havok_formats::cloth::read`), and so is the skeleton
//! its animation container holds: each cloth data's one transform set is that skeleton, bone for bone (in both games'
//! files every transform set has the name and bone count of its file's one skeleton: Sekiro's 610 and Elden Ring's
//! 2,692), so a runner fills transform `i` with the world transform of bone `i`. What ties the cloth to the world's
//! model is the joints of the `core.skeleton` it follows of the same names ([`HavokCloth::joints`]); the bones and
//! joints keep their own hierarchies, which a model's part file may cut (a part's FLVER makes a root of a bone whose
//! parent it does not hold), so a runner composes the cloth skeleton's bones through the cloth skeleton's parents.
//! The cloth runs in the file's frame ([`HavokCloth::frame`] says how it stands in the world's).

use std::sync::Arc;

use havok_formats::Tagfile;
use havok_formats::animation::{self, Skeleton};
use havok_formats::cloth::{self as format, Physics};
use primase_core::frame::SourceFrame;
use primase_core::{Component, Entity, JointId, Skeleton as CoreSkeleton, World};

use crate::invalid;

/// `havok.cloth`: a model's Havok cloth as stored (every cloth data of its tagfile, read whole), the skeleton its
/// transform sets are, and the joints of the model's `core.skeleton` those bones are.
#[derive(Clone, Debug)]
pub struct HavokCloth {
    /// The entity holding the `core.skeleton` the cloth follows (the model's).
    pub skeleton: Entity,
    /// The tagfile's name as the source stores it (`bd_m_9040_c_l.hkx`).
    pub file: String,
    /// What the tagfile holds of cloth: its cloth data (at least one), and its ragdolls and constraints as read.
    pub physics: Arc<Physics>,
    /// The tagfile's own skeleton (its animation container's one skeleton): every cloth data's transform set is this
    /// skeleton, transform `i` bone `i`.
    pub bones: Arc<Skeleton>,
    /// One a bone of [`HavokCloth::bones`]: the joint of the `core.skeleton` of the same name, or `None` where the model
    /// has none (a runner then holds the bone at its reference transform under its parent).
    pub joints: Vec<Option<JointId>>,
    /// The file's frame in the world's: what a runner converts the joints' transforms into and the cloth's back out of.
    pub frame: SourceFrame,
    /// The tagfile's size in bytes, as read.
    pub file_bytes: u64,
}

impl Component for HavokCloth {
    const NAME: &'static str = "havok.cloth";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.skeleton);
    }
    /// The tagfile's size: what the read cloth data hold is of its order (particles, constraints, deformer blocks).
    fn payload_bytes(&self) -> u64 {
        self.file_bytes
    }
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::Linearity<Self> {
        primase_core::Linearity::Invariant
    }
    /// The skeleton entity holds a `core.skeleton` and every joint named is one of its; there is a joint entry for each
    /// bone; every cloth data has one transform set, which is the bones (their skeleton's name and count).
    fn verify(&self, _e: Entity, w: &World) -> std::result::Result<(), String> {
        let skeleton = w
            .get::<CoreSkeleton>(self.skeleton)
            .ok_or_else(|| format!("{:?} holds no core.skeleton", self.skeleton))?;
        if self.joints.len() != self.bones.bones.len() {
            return Err(format!(
                "{} joint entries, {} cloth skeleton bones",
                self.joints.len(),
                self.bones.bones.len()
            ));
        }
        if let Some(j) = self
            .joints
            .iter()
            .flatten()
            .find(|j| !skeleton.joints.iter().any(|joint| joint.id == **j))
        {
            return Err(format!("joint {} is not in the skeleton", j.0));
        }
        if self.physics.cloths.is_empty() {
            return Err("no cloth data".into());
        }
        check_sets(&self.physics, &self.bones).map_err(|e| e.to_string())
    }
}

/// Every cloth data of `physics` has one transform set, named as `bones` is and of its bone count.
fn check_sets(physics: &Physics, bones: &Skeleton) -> havok_formats::Result<()> {
    for data in &physics.cloths {
        let [set] = data.transform_sets.as_slice() else {
            return Err(invalid(
                0,
                format!(
                    "cloth data {}: {} transform sets, where every cloth data of the games has one",
                    data.name,
                    data.transform_sets.len()
                ),
            ));
        };
        if set.name != bones.name || set.transforms as usize != bones.bones.len() {
            return Err(invalid(
                0,
                format!(
                    "cloth data {}: transform set {} of {} transforms is not the file's skeleton {} of {} bones",
                    data.name,
                    set.name,
                    set.transforms,
                    bones.name,
                    bones.bones.len()
                ),
            ));
        }
    }
    Ok(())
}

/// The cloth of tagfile `file` (`bytes`, its types its own: the games' cloth tagfiles carry them) following the
/// `core.skeleton` `joints` holds on entity `skeleton`, in the file's frame `frame`; `None` for a tagfile with no cloth
/// data. Refused: a file whose animation container does not hold exactly one skeleton, or whose cloth data's transform
/// sets are not that skeleton.
pub fn read_cloth(
    file: &str,
    bytes: &[u8],
    skeleton: Entity,
    joints: &CoreSkeleton,
    frame: SourceFrame,
) -> havok_formats::Result<Option<HavokCloth>> {
    let tag = Tagfile::read(bytes, None)?;
    let physics = format::read(&tag)?;
    if physics.cloths.is_empty() {
        return Ok(None);
    }
    let mut skeletons = animation::container(&tag)?.skeletons;
    if skeletons.len() != 1 {
        return Err(invalid(
            0,
            format!(
                "{} skeletons in the animation container, where the cloth's transform sets need one",
                skeletons.len()
            ),
        ));
    }
    let bones = skeletons.remove(0);
    check_sets(&physics, &bones)?;
    let mapped = bones
        .bones
        .iter()
        .map(|bone| joints.joints.iter().find(|j| j.name == bone.name).map(|j| j.id))
        .collect();
    Ok(Some(HavokCloth {
        skeleton,
        file: file.to_owned(),
        physics: Arc::new(physics),
        bones: Arc::new(bones),
        joints: mapped,
        frame,
        file_bytes: bytes.len() as u64,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use havok_formats::animation::{Bone, QsTransform};
    use havok_formats::cloth::TransformSetDefinition;
    use primase_core::{Joint, Rest};

    fn bones(names: &[&str]) -> Skeleton {
        Skeleton {
            name: "Master".into(),
            parents: (0..names.len()).map(|i| i as i16 - 1).collect(),
            bones: names
                .iter()
                .map(|n| Bone {
                    name: (*n).into(),
                    lock_translation: false,
                })
                .collect(),
            reference_pose: vec![
                QsTransform {
                    translation: [0.0; 4],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0, 0.0],
                };
                names.len()
            ],
            reference_floats: Vec::new(),
            float_slots: Vec::new(),
        }
    }

    fn physics(sets: Vec<TransformSetDefinition>) -> Physics {
        Physics {
            cloths: vec![format::Cloth {
                name: "scarf".into(),
                target_platform: 2,
                sim_cloths: Vec::new(),
                buffers: Vec::new(),
                transform_sets: sets,
                operators: Vec::new(),
                states: Vec::new(),
                state_transitions: Vec::new(),
                actions: Vec::new(),
            }],
            ragdolls: Vec::new(),
            constraints: Vec::new(),
        }
    }

    fn set(name: &str, transforms: u32) -> TransformSetDefinition {
        TransformSetDefinition {
            name: name.into(),
            set_type: 0,
            transforms,
        }
    }

    #[test]
    fn a_transform_set_must_be_the_files_skeleton() {
        let skeleton = bones(&["Master", "Pelvis", "Neck"]);
        assert!(check_sets(&physics(vec![set("Master", 3)]), &skeleton).is_ok());
        // another name, another count, and two sets are refused
        assert!(check_sets(&physics(vec![set("Other", 3)]), &skeleton).is_err());
        assert!(check_sets(&physics(vec![set("Master", 2)]), &skeleton).is_err());
        assert!(check_sets(&physics(vec![set("Master", 3), set("Master", 3)]), &skeleton).is_err());
    }

    #[test]
    fn verify_names_the_joint_entries_and_the_skeleton() {
        let mut w = World::new();
        let e = w.spawn();
        let joint = |id: u32, name: &str| Joint {
            id: JointId(id),
            name: name.into(),
            parent: None,
            rest: Rest::Trs {
                translation: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
            },
        };
        w.insert(
            e,
            CoreSkeleton {
                joints: vec![joint(0, "Master"), joint(7, "Neck")],
            },
        );
        let mut cloth = HavokCloth {
            skeleton: e,
            file: "test_c.hkx".into(),
            physics: Arc::new(physics(vec![set("Master", 3)])),
            bones: Arc::new(bones(&["Master", "Pelvis", "Neck"])),
            joints: vec![Some(JointId(0)), None, Some(JointId(7))],
            frame: SourceFrame::WORLD,
            file_bytes: 0,
        };
        assert_eq!(cloth.verify(e, &w), Ok(()));
        cloth.joints[1] = Some(JointId(3));
        assert!(cloth.verify(e, &w).is_err(), "joint 3 is not in the skeleton");
        cloth.joints.pop();
        assert!(cloth.verify(e, &w).is_err(), "a joint entry short");
    }
}
