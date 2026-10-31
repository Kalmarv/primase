//! Havok's implementation of the engine-independent animation simulation contract.
use crate::cloth::HavokCloth;
use havok_cloth::instance::{self, ClothInstance};
use havok_cloth::matrix::Mat4;
use havok_cloth::simd::V4;
use havok_formats::animation::QsTransform;
use havok_formats::cloth::{Cloth, Physics};
use primase_animation::deform::Vertices;
use primase_animation::pose::{Affine, affine, compose, inverse, transform};
use primase_animation::simulation::{JointSimulation, MeshInput, Simulation, SimulationSource};
use primase_core::{JointId, Skeleton, Transform};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Deref;
use std::sync::Arc;

impl HavokCloth {
    /// A generic simulation using the installed definition, with one output for each bound display buffer.
    pub fn simulation(&self) -> Simulation {
        Simulation::new(
            self.skeleton,
            Arc::new(self.clone()),
            self.displays
                .iter()
                .map(|display| self.display_buffer(display).map_or(0, |buffer| buffer.vertices))
                .collect(),
        )
    }
}

impl SimulationSource for HavokCloth {
    fn name(&self) -> String {
        self.file.clone()
    }
    fn start(
        &self,
        skeleton: &Skeleton,
        joints: Option<&BTreeMap<JointId, JointId>>,
    ) -> Result<Box<dyn JointSimulation>, String> {
        Ok(Box::new(Runner::new(Arc::new(self.clone()), skeleton, joints)?))
    }
}

/// One cloth data of a file's, as an instance holds it (the file's read cloth is shared).
#[derive(Clone, Debug)]
struct Data {
    physics: Arc<Physics>,
    index: usize,
}

impl Deref for Data {
    type Target = Cloth;
    fn deref(&self) -> &Cloth {
        &self.physics.cloths[self.index]
    }
}

/// Where a bone's world transform comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Bone {
    /// The joint's model transform, placed by the rig.
    Joint(JointId),
    /// The bone's reference transform (in the world's frame) under its parent: the model has no joint of its name.
    Rest(Affine),
}

/// A running `havok.cloth` on one rig.
struct Runner {
    cloth: Arc<HavokCloth>,
    instances: Vec<ClothInstance<Data>>,
    /// Each bone of the cloth skeleton: where its world transform comes from, and its parent there.
    bones: Vec<(Bone, Option<usize>)>,
    /// The (instance, transform set, bone) each driven joint is read from.
    joint_outputs: BTreeMap<JointId, (usize, u32, usize)>,
    driven: BTreeSet<JointId>,
    /// For each joint of the model under a driven joint (or driven itself), the nearest driven joint at or above it.
    carried_by: BTreeMap<JointId, JointId>,
    /// Where the last step left each driven joint, in the world.
    held: BTreeMap<JointId, Affine>,
    /// Each display (by its index in [`HavokCloth::displays`]): its instance and buffer.
    displays: Vec<(usize, usize)>,
    /// What the last step left each display buffer with, in the world's frame.
    shown: Vec<Option<Vertices>>,
}

/// A stored reference transform (`hkQsTransform`: translation, rotation quaternion `x y z w`, scale) as an affine map.
fn qs_affine(q: &QsTransform) -> Affine {
    let [x, y, z, w] = q.rotation.map(f64::from);
    let [sx, sy, sz] = [q.scale[0], q.scale[1], q.scale[2]].map(f64::from);
    let col = |c: [f64; 3], s: f64| c.map(|v| v * s);
    [
        col(
            [
                1.0 - 2.0 * (y * y + z * z),
                2.0 * (x * y + z * w),
                2.0 * (x * z - y * w),
            ],
            sx,
        ),
        col(
            [
                2.0 * (x * y - z * w),
                1.0 - 2.0 * (x * x + z * z),
                2.0 * (y * z + x * w),
            ],
            sy,
        ),
        col(
            [
                2.0 * (x * z + y * w),
                2.0 * (y * z - x * w),
                1.0 - 2.0 * (x * x + y * y),
            ],
            sz,
        ),
        [q.translation[0], q.translation[1], q.translation[2]].map(f64::from),
    ]
}

