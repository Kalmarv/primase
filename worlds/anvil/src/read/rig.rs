//! A rig: the bones of a SkeletonComponent's main Skeleton and of its addon Skeletons as one tree, and the palette a
//! Mesh's bone list makes of it.
//!
//! A bone is named by the CRC-32 of its name ([`StoredBone::name_hash`]), and a Mesh's bone record (`MeshBone`) names
//! its bone the same way ([`StoredMeshBone::bone_id`]): 497,168 of the 497,194 ids of Riders Republic's 29,174 meshes
//! with bones equal a bone of some Skeleton (tests/skin_binding.rs). So two Skeletons holding a bone of one name hold
//! one joint. The character kit's addon Skeletons (`MAD_CHR_SkelAddon_*`) are made so: each starts at the main
//! skeleton's root with copies of the bones above the part it adds (the 146 SkeletonComponents' addons share 1 or more
//! bones with their main Skeleton), and adds the part's own below them.
//!
//! [`StoredRig::assemble`] joins them by that name: the first Skeleton given (the main one) keeps all its bones; each
//! later one adds the bones whose name no earlier one holds, each hung from the bone its parent pointer names (its
//! name, wherever the rig first holds it) with the local position and rotation it stores. A bone both hold is the
//! earlier one's. Over the 146 SkeletonComponents (126 addon Skeletons): 617 bones are shared with an earlier
//! Skeleton and 707 are added, and the bone-bearing meshes of their Entities' Visuals name only bones of the rig on
//! 523 of 571; the other 48 (the animals' and the bike cinematics') each miss one bone, `8f6a7739`, held by no
//! Skeleton but `MAD_CHR_SkelAddon_EnvProxy_M`'s (tests/skin_binding.rs). That the shared bones of two Skeletons agree
//! in pose is not established: their stored global positions differ on 120 of the 126 addons, so an added bone's local
//! transform is relative to a parent the main Skeleton poses. Nor is a mesh's `InitialMatrixInverse` the Skeleton's rest:
//! it is the inverse of the bone's stored rest world matrix (a 4x4 of rows, the rotation rows those of the quaternion's
//! matrix, the translation row `-p R`) to 1e-3 for every bone of 233 of the 571 meshes (the props' and some bike parts'); for 136 it
//! differs from it by one transform the same on all the mesh's bones (the animals': a quarter turn about z and an
//! offset), and for 202 by one that varies with the bone (the characters' bike parts and others: the bind pose is not
//! the stored rest). The human main Skeleton `MAD_CHR_Skel_U` carries its own transform (0, 0, 0.964 and a quarter
//! turn about z) that its bones' global positions leave out; on the one rider part looked at (`MAD_CHR_BP_001_U_A001_LOD3`,
//! bone `462ea286`) the bind position is the stored global one with x and y exchanged and 0.964 added to z.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::collections::{HashMap, HashSet};

use crate::components::StoredSkeleton;
pub use crate::components::{RigBone, StoredRig};
use crate::read::mesh::StoredMeshBone;

/// Why Skeletons can't be joined into a rig.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("skeleton {skeleton}: bone {bone:08x} names a parent {parent:#x} that the skeleton doesn't hold")]
    Parent { skeleton: usize, bone: u32, parent: u64 },
    #[error("skeleton {skeleton}: two bones named {bone:08x}")]
    Duplicate { skeleton: usize, bone: u32 },
    #[error("the bone {bone:08x} is its own ancestor")]
    Cycle { bone: u32 },
}

impl StoredRig {
    /// The rig of `skeletons`, the main one first (see the module documentation). Refused where a Skeleton holds
    /// two bones of one name or a parent pointer names a bone it doesn't hold, or the bones' parents form a cycle.
    pub fn assemble(skeletons: &[&StoredSkeleton]) -> Result<StoredRig, Error> {
        let mut bones: Vec<RigBone> = Vec::new();
        let mut held: HashMap<u32, usize> = HashMap::new();
        for (skeleton, s) in skeletons.iter().enumerate() {
            let by_id: HashMap<u64, u32> = s.bones.iter().map(|b| (b.id, b.name_hash)).collect();
            let mut own: HashSet<u32> = HashSet::new();
            for b in &s.bones {
                if !own.insert(b.name_hash) {
                    return Err(Error::Duplicate {
                        skeleton,
                        bone: b.name_hash,
                    });
                }
                let parent = b
                    .parent
                    .map(|p| {
                        by_id.get(&p).copied().ok_or(Error::Parent {
                            skeleton,
                            bone: b.name_hash,
                            parent: p,
                        })
                    })
                    .transpose()?;
                if held.contains_key(&b.name_hash) {
                    continue;
                }
                held.insert(b.name_hash, bones.len());
                bones.push(RigBone {
                    name_hash: b.name_hash,
                    parent,
                    local_position: b.local_position,
                    local_rotation: b.local_rotation,
                    skeleton,
                });
            }
        }
        let rig = StoredRig { bones };
        for b in &rig.bones {
            let mut at = b.parent;
            for _ in 0..rig.bones.len() {
                let Some(p) = at else { break };
                if p == b.name_hash {
                    return Err(Error::Cycle { bone: p });
                }
                at = rig.bone(p).and_then(|p| p.parent);
            }
            if at.is_some() {
                return Err(Error::Cycle { bone: b.name_hash });
            }
        }
        Ok(rig)
    }

