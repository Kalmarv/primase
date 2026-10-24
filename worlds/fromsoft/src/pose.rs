//! Pure pose arithmetic shared by the reader and the passes, with no I/O: they both use it and never each other.
//! The source frame, placements' points, a FLVER skeleton's bone matrices (static posing) and Havok body poses.
use crate::flver::{Bone, Model};
use primase_core::Transform;
use primase_core::frame::{Axis, SourceFrame};

/// FromSoft's source frame: x, y and z negated, in metres.
pub(crate) const FRAME: SourceFrame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.);

pub(crate) type Matrix = [[f64; 3]; 4];
pub(crate) const ID: Matrix = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0.; 3]];
pub(crate) fn affine_vector(m: &Matrix, p: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|a| (0..3).map(|i| m[i][a] * p[i]).sum())
}
pub(crate) fn affine_point(m: &Matrix, p: [f64; 3]) -> [f64; 3] {
    let v = affine_vector(m, p);
    std::array::from_fn(|i| v[i] + m[3][i])
}
pub(crate) fn compose(parent: &Matrix, local: &Matrix) -> Matrix {
    [
        affine_vector(parent, local[0]),
        affine_vector(parent, local[1]),
        affine_vector(parent, local[2]),
        affine_point(parent, local[3]),
    ]
}
fn local(b: &Bone) -> Matrix {
    let [x, y, z] = b.rotation.map(f64::from);
    let (sx, cx) = x.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sz, cz) = z.sin_cos();
    let rx = [[1., 0., 0.], [0., cx, sx], [0., -sx, cx], [0.; 3]];
    let rz = [[cz, sz, 0.], [-sz, cz, 0.], [0., 0., 1.], [0.; 3]];
    let ry = [[cy, 0., -sy], [0., 1., 0.], [sy, 0., cy], [0.; 3]];
    let mut m = compose(&ry, &compose(&rz, &rx));
    for (col, scale) in m[..3].iter_mut().zip(b.scale) {
        for x in col {
            *x *= f64::from(scale);
        }
    }
    m[3] = b.translation.map(f64::from);
    m
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(x, y)| x * y).sum()
}
pub(crate) fn normal_matrix(m: &Matrix) -> Result<(Matrix, f64), String> {
    let cols = [cross(m[1], m[2]), cross(m[2], m[0]), cross(m[0], m[1])];
    let det = dot(m[0], cols[0]);
    if det == 0. || !det.is_finite() {
        return Err("singular rigid bone transform".into());
    }
    Ok((
        [
            cols[0].map(|x| x / det),
            cols[1].map(|x| x / det),
            cols[2].map(|x| x / det),
            [0.; 3],
        ],
        det,
    ))
}
/// A FLVER's bones as a `core.skeleton` in the world frame: joint `i` is bone `i` with its stored name and parent
/// (a negative parent: a root), its rest the bone's local transform as [`bone_matrices`] composes it (scale, then
/// rotation about X, Z, Y in that order, then translation), converted from the file's frame exactly
/// ([`primase_core::Rest::converted`]).
pub fn skeleton(model: &Model) -> primase_core::Skeleton {
    primase_core::Skeleton {
        joints: model
            .bones
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let m = local(b);
                let rest = primase_core::Rest::Affine(Transform {
                    cols: m.map(|c| c.map(|x| x as f32)),
                });
                primase_core::Joint {
                    id: primase_core::JointId(i as u32),
                    name: b.name.clone(),
                    parent: u32::try_from(b.parent).ok().map(primase_core::JointId),
                    rest: rest.converted(&FRAME),
                }
            })
            .collect(),
    }
}

/// Each bone's model-space matrix, through its parents: its parent's matrix (made once, whichever order the bones
/// come in) composed with its own, so a chain of any depth costs one composition a bone.
pub(crate) fn bone_matrices(bones: &[Bone]) -> Result<Vec<Matrix>, String> {
    #[derive(Clone, Copy)]
    enum State {
        Unseen,
        /// On the chain being resolved: meeting it again is a loop.
        Open,
        Done(Matrix),
    }
    let mut state = vec![State::Unseen; bones.len()];
    let mut chain = Vec::new();
    for i in 0..bones.len() {
        // up from `i` to a bone already made or a root
        let mut at = i;
        let mut above = None;
        loop {
            match state[at] {
                State::Done(m) => {
                    above = Some(m);
                    break;
                }
                State::Open => return Err(format!("invalid rigid bone hierarchy at {i}")),
                State::Unseen => {}
            }
            state[at] = State::Open;
            chain.push(at);
            let parent = bones[at].parent;
            if parent < 0 {
                break;
            }
            let parent = parent as usize;
            if parent >= bones.len() {
                return Err(format!("invalid rigid bone hierarchy at {i}"));
            }
            at = parent;
        }
        // and back down
        while let Some(bone) = chain.pop() {
            let m = match above {
                Some(parent) => compose(&parent, &local(&bones[bone])),
                None => local(&bones[bone]),
            };
            state[bone] = State::Done(m);
            above = Some(m);
        }
    }
    state
        .into_iter()
        .enumerate()
        .map(|(i, s)| match s {
            State::Done(m) => Ok(m),
            State::Unseen | State::Open => Err(format!("invalid rigid bone hierarchy at {i}")),
        })
        .collect()
}