/// A transform as havok-cloth's matrix: its columns, the `w` lanes of an affine map.
fn to_mat4(t: &Transform) -> Mat4 {
    let c = t.cols;
    Mat4 {
        cols: [
            V4([c[0][0], c[0][1], c[0][2], 0.0]),
            V4([c[1][0], c[1][1], c[1][2], 0.0]),
            V4([c[2][0], c[2][1], c[2][2], 0.0]),
            V4([c[3][0], c[3][1], c[3][2], 1.0]),
        ],
    }
}

/// havok-cloth's matrix as a transform (its `w` lanes dropped).
fn from_mat4(m: &Mat4) -> Transform {
    Transform {
        cols: m.cols.map(|c| [c.0[0], c.0[1], c.0[2]]),
    }
}

/// The first three lanes of a vector.
fn xyz(v: &V4) -> [f32; 3] {
    [v.0[0], v.0[1], v.0[2]]
}

impl Runner {
    /// A runner of `cloth` on a rig of `skeleton`; `map` takes the joints of the skeleton the cloth was read for to
    /// `skeleton`'s, where they differ.
    fn new(
        cloth: Arc<HavokCloth>,
        skeleton: &Skeleton,
        map: Option<&BTreeMap<JointId, JointId>>,
    ) -> Result<Runner, String> {
        // bone i's joint of the rig's skeleton
        let joint_at = |i: usize| -> Option<JointId> {
            let j = cloth.joints.get(i).copied().flatten()?;
            match map {
                Some(m) => m.get(&j).copied(),
                None => Some(j),
            }
        };
        let joint_parent: BTreeMap<JointId, Option<JointId>> =
            skeleton.joints.iter().map(|j| (j.id, j.parent)).collect();
        let bones_of = &cloth.bones;
        let mut bones = Vec::with_capacity(bones_of.bones.len());
        for (i, bone) in bones_of.bones.iter().enumerate() {
            let parent = bones_of
                .parents
                .get(i)
                .and_then(|&p| usize::try_from(p).ok())
                .filter(|&p| p < bones_of.bones.len());
            let source = match joint_at(i).filter(|j| joint_parent.contains_key(j)) {
                Some(joint) => Bone::Joint(joint),
                None => {
                    let rest = bones_of
                        .reference_pose
                        .get(i)
                        .ok_or_else(|| format!("bone {} has no reference transform", bone.name))?;
                    Bone::Rest(affine(&cloth.frame.transform(&transform(&qs_affine(rest)))))
                }
            };
            bones.push((source, parent));
        }
        let mut instances = Vec::with_capacity(cloth.physics.cloths.len());
        let mut joint_outputs = BTreeMap::new();
        for index in 0..cloth.physics.cloths.len() {
            let data = Data {
                physics: Arc::clone(&cloth.physics),
                index,
            };
            for (set, bone) in instance::driven_bones(&data) {
                if let Some(joint) = joint_at(bone) {
                    joint_outputs.insert(joint, (index, set, bone));
                }
            }
            let built = ClothInstance::new(data)
                .map_err(|e| format!("cloth data {}: {e}", cloth.physics.cloths[index].name))?;
            instances.push(built);
        }
        // each display buffer's triangles are its mesh's (its vertex frame operators read them)
        let mut displays = Vec::with_capacity(cloth.displays.len());
        for (i, d) in cloth.displays.iter().enumerate() {
            let (data, buffer) = (d.data as usize, d.buffer as usize);
            let triangles = instances
                .get_mut(data)
                .and_then(|instance| instance.triangles.get_mut(buffer))
                .ok_or_else(|| format!("display {i}: no buffer {buffer} of cloth data {data}"))?;
            triangles.clone_from(&d.triangles);
            displays.push((data, buffer));
        }
        let driven: BTreeSet<JointId> = joint_outputs.keys().copied().collect();
        let mut carried_by = BTreeMap::new();
        for joint in &skeleton.joints {
            let mut at = Some(joint.id);
            // up the model's hierarchy to the nearest driven joint (a bounded walk: a cycle is no skeleton's)
            for _ in 0..=skeleton.joints.len() {
                let Some(j) = at else { break };
                if driven.contains(&j) {
                    carried_by.insert(joint.id, j);
                    break;
                }
                at = joint_parent.get(&j).copied().flatten();
            }
        }
        let shown = vec![None; displays.len()];
        Ok(Runner {
            cloth,
            instances,
            bones,
            joint_outputs,
            driven,
            carried_by,
            held: BTreeMap::new(),
            displays,
            shown,
        })
    }

