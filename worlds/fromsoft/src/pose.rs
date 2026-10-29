//! Pure pose arithmetic shared by the reader and the passes, with no I/O: they both use it and never each other.
//! The source frame, placements' points, a FLVER skeleton's bone matrices (static posing) and Havok body poses.
use fromsoft_formats::affine::{Matrix, affine_point, affine_vector, compose};
use fromsoft_formats::flver::{Bone, Model};
use primase_core::Transform;
use primase_core::frame::{Axis, SourceFrame};

/// FromSoft's source frame: x, y and z negated, in metres.
pub(crate) const FRAME: SourceFrame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.);

/// A place a file states in its own frame (columns, as [`fromsoft_formats::msb::euler_matrix`] gives them) in the
/// world frame: the frame's handedness conversion.
pub(crate) fn placed(cols: [[f32; 3]; 4]) -> Transform {
    FRAME.transform(&Transform { cols })
}

pub(crate) const ID: Matrix = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0.; 3]];
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

/// `p` in the space of the affine map `m`: the point whose image under `m` is `p`. `None` where `m` has no inverse (a
/// bone scaled to nothing).
pub(crate) fn inverse_point(m: &Matrix, p: [f64; 3]) -> Option<[f64; 3]> {
    let [a, b, c, t] = *m;
    let cross = |u: [f64; 3], v: [f64; 3]| {
        [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ]
    };
    let dot = |u: [f64; 3], v: [f64; 3]| u[0] * v[0] + u[1] * v[1] + u[2] * v[2];
    let det = dot(a, cross(b, c));
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    let d = [p[0] - t[0], p[1] - t[1], p[2] - t[2]];
    Some([
        dot(cross(b, c), d) / det,
        dot(cross(c, a), d) / det,
        dot(cross(a, b), d) / det,
    ])
}

/// `p` as a direction in the space of the affine map `m` (its translation ignored). `None` where `m` has no inverse.
fn inverse_vector(m: &Matrix, v: [f64; 3]) -> Option<[f64; 3]> {
    inverse_point(&[m[0], m[1], m[2], [0.; 3]], v)
}

fn unit(v: [f64; 3]) -> Option<[f64; 3]> {
    let length = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    (length.is_finite() && length > 1e-9).then(|| v.map(|x| x / length))
}

fn cross3(u: [f64; 3], v: [f64; 3]) -> [f64; 3] {
    [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ]
}

/// The rotation of a dummy whose stored forward and upward directions are `forward` and `upward`, as the columns of its
/// local X, Y and Z axes: forward along -Z and upward along +Y (a convention: the files do not say which local axis a
/// socket faces along), X completing the right-handed set. Most dummies store the two nearly perpendicular; 164 of
/// Sekiro's and 4,216 of Elden Ring's are further off (the cosine above 0.01, up to 0.70). The stored vectors are not
/// unit length (the censuses found lengths of 0.057 to 0.3, the dummies' scale), so both are normalised first and the
/// second made exactly perpendicular to the first. `None` where either has no direction (zero or not finite) or they are parallel.
pub(crate) fn dummy_axes(forward: [f64; 3], upward: [f64; 3]) -> Option<[[f64; 3]; 3]> {
    let z = unit(forward)?.map(|x| -x);
    let u = unit(upward)?;
    let along = u.iter().zip(z).map(|(a, b)| a * b).sum::<f64>();
    let y = unit(std::array::from_fn(|i| u[i] - along * z[i]))?;
    Some([cross3(y, z), y, z])
}

