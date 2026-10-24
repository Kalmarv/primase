//! A skeleton (`core.skeleton`): the joints that skins bind to and clips animate, each with its parent and its local
//! rest transform as the source authors it. A definition: a skeleton is shared by every mesh bound to it and every
//! occurrence that poses it, and holds no pose of its own beyond its rest.
//!
//! A joint's identity is its [`JointId`], scoped to its skeleton: what bindings, clips and joint maps name. Its name
//! (and any tag or hash a source keeps beside it) is provenance: names repeat in real data, so nothing finds a joint
//! by its name. Joints are listed in the source's order; a parent may come after its child.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;

use crate::frame::SourceFrame;
use crate::{Component, Entity, JsonError, Transform, World};

/// A joint's identity within its skeleton (stable while the skeleton is edited; a source's index or id as read).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct JointId(pub u32);

/// A joint's rest transform in its parent's space (the skeleton's, for a root): `parent = M * local`. One form is the
/// authority, as the source states it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum Rest {
    /// Any affine map (a source storing a matrix).
    Affine(Transform),
    /// Translation, rotation, scale, applied scale first: `M = T * R * S` (glTF's node order). `rotation` is a unit
    /// quaternion `[x, y, z, w]`.
    Trs {
        translation: [f32; 3],
        rotation: [f32; 4],
        scale: [f32; 3],
    },
}

impl Rest {
    /// The map as an affine transform: a [`Rest::Trs`] composed in f64 and rounded once per entry.
    pub fn matrix(&self) -> Transform {
        match *self {
            Rest::Affine(m) => m,
            Rest::Trs {
                translation,
                rotation,
                scale,
            } => {
                let [x, y, z, w] = rotation.map(f64::from);
                // A unit quaternion rounded to f32 is not exactly unit (the f32 nearest 1/sqrt(2) twice has norm squared
                // 1 - 3.4e-8), and the columns below are those of a unit one: made unit first, so the matrix is the
                // rotation the quaternion stands for and a scale read back from its columns is the scale stored. A zero
                // or non-finite quaternion is left as it is (a rest the verifier refuses).
                let norm = (x * x + y * y + z * z + w * w).sqrt();
                let [x, y, z, w] = if norm > 0.0 && norm.is_finite() {
                    [x / norm, y / norm, z / norm, w / norm]
                } else {
                    [x, y, z, w]
                };
                // the rotation's columns (images of x, y, z)
                let r = [
                    [
                        1.0 - 2.0 * (y * y + z * z),
                        2.0 * (x * y + z * w),
                        2.0 * (x * z - y * w),
                    ],
                    [
                        2.0 * (x * y - z * w),
                        1.0 - 2.0 * (x * x + z * z),
                        2.0 * (y * z + x * w),
                    ],
                    [
                        2.0 * (x * z + y * w),
                        2.0 * (y * z - x * w),
                        1.0 - 2.0 * (x * x + y * y),
                    ],
                ];
                let mut cols = [[0.0f32; 3]; 4];
                for (j, col) in r.iter().enumerate() {
                    cols[j] = col.map(|v| (v * f64::from(scale[j])) as f32);
                }
                cols[3] = translation;
                Transform { cols }
            }
        }
    }

    /// The rest with both its sides converted from `frame` into the world's (`C M C^-1`, [`SourceFrame::transform`]).
    /// Exact: a frame is a signed permutation of the axes with a unit scale. A rotation conjugated by a signed
    /// permutation `C` stays a rotation whose quaternion is `(det(C) C v, w)`; a scale's axes are permuted, the frame's
    /// signs cancelling (`C diag(s) C^-1`) and the scale's own kept (a mirrored joint stays mirrored); a translation
    /// converts as a point.
    pub fn converted(&self, frame: &SourceFrame) -> Rest {
        match *self {
            Rest::Affine(m) => Rest::Affine(frame.transform(&m)),
            Rest::Trs {
                translation,
                rotation,
                scale,
            } => {
                let v = frame.direction([rotation[0], rotation[1], rotation[2]]);
                let v = if frame.determinant() < 0 { v.map(|x| -x) } else { v };
                // the frame's sign on each axis cancels: direction() applies it once, the axes' own signs once more
                let signs = frame.direction([1.0; 3]);
                let d = frame.direction(scale);
                let s = [d[0] * signs[0], d[1] * signs[1], d[2] * signs[2]];
                Rest::Trs {
                    translation: frame.point(translation),
                    rotation: [v[0], v[1], v[2], rotation[3]],
                    scale: s,
                }
            }
        }
    }