/// Every vertex of every mesh in model space (file frame): static vertices posed by the bone their NORMAL.w names,
/// as [`lower_model`] poses them but whatever their weights; dynamic and unbound ones as stored. For measurements a
/// reader makes before any pass runs (a model's extent), not a lowering.
pub fn posed_positions(source: &Model) -> Result<Vec<Vec<[f32; 3]>>, String> {
    let worlds = bone_matrices(&source.bones)?;
    Ok(source
        .meshes
        .iter()
        .map(|m| {
            m.vertices
                .iter()
                .map(|v| {
                    let matrix = (!m.dynamic)
                        .then(|| usize::try_from(v.normal_w).ok().and_then(|i| worlds.get(i)))
                        .flatten()
                        .filter(|m| **m != ID);
                    match matrix {
                        Some(matrix) => affine_point(matrix, v.position.map(f64::from)).map(|x| x as f32),
                        None => v.position,
                    }
                })
                .collect()
        })
        .collect())
}

/// The model-space box (file frame) of every vertex a face set uses, posed as [`posed_positions`]. `None` for a
/// model whose face sets use no vertex.
pub fn posed_bounds(source: &Model) -> Result<Option<[[f32; 3]; 2]>, String> {
    let posed = posed_positions(source)?;
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for (m, positions) in source.meshes.iter().zip(&posed) {
        for f in &m.faces {
            for &i in &f.indices {
                let Some(p) = positions.get(i as usize).filter(|_| !(f.strip && i == 0xffff)) else {
                    continue;
                };
                for k in 0..3 {
                    min[k] = min[k].min(p[k]);
                    max[k] = max[k].max(p[k]);
                }
            }
        }
    }
    Ok(min[0].is_finite().then_some([min, max]))
}

/// A body pose as the columns of its affine map (rotation by the unit quaternion `q`, then translation by `p`).
pub fn pose_columns(p: [f32; 3], q: [f32; 4]) -> [[f32; 3]; 4] {
    let [x, y, z, w] = q;
    let xx = x * x;
    let yy = y * y;
    let zz = z * z;
    let xy = x * y;
    let xz = x * z;
    let yz = y * z;
    let xw = x * w;
    let yw = y * w;
    let zw = z * w;
    [
        [1. - 2. * (yy + zz), 2. * (xy + zw), 2. * (xz - yw)],
        [2. * (xy - zw), 1. - 2. * (zz + xx), 2. * (yz + xw)],
        [2. * (xz + yw), 2. * (yz - xw), 1. - 2. * (yy + xx)],
        p,
    ]
}

/// `v` through the affine map with columns `c`.
pub fn posed(c: &[[f32; 3]; 4], v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| v[0] * c[0][i] + v[1] * c[1][i] + v[2] * c[2][i] + c[3][i])
}

pub(crate) fn pose(v: [f32; 3], p: [f32; 4], q: [f32; 4]) -> [f32; 3] {
    posed(&pose_columns([p[0], p[1], p[2]], q), v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bone(parent: i16, x: f32, yaw: f32) -> Bone {
        Bone {
            name: String::new(),
            translation: [x, 1.0, -x],
            rotation: [0.0, yaw, 0.1],
            scale: [1.0, 1.5, 1.0],
            parent,
        }
    }

    /// The matrix of a bone by walking up its parents, composing each outside the ones below it.
    fn walked(bones: &[Bone], i: usize) -> Matrix {
        let mut m = local(&bones[i]);
        let mut parent = bones[i].parent;
        while parent >= 0 {
            let p = parent as usize;
            m = compose(&local(&bones[p]), &m);
            parent = bones[p].parent;
        }
        m
    }

    /// Bones in any order (a parent after its child included) get the matrices walking up their parents gives, to
    /// rounding.
    #[test]
    fn a_bones_matrix_is_its_parents_composed_with_its_own_in_any_order() {
        let bones = [
            bone(2, 1.0, 0.3),
            bone(0, 2.0, -0.2),
            bone(-1, 3.0, 0.5),
            bone(1, -1.0, 1.1),
            bone(-1, 0.5, 0.0),
            bone(4, 4.0, 0.7),
        ];
        let made = bone_matrices(&bones).unwrap();
        for (i, m) in made.iter().enumerate() {
            let expected = walked(&bones, i);
            for (a, b) in m.iter().flatten().zip(expected.iter().flatten()) {
                assert!(
                    (a - b).abs() <= 1e-12 * (1.0 + b.abs()),
                    "bone {i}: {a} against {b}"
                );
            }
        }
    }

    /// A chain of every bone a model can hold (an i16 parent) is one composition a bone, not a walk up the whole chain
    /// for each; a loop and a parent past the bones are errors naming the bone.
    #[test]
    fn a_deep_chain_is_linear_and_a_loop_is_an_error() {
        let chain: Vec<Bone> = (0..i16::MAX).map(|i| bone(i - 1, 0.001, 0.0001)).collect();
        assert_eq!(bone_matrices(&chain).unwrap().len(), chain.len());
        let looped = [bone(1, 0.0, 0.0), bone(2, 0.0, 0.0), bone(0, 0.0, 0.0)];
        assert_eq!(
            bone_matrices(&looped).unwrap_err(),
            "invalid rigid bone hierarchy at 0"
        );
        assert!(bone_matrices(&[bone(5, 0.0, 0.0)]).is_err());
        assert!(bone_matrices(&[bone(0, 0.0, 0.0)]).is_err());
    }
}