    /// The bone named `name_hash`.
    pub fn bone(&self, name_hash: u32) -> Option<&RigBone> {
        self.bones.iter().find(|b| b.name_hash == name_hash)
    }

    /// The position in [`StoredRig::bones`] of the bone named `name_hash`.
    pub fn position(&self, name_hash: u32) -> Option<usize> {
        self.bones.iter().position(|b| b.name_hash == name_hash)
    }

    /// A Mesh's palette in this rig: for each of its bone records, in order (the skin stream's slot numbers), the
    /// position of the rig's bone of that name, none where the rig holds no such bone.
    pub fn palette(&self, mesh_bones: &[StoredMeshBone]) -> Vec<Option<usize>> {
        let at: HashMap<u32, usize> = self
            .bones
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name_hash, i))
            .collect();
        mesh_bones.iter().map(|b| at.get(&b.bone_id).copied()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::StoredBone;

    fn bone(id: u64, name_hash: u32, parent: Option<u64>, x: f32) -> StoredBone {
        StoredBone {
            id,
            name_hash,
            parent,
            mirror: None,
            global_position: [0.0; 4],
            global_rotation: [0.0, 0.0, 0.0, 1.0],
            local_position: [x, 0.0, 0.0, 0.0],
            local_rotation: [0.0, 0.0, 0.0, 1.0],
            flag: 0,
            kind: 0,
            hashes: Vec::new(),
            kind2: 7,
            unnamed: 1.0,
            key: 0,
        }
    }

    fn skeleton(bones: Vec<StoredBone>) -> StoredSkeleton {
        StoredSkeleton {
            kind: 0,
            root: bones.first().map(|b| b.id),
            bones,
            position: [0.0; 4],
            rotation: [0.0, 0.0, 0.0, 1.0],
            body_part_mapping: None,
            pose_groups: Vec::new(),
            ik_data: None,
            look_at: None,
            floats: [0.0; 2],
            reflex3: None,
            hashes: Vec::new(),
        }
    }

    fn mesh_bone(bone_id: u32) -> StoredMeshBone {
        StoredMeshBone {
            bone_id,
            initial_matrix_inverse: [[0.0; 4]; 4],
            used_by_submeshes: true,
        }
    }

    #[test]
    fn an_addon_adds_the_bones_the_main_skeleton_lacks_under_the_bones_they_share() {
        // main: hips -> spine; addon: hips -> spine -> lash (new), with the spine at another place
        let main = skeleton(vec![bone(1, 0xA, None, 0.0), bone(2, 0xB, Some(1), 1.0)]);
        let addon = skeleton(vec![
            bone(10, 0xA, None, 0.0),
            bone(11, 0xB, Some(10), 5.0),
            bone(12, 0xC, Some(11), 2.0),
        ]);
        let rig = StoredRig::assemble(&[&main, &addon]).expect("rig");
        let names: Vec<u32> = rig.bones.iter().map(|b| b.name_hash).collect();
        assert_eq!(names, [0xA, 0xB, 0xC]);
        // the shared spine is the main skeleton's, the new bone hangs from it with its stored local position
        assert_eq!(
            rig.bone(0xB).map(|b| (b.skeleton, b.local_position[0])),
            Some((0, 1.0))
        );
        let c = rig.bone(0xC).expect("lash");
        assert_eq!((c.parent, c.skeleton, c.local_position[0]), (Some(0xB), 1, 2.0));
    }

    #[test]
    fn a_palette_names_the_rig_bone_of_each_slot_and_none_for_a_bone_the_rig_lacks() {
        let main = skeleton(vec![bone(1, 0xA, None, 0.0), bone(2, 0xB, Some(1), 1.0)]);
        let rig = StoredRig::assemble(&[&main]).expect("rig");
        let palette = rig.palette(&[mesh_bone(0xB), mesh_bone(0xFF), mesh_bone(0xA)]);
        assert_eq!(palette, [Some(1), None, Some(0)]);
    }

    #[test]
    fn a_parent_the_skeleton_does_not_hold_and_a_duplicate_name_are_refused() {
        let lost = skeleton(vec![bone(1, 0xA, Some(99), 0.0)]);
        assert_eq!(
            StoredRig::assemble(&[&lost]),
            Err(Error::Parent {
                skeleton: 0,
                bone: 0xA,
                parent: 99
            })
        );
        let twice = skeleton(vec![bone(1, 0xA, None, 0.0), bone(2, 0xA, Some(1), 0.0)]);
        assert_eq!(
            StoredRig::assemble(&[&twice]),
            Err(Error::Duplicate {
                skeleton: 0,
                bone: 0xA
            })
        );
    }

    #[test]
    fn parents_that_form_a_cycle_are_refused() {
        let ring = skeleton(vec![bone(1, 0xA, Some(2), 0.0), bone(2, 0xB, Some(1), 0.0)]);
        assert!(matches!(StoredRig::assemble(&[&ring]), Err(Error::Cycle { .. })));
    }
}