    fn values(&self) -> Vec<f32> {
        match self {
            Rest::Affine(m) => m.cols.iter().flatten().copied().collect(),
            Rest::Trs {
                translation,
                rotation,
                scale,
            } => translation.iter().chain(rotation).chain(scale).copied().collect(),
        }
    }
}

/// One joint of a skeleton.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Joint {
    pub id: JointId,
    /// The source's name for it: provenance, not identity.
    pub name: String,
    /// The joint whose space its rest is in; `None` for a root.
    pub parent: Option<JointId>,
    pub rest: Rest,
}

/// `core.skeleton` (see the module documentation).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Skeleton {
    pub joints: Vec<Joint>,
}

/// How far a [`Rest::Trs`] rotation's length may be from 1: sources store unit quaternions rounded to f32 (or
/// quantised); a length further off is not a rotation.
const UNIT_TOLERANCE: f32 = 1e-3;

impl Skeleton {
    /// The position of joint `id` in [`Skeleton::joints`].
    pub fn index_of(&self, id: JointId) -> Option<usize> {
        self.joints.iter().position(|j| j.id == id)
    }

    /// The skeleton with every rest converted from `frame` into the world's ([`Rest::converted`]): for a reader whose
    /// whole skeleton is in one source frame.
    pub fn converted(&self, frame: &SourceFrame) -> Skeleton {
        Skeleton {
            joints: self
                .joints
                .iter()
                .map(|j| Joint {
                    rest: j.rest.converted(frame),
                    ..j.clone()
                })
                .collect(),
        }
    }
}

impl Component for Skeleton {
    const NAME: &'static str = "core.skeleton";
    /// A definition: no mesh of its entity's is described by it.
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// Ids unique; each parent a joint of the skeleton other than itself; no cycle through parents; every rest finite;
    /// a TRS rotation of unit length (within [`UNIT_TOLERANCE`]).
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        let mut parents = BTreeMap::new();
        for j in &self.joints {
            if parents.insert(j.id, j.parent).is_some() {
                return Err(format!("joint {:?} listed twice", j.id));
            }
            if let Some(i) = j.rest.values().iter().position(|x| !x.is_finite()) {
                return Err(format!("joint {:?}: rest value {i} is not finite", j.id));
            }
            if let Rest::Trs { rotation, .. } = j.rest {
                let len = rotation.iter().map(|x| x * x).sum::<f32>().sqrt();
                if (len - 1.0).abs() > UNIT_TOLERANCE {
                    return Err(format!(
                        "joint {:?}: rotation of length {len}, not a unit quaternion",
                        j.id
                    ));
                }
            }
        }
        for j in &self.joints {
            match j.parent {
                Some(p) if p == j.id => return Err(format!("joint {:?} is its own parent", j.id)),
                Some(p) if !parents.contains_key(&p) => {
                    return Err(format!(
                        "joint {:?}: parent {p:?} is not a joint of the skeleton",
                        j.id
                    ));
                }
                _ => {}
            }
            // up the parents: a cycle comes back to a joint already seen
            let mut seen = BTreeSet::from([j.id]);
            let mut at = j.parent;
            while let Some(p) = at {
                if !seen.insert(p) {
                    return Err(format!("joint {:?}: its parents form a cycle", j.id));
                }
                at = parents.get(&p).copied().flatten();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Axis;

    const S: f32 = std::f32::consts::FRAC_1_SQRT_2;

    fn joint(id: u32, parent: Option<u32>, rest: Rest) -> Joint {
        Joint {
            id: JointId(id),
            name: format!("j{id}"),
            parent: parent.map(JointId),
            rest,
        }
    }

    fn trs(translation: [f32; 3], rotation: [f32; 4], scale: [f32; 3]) -> Rest {
        Rest::Trs {
            translation,
            rotation,
            scale,
        }
    }

    fn errors(s: Skeleton) -> Vec<String> {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, s);
        w.verify_components().into_iter().map(|(_, _, m)| m).collect()
    }

    /// Hand-worked: T(1, 2, 3) R(90 degrees about z) S(2, 1, 1). x scales to (2, 0, 0) then turns to (0, 2, 0); y turns
    /// to (-1, 0, 0); z stays.
    #[test]
    fn trs_composes_translation_rotation_scale() {
        let m = trs([1.0, 2.0, 3.0], [0.0, 0.0, S, S], [2.0, 1.0, 1.0]).matrix();
        let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6);
        assert!(near(m.cols[0], [0.0, 2.0, 0.0]), "{m:?}");
        assert!(near(m.cols[1], [-1.0, 0.0, 0.0]), "{m:?}");
        assert!(near(m.cols[2], [0.0, 0.0, 1.0]), "{m:?}");
        assert_eq!(m.cols[3], [1.0, 2.0, 3.0]);
    }

