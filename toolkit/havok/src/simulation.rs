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
mod tests {
    #![allow(
        clippy::disallowed_methods,
        reason = "test I/O: installed definitions and captured cloth steps"
    )]
    //! Captured inputs exercise the core-facing runner for both games; no viewer or game-specific client is involved.

    use super::*;
    use crate::cloth::ModelMesh;
    use crate::inputs::{Game, OracleSet, Query, expect_found, resolve};
    use havok_formats::Tagfile;
    use havok_formats::cloth::OperatorKind;
    use primase_core::frame::{Axis, SourceFrame};
    use primase_core::{Entity, Joint, Rest};

    /// FromSoft's frame in the world's, as primase's FromSoft world reads it: z negated, in metres.
    const FRAME: SourceFrame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.0);

    /// The identity placement.
    const PLACED: Affine = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0., 0., 0.]];

    /// One step of a capture: its timestep, the bones of its transform set (four columns of four floats each), and the
    /// particle positions the game's step left (four floats each).
    struct Step {
        dt: f32,
        bones: Vec<[f32; 16]>,
        after: Vec<[f32; 4]>,
    }

    /// A capture's steps as havok-cloth's tests read them (`CLF5`, `CLF4` or `CLF3`, the layouts in havok-cloth's
    /// tests/game_capture.rs): per step the timestep, three particle states (before the step's notification, after it,
    /// and after the step: each the last substep, the damping, the positions and the previous positions), the bones,
    /// the buffers' matrices (`CLF5` and `CLF4`), the collidables before and after, the game's action calls, and the
    /// instance's constraint states (`CLF5`: a volume's 28 floats, a transition's state, time and a float per particle,
    /// each before and after the step).
    fn read_frames(bytes: &[u8]) -> Vec<Step> {
        let version = match &bytes[..4] {
            b"CLF5" => 5,
            b"CLF4" => 4,
            b"CLF3" => 3,
            _ => panic!("not a frames file"),
        };
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let f32_at = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let [steps, particles, bones, collidables] = [4, 8, 12, 16].map(u32_at);
        let buffers = if version >= 4 { u32_at(20) } else { 0 };
        let constraint_states = if version >= 5 { u32_at(24) } else { 0 };
        let mut at = 20 + 4 * (version - 3);
        let mut out = Vec::with_capacity(steps);
        for _ in 0..steps {
            let dt = f32_at(at);
            at += 4;
            // the third state's positions: after its last substep and damping, and the two states before it
            let positions = at + 2 * (8 + particles * 32) + 8;
            let after = (0..particles)
                .map(|p| std::array::from_fn(|k| f32_at(positions + p * 16 + k * 4)))
                .collect();
            at += 3 * (8 + particles * 32);
            let read: Vec<[f32; 16]> = (0..bones)
                .map(|b| std::array::from_fn(|k| f32_at(at + b * 64 + k * 4)))
                .collect();
            at += bones * 64 + buffers * 128 + collidables * 96 * 2;
            let calls = u32_at(at);
            at += 4 + calls * (4 + particles * 16);
            for _ in 0..constraint_states {
                // the set's index, the kind (1 a volume, 2 a transition), then the state before and after the step
                let state = match u32_at(at + 4) {
                    1 => 28 * 4,
                    2 => 8 + particles * 4,
                    kind => panic!("constraint state kind {kind}"),
                };
                at += 8 + 2 * state;
            }
            out.push(Step {
                dt,
                bones: read,
                after,
            });
        }
        assert_eq!(at, bytes.len(), "the frames file ends where its last step does");
        out
    }

    /// A captured bone (the file's frame) as the joint's model transform in the world's frame.
    fn joint_of(bone: &[f32; 16]) -> Affine {
        let col = |c: usize| [bone[c * 4], bone[c * 4 + 1], bone[c * 4 + 2]];
        affine(&FRAME.transform(&Transform {
            cols: [col(0), col(1), col(2), col(3)],
        }))
    }

    /// Capture `frames` of `game` (its cloth: the whole tagfile it names, its display buffers bound to the meshes
    /// `meshes` names of the read cloth) on a skeleton made of the cloth skeleton's own bones (so every bone is its
    /// joint, parent for parent), with the capture's steps and the cloth data it records; `None` without the inputs.
    fn capture(
        game: Game,
        frames: &str,
        meshes: impl Fn(&Physics) -> Vec<ModelMesh>,
    ) -> Option<(HavokCloth, Skeleton, Vec<Step>, String)> {
        let captures = Query::Oracle(OracleSet::Capture, game, "havok-cloth");
        let corpus = Query::Oracle(OracleSet::Corpus, game, "havok-cloth");
        let (dir, corpus_dir) = (resolve(captures)?, resolve(corpus)?);
        let list = std::fs::read_to_string(dir.join("captures.tsv")).unwrap();
        let rows: Vec<Vec<&str>> = list.lines().skip(1).map(|l| l.split('\t').collect()).collect();
        expect_found(captures, "captures", rows.len());
        let row = rows
            .iter()
            .find(|r| r.first() == Some(&frames))
            .unwrap_or_else(|| panic!("captures.tsv: no {frames}"));
        let [_, tagfile, cloth_data, _processor] = row.as_slice() else {
            panic!("captures.tsv: not four columns");
        };
        let bytes = std::fs::read(corpus_dir.join(tagfile)).unwrap();
        let tag = Tagfile::read(&bytes, None).unwrap();
        let bones = havok_formats::animation::container(&tag)
            .unwrap()
            .skeletons
            .remove(0);
        let skeleton = Skeleton {
            joints: bones
                .bones
                .iter()
                .enumerate()
                .map(|(i, b)| Joint {
                    id: JointId(i as u32),
                    name: b.name.clone(),
                    parent: u32::try_from(bones.parents[i]).ok().map(JointId),
                    rest: Rest::Affine(Transform::IDENTITY),
                })
                .collect(),
        };
        let physics = havok_formats::cloth::read(&tag).unwrap();
        let cloth = crate::cloth::read_cloth(
            tagfile,
            &bytes,
            Entity::from_index(0),
            &skeleton,
            FRAME,
            &meshes(&physics),
        )
        .unwrap()
        .expect("the capture's tagfile has cloth data");
        let steps = read_frames(&std::fs::read(dir.join(frames)).unwrap());
        Some((cloth, skeleton, steps, (*cloth_data).to_owned()))
    }

    fn model_of(bones: &[[f32; 16]]) -> BTreeMap<JointId, Affine> {
        bones
            .iter()
            .enumerate()
            .map(|(i, b)| (JointId(i as u32), joint_of(b)))
            .collect()
    }

    /// The runner gives every cloth data the game's own transforms: each bone of the transform set it fills from the
    /// joints posed as the game posed the bones is the recorded one, in every step (equal as floats: only signed zeros
    /// may differ, from the placement's sums).
    #[test]
    fn the_transform_set_is_filled_with_the_games_bones() {
        let Some((cloth, skeleton, steps, _)) = capture(Game::Elden, "rearrope-cap1.clf5", |_| Vec::new())
        else {
            return;
        };
        let mut runner = Runner::new(Arc::new(cloth), &skeleton, None).unwrap();
        fills_the_games_bones(&mut runner, &steps, "the cloth skeleton's parents");
    }

    /// Whatever parents the model gives the joints, each bone is its joint's model transform placed by the rig: an
    /// Elden Ring part's FLVER keeps the bones whose parents it does not hold as roots, an assembled skeleton keeps a
    /// joint no part parents as one, and the game's clips (mapped by the source's model-space pose) give such a joint
    /// the Havok skeleton's model-space transform. On a model whose joints are all roots, and on one whose joints each
    /// have the joint listed before them as their parent, the transform set filled from joints posed as the game posed
    /// the bones is the recorded one in every step (composed under the bone's parent, as the runner once read a root
    /// the bone is not, it counted the parents twice).
    #[test]
    fn the_models_parents_do_not_move_the_games_bones() {
        let Some((cloth, skeleton, steps, _)) = capture(Game::Elden, "rearrope-cap1.clf5", |_| Vec::new())
        else {
            return;
        };
        let cloth = Arc::new(cloth);
        let parented = |parent: &dyn Fn(usize) -> Option<JointId>| Skeleton {
            joints: skeleton
                .joints
                .iter()
                .enumerate()
                .map(|(i, j)| Joint {
                    parent: parent(i),
                    ..j.clone()
                })
                .collect(),
        };
        let roots = parented(&|_| None);
        let shifted = parented(&|i| i.checked_sub(1).map(|p| skeleton.joints[p].id));
        for (parents, rig) in [("roots", roots), ("the joint before", shifted)] {
            let mut runner = Runner::new(Arc::clone(&cloth), &rig, None).unwrap();
            fills_the_games_bones(&mut runner, &steps, parents);
        }
    }

    /// Every bone `runner` fills each instance's transform set with from joints posed as the game posed the bones in
    /// `steps` is the recorded one (equal as floats: only signed zeros may differ, from the placement's sums).
    fn fills_the_games_bones(runner: &mut Runner, steps: &[Step], parents: &str) {
        assert!(
            runner.bones.iter().all(|(b, _)| matches!(b, Bone::Joint(_))),
            "{parents}: every bone is its joint"
        );
        for (n, step) in steps.iter().enumerate() {
            runner.fill(&PLACED, &model_of(&step.bones)).unwrap();
            for instance in &runner.instances {
                for (b, (filled, recorded)) in instance.transform_sets[0].iter().zip(&step.bones).enumerate()
                {
                    for c in 0..4 {
                        for r in 0..3 {
                            assert!(
                                filled.cols[c].0[r] == recorded[c * 4 + r],
                                "{parents}: step {n} bone {b} column {c} row {r}: {} filled, {} recorded",
                                filled.cols[c].0[r],
                                recorded[c * 4 + r]
                            );
                        }
                    }
                }
            }
        }
    }

    /// The generic runner's cloth on the capture: posed frame by frame as the game posed the bones, at the capture's
    /// timesteps. The bones the cloth data's bone deformers set are joints the view drives; after the capture each
    /// stands on the cloth as an orthonormal frame (what those deformers make of a triangle) near where the game's
    /// own cloth left its bone (within half a metre: a bound on a cloth hanging off the body, not on how near the run
    /// is to the game's, which havok-cloth's game-capture tests measure).
    #[test]
    fn the_generic_runner_drives_elden_cloth_bones_on_a_capture() {
        drives_its_bones(Game::Elden, "rearrope-cap1.clf5");
    }

    /// Sekiro's cloths with bone deformers, as the rope's above: two NPC cloths of one character (`c1010_hakama`,
    /// `c1010_kusazuri`: object-space skins) and a sleeve whose cloth data has two sim cloths (`c7200_sleeve`: a
    /// bone-space skin for the first). In every step of these captures the game's transform set holds each bone the
    /// cloth data drives within 0.17 m of the cloth's particles, where its other bones are mostly hundreds of metres
    /// off (stale), so the bound compares the view's bones with the game's cloth.
    #[test]
    fn the_generic_runner_drives_sekiro_cloth_bones_on_captures() {
        for frames in ["hakama-cap6.clf5", "kusazuri-cap6.clf5", "sleeve-cap6.clf5"] {
            drives_its_bones(Game::Sekiro, frames);
        }
    }

    /// The generic runner's cloth run on capture `frames` of `game`; the joints the captured cloth data's bone deformers set
    /// end orthonormal and within half a metre of the game's bones (the tagfile's other cloth datas run too: the bones
    /// they drive are not the capture's, so are not compared).
    fn drives_its_bones(game: Game, frames: &str) {
        let Some((cloth, skeleton, steps, cloth_data)) = capture(game, frames, |_| Vec::new()) else {
            return;
        };
        // the bones the captured cloth data drives (of its transform set, whose bones are the skeleton's joints here)
        let data = cloth
            .physics
            .cloths
            .iter()
            .find(|c| c.name == cloth_data)
            .unwrap_or_else(|| panic!("{frames}: no cloth data {cloth_data}"));
        let set = &data.transform_sets[0].name;
        let own: BTreeSet<JointId> = data
            .operators
            .iter()
            .filter_map(|op| match &op.kind {
                OperatorKind::SimpleMeshBoneDeform {
                    output_transform_set,
                    triangle_bone_pairs,
                    ..
                } => Some((*output_transform_set, triangle_bone_pairs)),
                _ => None,
            })
            .flat_map(|(output, pairs)| {
                assert_eq!(
                    data.transform_sets[output as usize].name, *set,
                    "{frames}: the bones driven into another transform set"
                );
                pairs.iter().map(|&(bone, _)| JointId(u32::from(bone) / 64))
            })
            .collect();
        assert!(!own.is_empty(), "{frames}: the cloth data drives bones");
        let cloth = Arc::new(cloth);
        let mut runner = cloth.start(&skeleton, None).unwrap();
        let mut model = BTreeMap::new();
        let mut accumulated = 0.0f64;
        for (n, step) in steps.iter().enumerate() {
            model = model_of(&step.bones);
            if n == 0 {
                for _ in 0..30 {
                    runner.step(&PLACED, &mut model, &[], 1.0 / 30.0).unwrap();
                }
            } else {
                accumulated += f64::from(step.dt);
                while accumulated + 1e-12 >= 1.0 / 30.0 {
                    runner.step(&PLACED, &mut model, &[], 1.0 / 30.0).unwrap();
                    accumulated -= 1.0 / 30.0;
                }
                runner.hold(&PLACED, &mut model);
            }
        }
        let runner = Runner::new(Arc::clone(&cloth), &skeleton, None).unwrap();
        assert!(
            own.is_subset(&runner.driven),
            "{frames}: the view drives the bones the cloth data drives"
        );
        let last = &steps.last().unwrap().bones;
        for joint in &own {
            let m = model[joint];
            assert!(m.iter().flatten().all(|x| x.is_finite()), "{frames} {joint:?}");
            let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
            // the deformers normalise each axis with the processor's reciprocal square root estimate (relative error
            // at most 1.5 * 2^-12, so a squared length up to 7.3e-4 from 1): the game's own driven bones in these
            // captures are up to 4.9e-4 from unit length, and orthogonal within 4.3e-7
            for c in 0..3 {
                assert!(
                    (dot(m[c], m[c]) - 1.0).abs() < 1e-3,
                    "{frames} {joint:?} column {c}: not unit"
                );
                assert!(
                    dot(m[c], m[(c + 1) % 3]).abs() < 1e-4,
                    "{frames} {joint:?} columns {c}: not orthogonal"
                );
            }
            let game = joint_of(&last[joint.0 as usize]);
            let off = (0..3).map(|r| (m[3][r] - game[3][r]).powi(2)).sum::<f64>().sqrt();
            assert!(off < 0.5, "{frames} {joint:?}: {off} m from the game's bone");
        }
    }

    /// Sekiro's scarf neck (`neck-cap5`: the Wolf's `BD_M_9040_muffler_neck`, whose display buffer, the mesh the game
    /// draws, is its particles gathered), run on the capture's bones at its timesteps with the display buffer bound and
    /// fed the mesh as posed: after every step, each display vertex the gather writes shows the runner's own particle
    /// exactly (in the world's frame), and after the capture it stands within half a metre of the game's own particle
    /// (the same bound as for the rope's bones: a frame or mapping error puts it metres away, the capture standing some
    /// 200 m from the origin).
    #[test]
    fn the_scarfs_display_vertices_follow_its_particles_on_a_sekiro_capture() {
        let neck = "BD_M_Muffler_neck";
        // the display buffer bound to a mesh of its own counts (no FLVER here: the binding is primase's, tested there)
        let bind = |physics: &Physics| -> Vec<ModelMesh> {
            physics
                .cloths
                .iter()
                .flat_map(|c| &c.buffers)
                .filter(|b| b.buffer_type == crate::cloth::DISPLAY_BUFFER && b.name == neck)
                .map(|b| ModelMesh {
                    name: b.name.clone(),
                    vertices: b.vertices as usize,
                    triangles: vec![0; b.triangles as usize * 3],
                })
                .collect()
        };
        let Some((cloth, skeleton, steps, cloth_data)) = capture(Game::Sekiro, "neck-cap5.clf5", bind) else {
            return;
        };
        let [display] = cloth.displays.as_slice() else {
            panic!("one display bound: {:?}", cloth.unbound);
        };
        let data = display.data as usize;
        assert_eq!(cloth.physics.cloths[data].name, cloth_data);
        // the gather from the sim's particles into the display buffer
        let gather = cloth.physics.cloths[data]
            .operators
            .iter()
            .find_map(|op| match &op.kind {
                OperatorKind::GatherAllVertices {
                    input_from_output,
                    output_buffer,
                    ..
                } if *output_buffer == display.buffer => Some(input_from_output.clone()),
                _ => None,
            })
            .expect("the neck's particles gathered into its display buffer");
        let count = cloth.display_buffer(display).unwrap().vertices as usize;
        let cloth = Arc::new(cloth);
        let mut runner = Runner::new(Arc::clone(&cloth), &skeleton, None).unwrap();
        let vertices: Vec<u32> = (0..count as u32).collect();
        let posed = Vertices {
            positions: vec![[0.0; 3]; count],
            normals: Some(vec![[0.0, 1.0, 0.0]; count]),
            tangents: Some(vec![[1.0, 0.0, 0.0, 1.0]; count]),
            further_tangents: BTreeMap::new(),
        };
        let input = [MeshInput {
            output: 0,
            vertices: &vertices,
            posed: &posed,
        }];
        let gathered: Vec<(usize, usize)> = gather
            .iter()
            .enumerate()
            .filter_map(|(v, &p)| usize::try_from(p).ok().map(|p| (v, p)))
            .collect();
        assert!(!gathered.is_empty());
        for (n, step) in steps.iter().enumerate() {
            let mut model = model_of(&step.bones);
            runner.step(&PLACED, &mut model, &input, step.dt).unwrap();
            let shown = runner.output(0).expect("the display buffer after a step");
            let particles = &runner.instances[data].sims[0].positions;
            for &(v, p) in &gathered {
                assert_eq!(
                    shown.positions[v],
                    FRAME.point(xyz(&particles[p])),
                    "step {n} display vertex {v}: particle {p}"
                );
            }
        }
        let shown = runner.output(0).unwrap();
        let game = &steps.last().unwrap().after;
        for &(v, p) in &gathered {
            let at = FRAME.point([game[p][0], game[p][1], game[p][2]]);
            let off = (0..3)
                .map(|r| f64::from(shown.positions[v][r] - at[r]).powi(2))
                .sum::<f64>()
                .sqrt();
            assert!(
                off < 0.5,
                "display vertex {v}: {off} m from the game's particle {p}"
            );
        }
    }
}