    /// Each bone's world transform, in the world's frame, from the rig's pose (`model`, placed by `placement`).
    fn world_bones(&self, placement: &Affine, model: &BTreeMap<JointId, Affine>) -> Vec<Affine> {
        let mut world: Vec<Option<Affine>> = vec![None; self.bones.len()];
        for i in 0..self.bones.len() {
            // the chain up to a bone already made (the files list parents first; this does not rely on it)
            let mut chain = vec![i];
            while let Some(&b) = chain.last() {
                match self.bones[b].1 {
                    Some(p) if world[p].is_none() && !chain.contains(&p) => chain.push(p),
                    _ => break,
                }
            }
            for &b in chain.iter().rev() {
                if world[b].is_some() {
                    continue;
                }
                let above = self.bones[b].1.and_then(|p| world[p]).unwrap_or(*placement);
                let made = match self.bones[b].0 {
                    Bone::Joint(j) => model.get(&j).map(|m| compose(placement, m)),
                    Bone::Rest(r) => Some(compose(&above, &r)),
                };
                // a joint the pose does not hold: the bone stays with its parent
                world[b] = Some(made.unwrap_or(above));
            }
        }
        world.into_iter().map(|w| w.unwrap_or(*placement)).collect()
    }

    /// Fill every instance's transform set with the bones' world transforms in the file's frame, from the rig's pose.
    fn fill(&mut self, placement: &Affine, model: &BTreeMap<JointId, Affine>) -> Result<(), String> {
        let frame = self.cloth.frame;
        let bones: Vec<Mat4> = self
            .world_bones(placement, model)
            .iter()
            .map(|w| to_mat4(&frame.transform_to_source(&transform(w))))
            .collect();
        for (index, instance) in self.instances.iter_mut().enumerate() {
            let set = instance
                .transform_sets
                .get_mut(0)
                .ok_or_else(|| format!("cloth data {index} has no transform set"))?;
            if set.len() != bones.len() {
                return Err(format!(
                    "cloth data {index}: {} transforms, {} bones",
                    set.len(),
                    bones.len()
                ));
            }
            set.copy_from_slice(&bones);
        }
        Ok(())
    }