    /// A unit quaternion rounded to f32 is not exactly unit: (0, 0, S, S) with S the f32 nearest 1/sqrt(2) has norm squared
    /// 1 - 3.4e-8. The matrix is that of the rotation the quaternion stands for, so its columns are of length 1 (the matrix of
    /// the quaternion taken as unit had columns 3.4e-8 short), and a scale stored as 1 is read back from them as 1.
    #[test]
    fn the_matrix_of_a_quaternion_rounded_to_f32_has_columns_of_the_scale_stored() {
        let m = trs([0.0; 3], [0.0, 0.0, S, S], [1.0, 1.0, 1.0]).matrix();
        for c in &m.cols[..3] {
            let len = c.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>().sqrt();
            assert!((len - 1.0).abs() < 1e-9, "{c:?} has length {len}");
        }
        let scaled = trs([0.0; 3], [0.0, 0.0, S, S], [2.0, 1.0, 3.0]).matrix();
        for (c, scale) in scaled.cols[..3].iter().zip([2.0, 1.0, 3.0]) {
            let len = c.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>().sqrt();
            assert!((len - scale).abs() < 1e-9, "{c:?} has length {len}, not {scale}");
        }
    }

    /// Hand-worked frame conversions, both of a proper and of a mirroring frame:
    /// - ENU (world x = x, y = z, z = -y; det +1): 90 degrees about the source's z (x to y) is 90 degrees about world
    ///   +y (world x to world -z, which is the source's y): quaternion (0, s, 0, s); (1, 2, 3) is (1, 3, -2).
    /// - (x, y, -z) (det -1): 90 degrees about x (y to z) becomes y to -z, -90 degrees about x: (-s, 0, 0, s); about
    ///   z it stays (0, 0, s, s).
    ///
    /// Each agrees with converting the rest's matrix.
    #[test]
    fn frame_conversion_is_exact_for_proper_and_mirroring_frames() {
        let enu = SourceFrame::ENU;
        let r = trs([1.0, 2.0, 3.0], [0.0, 0.0, S, S], [1.0, 2.0, 3.0]).converted(&enu);
        assert_eq!(r, trs([1.0, 3.0, -2.0], [0.0, S, 0.0, S], [1.0, 3.0, 2.0]));
        let mirror = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.0);
        assert_eq!(
            trs([0.0; 3], [S, 0.0, 0.0, S], [1.0; 3]).converted(&mirror),
            trs([0.0; 3], [-S, 0.0, 0.0, S], [1.0; 3])
        );
        assert_eq!(
            trs([0.0; 3], [0.0, 0.0, S, S], [1.0; 3]).converted(&mirror),
            trs([0.0; 3], [0.0, 0.0, S, S], [1.0; 3])
        );
        // a mirrored joint stays mirrored: its scale's own signs kept, only its axes permuted (the world frame changes
        // nothing; ENU swaps y and z)
        let mirrored = trs([0.0; 3], [0.0, 0.0, 0.0, 1.0], [-2.0, 3.0, 4.0]);
        assert_eq!(mirrored.converted(&SourceFrame::WORLD), mirrored);
        assert_eq!(
            mirrored.converted(&enu),
            trs([0.0; 3], [0.0, 0.0, 0.0, 1.0], [-2.0, 4.0, 3.0])
        );
        for frame in [SourceFrame::WORLD, enu, mirror] {
            for rest in [
                trs([1.0, 2.0, 3.0], [0.0, 0.0, S, S], [1.0, 2.0, 3.0]),
                trs([0.5, -1.0, 2.0], [S, 0.0, 0.0, S], [2.0, 1.0, 1.0]),
                trs([0.5, -1.0, 2.0], [S, 0.0, 0.0, S], [-2.0, 3.0, 4.0]),
                trs([1.0, 2.0, 3.0], [0.0, 0.0, S, S], [2.0, -1.0, -0.5]),
            ] {
                let a = rest.converted(&frame).matrix();
                let b = frame.transform(&rest.matrix());
                let near = a
                    .cols
                    .iter()
                    .flatten()
                    .zip(b.cols.iter().flatten())
                    .all(|(x, y)| (x - y).abs() < 1e-6);
                assert!(near, "{frame:?}: {a:?} vs {b:?}");
            }
        }
        // an affine rest converts as a transform, and a skeleton converts every rest, ids and parents unchanged
        let s = Skeleton {
            joints: vec![
                joint(7, None, Rest::Affine(Transform::IDENTITY)),
                joint(3, Some(7), trs([1.0, 2.0, 3.0], [0.0, 0.0, 0.0, 1.0], [1.0; 3])),
            ],
        };
        let c = s.converted(&enu);
        assert_eq!(c.joints[0].rest, Rest::Affine(Transform::IDENTITY));
        assert_eq!(
            (c.joints[1].id, c.joints[1].parent),
            (JointId(3), Some(JointId(7)))
        );
        assert_eq!(c.index_of(JointId(3)), Some(1));
    }

    /// A skeleton in the source's order (a parent after its child) verifies; duplicate ids, a missing or own parent,
    /// a cycle, a non-finite rest and a rotation that is not a unit quaternion each fail, naming the joint.
    #[test]
    fn malformed_skeletons_fail_verification() {
        let id = Rest::Affine(Transform::IDENTITY);
        let unit = [0.0, 0.0, 0.0, 1.0];
        assert!(
            errors(Skeleton {
                joints: vec![joint(2, Some(1), id), joint(1, None, id)],
            })
            .is_empty()
        );
        let fails = |joints: Vec<Joint>, what: &str| {
            let e = errors(Skeleton { joints });
            assert!(e.iter().any(|m| m.contains(what)), "{what}: {e:?}");
        };
        fails(vec![joint(1, None, id), joint(1, None, id)], "listed twice");
        fails(vec![joint(1, Some(9), id)], "is not a joint");
        fails(vec![joint(1, Some(1), id)], "its own parent");
        fails(vec![joint(1, Some(2), id), joint(2, Some(1), id)], "cycle");
        fails(
            vec![joint(1, None, trs([f32::NAN, 0.0, 0.0], unit, [1.0; 3]))],
            "not finite",
        );
        fails(
            vec![joint(1, None, trs([0.0; 3], [0.0, 0.0, 0.0, 2.0], [1.0; 3]))],
            "not a unit quaternion",
        );
    }
}