/// Where a dummy polygon stands in its model's space at rest, as an affine map in the file's frame: its position taken
/// in the space of its parent bone (`worlds`, [`bone_matrices`]) where it has one, and the rotation of its frame
/// ([`dummy_axes`], through the same bone) where it uses its upward vector and the directions have one, else the
/// identity (as [`dummy_sockets`] takes it). `None` where it names a bone the model lacks or the position is not finite.
pub(crate) fn dummy_in_model(worlds: &[Matrix], d: &fromsoft_formats::flver::Dummy) -> Option<Matrix> {
    if d.position.iter().any(|x| !x.is_finite()) {
        return None;
    }
    let parent = usize::try_from(d.parent_bone).ok();
    let at = d.position.map(f64::from);
    let t = match parent {
        Some(p) => affine_point(worlds.get(p)?, at),
        None if d.parent_bone == -1 => at,
        None => return None,
    };
    let direction = |v: [f32; 3]| {
        let v = v.map(f64::from);
        match parent {
            Some(p) => Some(affine_vector(worlds.get(p)?, v)),
            None => Some(v),
        }
    };
    let axes = d
        .use_upward_vector
        .then(|| dummy_axes(direction(d.forward)?, direction(d.upward)?))
        .flatten();
    let [x, y, z] = axes.unwrap_or([[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]);
    Some([x, y, z, t])
}

/// The name of the socket a dummy polygon with reference id `reference_id` is (what an animation event that names a dummy
/// polygon finds it by).
pub(crate) fn dummy_socket_name(reference_id: i32) -> String {
    format!("dummy {reference_id}")
}

/// A model's dummy polygons as sockets on its skeleton (`core.sockets`; joint `i` is bone `i`, see [`skeleton`]), each
/// where the dummy stands in the model at rest and following the bone it is attached to: the dummy's position is taken
/// in the space of its parent bone where it has one (SoulsFormatsNEXT: "initially transformed to" it) and in the
/// model's where it has none; the socket's joint is its attach bone, else its parent bone. The socket's rotation is the
/// dummy's frame ([`dummy_axes`]: forward along -Z, upward along +Y, both normalised) where the dummy uses its upward
/// vector, the directions taken in the same space as the position and then in the joint's; where it does not, or a
/// direction has none, the rotation is the identity. The directions' stored lengths are not part of the socket (they
/// stay in `fromsoft.dummies`, as stored). Named `dummy <reference id>`; the ids number the sockets in file order.
///
/// Also the indices of the dummies that were left out: one that names a bone the model does not have, or no bone at all
/// to follow, or a bone with no inverse, or a position that is not finite.
pub(crate) fn dummy_sockets(model: &Model) -> Result<(primase_core::Sockets, Vec<usize>), String> {
    dummy_sockets_numbered(model, |id| id)
}

/// [`dummy_sockets`] with each socket named by `number(reference id)`: a held weapon's polygons are named by the global ids
/// animation events use (`HandSlot::global_id`), not by the number in the weapon's own model.
pub(crate) fn dummy_sockets_numbered(
    model: &Model,
    number: impl Fn(i32) -> i32,
) -> Result<(primase_core::Sockets, Vec<usize>), String> {
    let worlds = bone_matrices(&model.bones)?;
    let bone = |i: i16| usize::try_from(i).ok();
    let mut sockets = Vec::new();
    let mut left_out = Vec::new();
    for (i, d) in model.dummies.iter().enumerate() {
        let parent = bone(d.parent_bone);
        let attach = bone(d.attach_bone);
        let placed = (|| {
            if d.position.iter().any(|x| !x.is_finite()) {
                return None;
            }
            let at = d.position.map(f64::from);
            let in_model = match parent {
                Some(p) => affine_point(worlds.get(p)?, at),
                // -1: the dummy is already in the model's space; another negative number is no bone
                None if d.parent_bone == -1 => at,
                None => return None,
            };
            let joint = attach.or(parent).filter(|&j| j < worlds.len())?;
            let [x, y, z] = inverse_point(&worlds[joint], in_model)?;
            // the directions are in the position's space; into the model's, then the joint's
            let axes = d
                .use_upward_vector
                .then(|| {
                    let direction = |v: [f32; 3]| {
                        let v = v.map(f64::from);
                        let in_model = match parent {
                            Some(p) => affine_vector(worlds.get(p)?, v),
                            None => v,
                        };
                        inverse_vector(&worlds[joint], in_model)
                    };
                    dummy_axes(direction(d.forward)?, direction(d.upward)?)
                })
                .flatten();
            Some((joint, [x as f32, y as f32, z as f32], axes))
        })();
        let Some((joint, t, axes)) = placed else {
            left_out.push(i);
            continue;
        };
        let [ax, ay, az] = axes.unwrap_or([[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]);
        let offset = primase_core::Rest::Affine(Transform {
            cols: [
                ax.map(|c| c as f32),
                ay.map(|c| c as f32),
                az.map(|c| c as f32),
                t,
            ],
        })
        .converted(&FRAME)
        .matrix();
        sockets.push(primase_core::Socket {
            id: primase_core::SocketId(i as u32),
            name: dummy_socket_name(number(i32::from(d.reference_id))),
            joint: primase_core::JointId(joint as u32),
            offset,
        });
    }
    Ok((primase_core::Sockets(sockets), left_out))
}

/// The bone a static mesh's vertices follow: each its own, named by its NORMAL.w, when the mesh's layout stores a
/// normal W ([`Model::stores_normal_w`]); else every vertex the mesh's node (its `NodeIndex`), when that names one of
/// the model's bones; else none (`Unbound`, with the node as stored: -1 or past the bones; Elden Ring's files have
/// such meshes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StaticBones {
    PerVertex,
    Node(usize),
    Unbound(i32),
}

impl StaticBones {
    /// The bone vertex `v` follows (by NORMAL.w it may name none; an unbound mesh's vertices follow none).
    pub(crate) fn of(self, v: &fromsoft_formats::flver::Vertex) -> Option<usize> {
        match self {
            StaticBones::PerVertex => usize::try_from(v.normal_w).ok(),
            StaticBones::Node(bone) => Some(bone),
            StaticBones::Unbound(_) => None,
        }
    }
}

/// How static mesh `mesh` of `model` names its bones ([`StaticBones`]).
pub(crate) fn static_bones(model: &Model, mesh: usize) -> StaticBones {
    if model.stores_normal_w(mesh) {
        return StaticBones::PerVertex;
    }
    let node = model.meshes.get(mesh).map_or(-1, |m| m.node);
    usize::try_from(node)
        .ok()
        .filter(|&b| b < model.bones.len())
        .map_or(StaticBones::Unbound(node), StaticBones::Node)
}

/// Every vertex of every mesh in model space (file frame): static vertices posed by their bone ([`StaticBones`]), as
/// [`lower_model`] poses them but whatever their weights; dynamic ones, and static ones whose bone is not one of the
/// model's, as stored. For measurements a reader makes before any pass runs (a model's extent), not a lowering.
pub fn posed_positions(source: &Model) -> Result<Vec<Vec<[f32; 3]>>, String> {
    let worlds = bone_matrices(&source.bones)?;
    Ok(source
        .meshes
        .iter()
        .enumerate()
        .map(|(index, m)| {
            let bones = (!m.dynamic).then(|| static_bones(source, index));
            m.vertices
                .iter()
                .map(|v| {
                    let matrix = bones
                        .and_then(|b| b.of(v))
                        .and_then(|i| worlds.get(i))
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
            child: -1,
            next_sibling: -1,
            previous_sibling: -1,
            bounds_min: [f32::MAX; 3],
            bounds_max: [-f32::MAX; 3],
            flags: 0,
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

    fn dummy(position: [f32; 3], parent_bone: i16, attach_bone: i16) -> fromsoft_formats::flver::Dummy {
        fromsoft_formats::flver::Dummy {
            position,
            forward: [0.0, 0.0, 1.0],
            upward: [0.0, 1.0, 0.0],
            reference_id: 120,
            parent_bone,
            attach_bone,
            color: [255; 4],
            flag1: false,
            use_upward_vector: false,
            unk30: 0,
            unk34: 0,
        }
    }

    /// A dummy is a socket on the bone it follows, at the place it stands: in its parent bone's space where it has a
    /// parent, in the model's where the parent is -1; one naming a bone the model lacks, or following none, is left out.
    #[test]
    fn a_dummy_is_a_socket_on_the_bone_it_follows_at_the_place_it_stands() {
        let flat = |name: &str, translation: [f32; 3], parent: i16| Bone {
            name: name.into(),
            translation,
            rotation: [0.0; 3],
            scale: [1.0; 3],
            parent,
            child: -1,
            next_sibling: -1,
            previous_sibling: -1,
            bounds_min: [f32::MAX; 3],
            bounds_max: [-f32::MAX; 3],
            flags: 0,
        };
        let model = Model {
            bones: vec![
                flat("root", [1.0, 0.0, 0.0], -1),
                flat("hand", [0.0, 2.0, 0.0], 0),
            ],
            dummies: vec![
                // the hand's own origin, which stands at (1, 2, 0) in the model
                dummy([0.0; 3], 1, 1),
                // a point of the model's space, following the hand
                dummy([5.0, 5.0, 5.0], -1, 1),
                // a parent the model does not have
                dummy([0.0; 3], 7, 1),
                // no bone to follow
                dummy([0.0; 3], -1, -1),
                // a position that is not a number
                dummy([f32::NAN, 0.0, 0.0], 1, 1),
            ],
            version: 0x2001a,
            speedtree: false,
            layouts: vec![],
            materials: vec![],
            meshes: vec![],
        };
        let (sockets, left_out) = dummy_sockets(&model).unwrap();
        assert_eq!(left_out, [2, 3, 4]);
        assert_eq!(sockets.0.len(), 2);
        let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
        let (first, second) = (&sockets.0[0], &sockets.0[1]);
        assert_eq!(
            (first.id, first.joint, first.name.as_str()),
            (primase_core::SocketId(0), primase_core::JointId(1), "dummy 120")
        );
        assert!(near(first.offset.cols[3], [0.0; 3]), "{:?}", first.offset);
        // (5, 5, 5) is (4, 3, 5) from the hand in the file's frame, (4, 3, -5) in the world's
        assert_eq!(
            (second.id, second.joint),
            (primase_core::SocketId(1), primase_core::JointId(1))
        );
        assert!(
            near(second.offset.cols[3], [4.0, 3.0, -5.0]),
            "{:?}",
            second.offset
        );
    }

    /// Directions the file stores at 0.05 to 0.3 of a unit give the same frame as unit ones: forward along -Z, upward
    /// along +Y, the frame orthonormal; a direction with none (zero, not a number) or parallel ones give none; an
    /// upward slightly off perpendicular (as most of the install's are) is made exactly perpendicular.
    #[test]
    fn a_dummys_frame_is_built_from_normalised_directions() {
        let near = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-12);
        // forward along +X at length 0.5, upward along +Y at length 0.3: Z = -forward, X = Y x Z
        let axes = dummy_axes([0.5, 0.0, 0.0], [0.0, 0.3, 0.0]).unwrap();
        assert!(
            near(axes[0], [0., 0., 1.]) && near(axes[1], [0., 1., 0.]) && near(axes[2], [-1., 0., 0.]),
            "{axes:?}"
        );
        // the install's own convention, forward -Z and upward +Y at the stored 0.196, is the identity
        let identity = dummy_axes([0.0, 0.0, -0.196], [0.0, 0.196, 0.0]).unwrap();
        assert!(
            near(identity[0], [1., 0., 0.])
                && near(identity[1], [0., 1., 0.])
                && near(identity[2], [0., 0., 1.])
        );
        // 0.0006 off perpendicular, 0.057 long: still orthonormal
        let off = dummy_axes([0.0, 0.0, -0.057], [0.0, 0.057, 0.000_034]).unwrap();
        let dot = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>();
        for i in 0..3 {
            assert!((dot(off[i], off[i]) - 1.0).abs() < 1e-12, "{off:?}");
            for j in 0..i {
                assert!(dot(off[i], off[j]).abs() < 1e-12, "{off:?}");
            }
        }
        assert!(dummy_axes([0.0; 3], [0.0, 1.0, 0.0]).is_none());
        assert!(dummy_axes([f64::NAN, 0.0, 1.0], [0.0, 1.0, 0.0]).is_none());
        assert!(dummy_axes([0.0, 0.0, 1.0], [0.0, 0.0, 3.0]).is_none());
    }

    /// A dummy that uses its upward vector is a socket with the rotation of its frame, from non-unit stored directions;
    /// one that does not keeps the identity rotation.
    #[test]
    fn a_dummy_using_its_upward_vector_has_the_rotation_of_its_frame() {
        let bone = Bone {
            name: "root".into(),
            translation: [0.0; 3],
            rotation: [0.0; 3],
            scale: [1.0; 3],
            parent: -1,
            child: -1,
            next_sibling: -1,
            previous_sibling: -1,
            bounds_min: [f32::MAX; 3],
            bounds_max: [-f32::MAX; 3],
            flags: 0,
        };
        let mut turned = dummy([0.0; 3], -1, 0);
        turned.forward = [0.5, 0.0, 0.0];
        turned.upward = [0.0, 0.3, 0.0];
        turned.use_upward_vector = true;
        let mut unused = turned.clone();
        unused.use_upward_vector = false;
        let model = Model {
            bones: vec![bone],
            dummies: vec![turned, unused],
            version: 0x2001a,
            speedtree: false,
            layouts: vec![],
            materials: vec![],
            meshes: vec![],
        };
        let (sockets, left_out) = dummy_sockets(&model).unwrap();
        assert!(left_out.is_empty());
        let cols = |i: usize| sockets.0[i].offset.cols;
        let length = |v: [f32; 3]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
        let dot = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        let turned = cols(0);
        for i in 0..3 {
            assert!((length(turned[i]) - 1.0).abs() < 1e-5, "{turned:?}");
            for j in 0..i {
                assert!(dot(turned[i], turned[j]).abs() < 1e-5, "{turned:?}");
            }
        }
        assert_ne!(
            turned[..3],
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            "{turned:?}"
        );
        assert_eq!(cols(1)[..3], [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    }

    /// The point whose image under an affine map is a given point; none where the map is singular.
    #[test]
    fn a_point_is_found_in_the_space_of_a_map_that_has_an_inverse() {
        let m = compose(&local(&bone(-1, 2.0, 0.4)), &local(&bone(-1, -1.0, -0.3)));
        let p = [0.3, -1.2, 2.5];
        let image = affine_point(&m, p);
        let back = inverse_point(&m, image).unwrap();
        assert!(back.iter().zip(p).all(|(a, b)| (a - b).abs() < 1e-9), "{back:?}");
        assert!(inverse_point(&[[1., 0., 0.], [0., 0., 0.], [0., 0., 1.], [0.; 3]], p).is_none());
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