    /// Put each mesh as posed into the display buffer it shows, in the file's frame: positions (`w` 1), normals and
    /// tangent directions (`w` 0), each at the buffer vertex its vertex is.
    fn load(&mut self, meshes: &[MeshInput<'_>]) -> Result<(), String> {
        let frame = self.cloth.frame;
        for input in meshes {
            let &(data, buffer) = self
                .displays
                .get(input.output)
                .ok_or_else(|| format!("no display {}", input.output))?;
            let b = self
                .instances
                .get_mut(data)
                .and_then(|i| i.buffers.get_mut(buffer))
                .ok_or_else(|| format!("display {}: no buffer {buffer}", input.output))?;
            let posed = input.posed;
            for (k, &v) in input.vertices.iter().enumerate() {
                let v = v as usize;
                if let (Some(slot), Some(p)) = (b.positions.get_mut(v), posed.positions.get(k)) {
                    let [x, y, z] = frame.point_to_source(*p);
                    *slot = V4([x, y, z, 1.0]);
                }
                if let (Some(slot), Some(n)) = (
                    b.normals.get_mut(v),
                    posed.normals.as_ref().and_then(|n| n.get(k)),
                ) {
                    let [x, y, z] = frame.direction_to_source(*n);
                    *slot = V4([x, y, z, 0.0]);
                }
                if let (Some(slot), Some(t)) = (
                    b.tangents.get_mut(v),
                    posed.tangents.as_ref().and_then(|t| t.get(k)),
                ) {
                    let [x, y, z] = frame.direction_to_source([t[0], t[1], t[2]]);
                    *slot = V4([x, y, z, 0.0]);
                }
            }
        }
        Ok(())
    }
}

impl JointSimulation for Runner {
    fn step(
        &mut self,
        placement: &Affine,
        model: &mut BTreeMap<JointId, Affine>,
        meshes: &[MeshInput<'_>],
        seconds: f32,
    ) -> Result<(), String> {
        self.fill(placement, model)?;
        self.load(meshes)?;
        for instance in &mut self.instances {
            // Runtime effect wind is not authored in this definition. Stepping without an external action
            // still applies the installed gravity, constraints and animation inputs.
            instance.step(seconds, &mut []).map_err(|e| e.to_string())?;
        }
        let frame = self.cloth.frame;
        for (&joint, &(index, set, bone)) in &self.joint_outputs {
            if let Some(m) = self.instances[index]
                .transform_sets
                .get(set as usize)
                .and_then(|s| s.get(bone))
            {
                self.held.insert(joint, affine(&frame.transform(&from_mat4(m))));
            }
        }
        // the display buffers, back in the world's frame (a tangent's handedness is the mesh's own, not stated here)
        for (d, &(data, buffer)) in self.displays.iter().enumerate() {
            let b = &self.instances[data].buffers[buffer];
            self.shown[d] = Some(Vertices {
                positions: b.positions.iter().map(|p| frame.point(xyz(p))).collect(),
                normals: Some(b.normals.iter().map(|n| frame.direction(xyz(n))).collect()),
                tangents: Some(
                    b.tangents
                        .iter()
                        .map(|t| {
                            let [x, y, z] = frame.direction(xyz(t));
                            [x, y, z, 1.0]
                        })
                        .collect(),
                ),
                further_tangents: BTreeMap::new(),
            });
        }
        Ok(())
    }

    fn hold(&self, placement: &Affine, model: &mut BTreeMap<JointId, Affine>) {
        if self.held.is_empty() {
            return;
        }
        let Some(unplace) = inverse(placement) else {
            return;
        };
        let original = model.clone();
        // each driven joint's new model transform, and what it moved by
        let mut moved: BTreeMap<JointId, Affine> = BTreeMap::new();
        for (&joint, world) in &self.held {
            let target = compose(&unplace, world);
            if let Some(before) = original.get(&joint).and_then(inverse) {
                moved.insert(joint, compose(&target, &before));
            }
            model.insert(joint, target);
        }
        // the joints under a driven joint that it does not drive go with it
        for (joint, by) in &self.carried_by {
            if joint == by || self.driven.contains(joint) {
                continue;
            }
            if let (Some(delta), Some(m)) = (moved.get(by), original.get(joint)) {
                model.insert(*joint, compose(delta, m));
            }
        }
    }

    fn driven(&self) -> &BTreeSet<JointId> {
        &self.driven
    }

    fn output(&self, output: usize) -> Option<&Vertices> {
        self.shown.get(output)?.as_ref()
    }
}

#[cfg(test)]
#[path = "simulation_tests.rs"]
mod tests;
