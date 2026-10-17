//! `gta-pose-fragments`: a fragment's models drawn at their bones' poses, as the game draws a fragment at rest.
//!
//! The reader keeps each primitive's pose as stored ([`GtaPoses`], rows in GTA's frame). The pass places the posed
//! primitives' positions by the pose, turns their normals and tangents with it ([`Turn`]: as positions under a
//! rotation, normals by the inverse transpose and tangents by the pose at their stored length otherwise, a mirror
//! flipping the handedness and reversing the triangles; a pose with no inverse flattens the model, and its normals
//! and tangents are left out), in GTA's frame (the frame conversion is a signed permutation, so this is bit-exact with posing before it), and removes
//! `gta.poses`. Every other GTA pass on meshes refuses a mesh still holding poses, so this one runs first.
use primase_core::pass::{Pass, PassLog};
use primase_core::{Mesh, Semantic, Stream, World};

use crate::components::GtaPoses;
use crate::region::GTA;

/// A model's transform in GTA's frame: rows `[x, y, z, translation]`.
pub type Pose = [[f32; 4]; 3];

/// How far from orthonormal (the largest entry of `M M^T - I`, `M` a pose's 3x3 part) a pose may be and still turn
/// normals and tangents as it turns positions: within it, `M`'s inverse transpose equals `M` to that order (an
/// approximation). Every pose a model of the install uses is either within 1e-4 or singular (census).
const RIGID: f64 = 1e-4;

/// How a pose turns directions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Turn {
    /// A rotation (orthonormal, determinant 1): normals and tangents as positions, without the translation.
    Rigid,
    /// Any other invertible 3x3 part: normals by its inverse transpose (rows), tangents by it, both kept at their
    /// stored length; a negative determinant (a mirror) flips the tangent's handedness and reverses the triangles.
    General {
        inverse_transpose: [[f64; 3]; 3],
        mirror: bool,
    },
    /// No inverse (determinant 0): the model is flattened, and its normals and tangents are not defined.
    Singular,
}

/// A model's pose and how it turns directions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placing {
    pose: Pose,
    turn: Turn,
}

impl Placing {
    pub fn new(pose: Pose) -> Placing {
        let m: [[f64; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| f64::from(pose[i][j])));
        // cofactors: C[i][j] = m[i+1][j+1] m[i+2][j+2] - m[i+1][j+2] m[i+2][j+1] (indices mod 3); M^-T = C / det
        let c: [[f64; 3]; 3] = std::array::from_fn(|i| {
            std::array::from_fn(|j| {
                let (a, b, x, y) = ((i + 1) % 3, (i + 2) % 3, (j + 1) % 3, (j + 2) % 3);
                m[a][x] * m[b][y] - m[a][y] * m[b][x]
            })
        });
        let det: f64 = (0..3).map(|j| m[0][j] * c[0][j]).sum();
        let scale = m.iter().flatten().fold(0f64, |a, &v| a.max(v.abs()));
        let orth = (0..3)
            .flat_map(|i| (0..3).map(move |j| (i, j)))
            .map(|(i, j)| {
                ((0..3).map(|k| m[i][k] * m[j][k]).sum::<f64>() - f64::from(u8::from(i == j))).abs()
            })
            .fold(0f64, f64::max);
        let turn = if !det.is_finite() || det.abs() <= 1e-9 * scale.powi(3) {
            Turn::Singular
        } else if det > 0.0 && orth <= RIGID {
            Turn::Rigid
        } else {
            Turn::General {
                inverse_transpose: c.map(|row| row.map(|v| v / det)),
                mirror: det < 0.0,
            }
        };
        Placing { pose, turn }
    }

    /// A position.
    fn point(&self, v: [f32; 3]) -> [f32; 3] {
        transform(&self.pose, v, 1.0)
    }

    /// A normal; `None` under a singular pose.
    fn normal(&self, n: [f32; 3]) -> Option<[f32; 3]> {
        match self.turn {
            Turn::Rigid => Some(transform(&self.pose, n, 0.0)),
            Turn::General {
                inverse_transpose, ..
            } => Some(keep_length(n, inverse_transpose.map(|r| dot(r, n)))),
            Turn::Singular => None,
        }
    }

    /// A tangent with its handedness in w; `None` under a singular pose.
    fn tangent(&self, t: [f32; 4]) -> Option<[f32; 4]> {
        let xyz = [t[0], t[1], t[2]];
        match self.turn {
            Turn::Rigid => {
                let d = transform(&self.pose, xyz, 0.0);
                Some([d[0], d[1], d[2], t[3]])
            }
            Turn::General { mirror, .. } => {
                let m = self
                    .pose
                    .map(|r| [f64::from(r[0]), f64::from(r[1]), f64::from(r[2])]);
                let d = keep_length(xyz, m.map(|r| dot(r, xyz)));
                Some([d[0], d[1], d[2], if mirror { -t[3] } else { t[3] }])
            }
            Turn::Singular => None,
        }
    }
}

fn dot(r: [f64; 3], v: [f32; 3]) -> f64 {
    r[0] * f64::from(v[0]) + r[1] * f64::from(v[1]) + r[2] * f64::from(v[2])
}

/// `turned` scaled to the length of `stored` (as it is when it has no length).
fn keep_length(stored: [f32; 3], turned: [f64; 3]) -> [f32; 3] {
    let len = |v: [f64; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let (a, b) = (len(stored.map(f64::from)), len(turned));
    let k = if b > 0.0 { a / b } else { 1.0 };
    turned.map(|v| (v * k) as f32)
}

fn transform(r: &Pose, v: [f32; 3], w: f64) -> [f32; 3] {
    r.map(|row| {
        (f64::from(row[0]) * f64::from(v[0])
            + f64::from(row[1]) * f64::from(v[1])
            + f64::from(row[2]) * f64::from(v[2])
            + f64::from(row[3]) * w) as f32
    })
}

/// `gta-pose-fragments` (see the module documentation).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoseFragments;

impl Pass for PoseFragments {
    fn name(&self) -> &'static str {
        "gta-pose-fragments"
    }
    /// Nothing: a world without fragments has no poses to place.
    fn requires(&self) -> &[&'static str] {
        &[]
    }
    fn removes(&self) -> &[&'static str] {
        &["gta.poses"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let holders: Vec<_> = w.iter::<GtaPoses>().map(|(e, _)| e).collect();
        for e in holders {
            let poses = w.remove::<GtaPoses>(e).expect("poses").0;
            let mut mesh = w
                .remove::<Mesh>(e)
                .ok_or("gta.poses on an entity without core.mesh")?;
            if poses.len() != mesh.primitives().len() {
                return Err(format!(
                    "{e:?}: {} poses for {} primitives",
                    poses.len(),
                    mesh.primitives().len()
                ));
            }
            for (p, pose) in mesh.primitives_mut().iter_mut().zip(poses) {
                let Some(pose) = pose else { continue };
                let t = Placing::new(pose);
                log.count("primitives posed", 1);
                match t.turn {
                    Turn::Rigid => {}
                    Turn::General { mirror, .. } => {
                        log.count("primitives posed by a pose that is not a rotation", 1);
                        if mirror {
                            // a mirror turns every triangle over: reverse them so their fronts stay where they were
                            let n = p.vertex_count() as u32;
                            for tri in p
                                .indices
                                .get_or_insert_with(|| (0..n).collect())
                                .as_chunks_mut::<3>()
                                .0
                            {
                                tri.swap(1, 2);
                            }
                            log.count("primitives posed by a mirroring pose: triangles reversed", 1);
                        }
                    }
                    Turn::Singular => log.count(
                        "primitives posed flat (no inverse): normals and tangents left out",
                        1,
                    ),
                }
                if let Some(Stream::F32x3(v)) = p.streams.get_mut(&Semantic::Position) {
                    for x in v.iter_mut() {
                        *x = GTA.point(t.point(GTA.point_to_source(*x)));
                    }
                }
                if t.turn == Turn::Singular {
                    p.streams.remove(&Semantic::Normal);
                    p.streams.remove(&Semantic::Tangent);
                    continue;
                }
                if let Some(Stream::F32x3(v)) = p.streams.get_mut(&Semantic::Normal) {
                    for x in v.iter_mut() {
                        *x = GTA.direction(t.normal(GTA.direction_to_source(*x)).expect("invertible"));
                    }
                }
                if let Some(Stream::F32x4(v)) = p.streams.get_mut(&Semantic::Tangent) {
                    for x in v.iter_mut() {
                        let s = GTA.direction_to_source([x[0], x[1], x[2]]);
                        let u = t.tangent([s[0], s[1], s[2], x[3]]).expect("invertible");
                        let d = GTA.direction([u[0], u[1], u[2]]);
                        *x = [d[0], d[1], d[2], u[3]];
                    }
                }
            }
            w.insert(e, mesh);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(m: [[f32; 3]; 3]) -> Pose {
        m.map(|r| [r[0], r[1], r[2], 5.0])
    }

    #[test]
    fn poses_turn_normals_by_their_inverse_transpose() {
        // a rotation of 90 degrees about z: rigid, every direction turns as a position does
        let r = Placing::new(pose([[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]));
        assert_eq!(r.turn, Turn::Rigid);
        assert_eq!(r.normal([1.0, 0.0, 0.0]), Some([0.0, 1.0, 0.0]));
        assert_eq!(r.tangent([1.0, 0.0, 0.0, -1.0]), Some([0.0, 1.0, 0.0, -1.0]));
        assert_eq!(r.point([0.0, 0.0, 0.0]), [5.0, 5.0, 5.0]);
        // a shear: the normal stays perpendicular to the sheared surface's tangents, at its stored length
        let s = Placing::new(pose([[1.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]));
        assert!(matches!(s.turn, Turn::General { mirror: false, .. }));
        // the surface x = 0 (tangents y and z) leans to x = y: its normal becomes (1, -1, 0) / sqrt 2
        let n = s.normal([1.0, 0.0, 0.0]).expect("normal");
        let (t, u) = (
            s.tangent([0.0, 1.0, 0.0, 1.0]).expect("tangent"),
            s.tangent([0.0, 0.0, 1.0, 1.0]).expect("tangent"),
        );
        let h = std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (t[0] - h).abs() < 1e-6 && (t[1] - h).abs() < 1e-6 && t[2] == 0.0 && t[3] == 1.0,
            "{t:?}"
        );
        let d = |a: [f32; 4]| f64::from(a[0] * n[0] + a[1] * n[1] + a[2] * n[2]);
        assert!(
            d(t).abs() < 1e-6 && d(u).abs() < 1e-6,
            "{n:?} against {t:?} and {u:?}"
        );
        assert!(
            (n[0] - h).abs() < 1e-6 && (n[1] + h).abs() < 1e-6 && n[2] == 0.0,
            "{n:?}"
        );
        // a mirror flips the handedness
        let m = Placing::new(pose([[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]));
        assert!(matches!(m.turn, Turn::General { mirror: true, .. }));
        assert_eq!(m.tangent([0.0, 1.0, 0.0, 1.0]), Some([0.0, 1.0, 0.0, -1.0]));
        assert_eq!(m.normal([1.0, 0.0, 0.0]), Some([-1.0, 0.0, 0.0]));
        // no inverse: no normals or tangents
        let z = Placing::new(pose([[0.0; 3]; 3]));
        assert_eq!(z.turn, Turn::Singular);
        assert_eq!(z.normal([0.0, 0.0, 1.0]), None);
        assert_eq!(z.tangent([1.0, 0.0, 0.0, 1.0]), None);
    }

    #[test]
    fn a_mirroring_pose_keeps_fronts_facing_the_normals() {
        use primase_core::{Primitive, Topology};
        // a triangle facing +z in GTA's frame (counter-clockwise seen from above), its normals +z
        let up = GTA.direction([0.0, 0.0, 1.0]);
        let mut streams = std::collections::BTreeMap::new();
        streams.insert(
            Semantic::Position,
            Stream::F32x3(
                [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
                    .map(|p| GTA.point(p))
                    .to_vec(),
            ),
        );
        streams.insert(Semantic::Normal, Stream::F32x3(vec![up; 3]));
        let mut w = World::new();
        let e = w.spawn();
        w.insert(
            e,
            Mesh::new(vec![Primitive {
                topology: Topology::Triangles,
                streams,
                indices: Some(vec![0, 1, 2]),
                material: None,
            }]),
        );
        // mirrored in x: the triangle now lies the other way round, its normals still +z
        w.insert(
            e,
            GtaPoses(vec![Some(pose([
                [-1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ]))]),
        );
        let mut log = PassLog::default();
        PoseFragments.run(&mut w, &mut log).expect("posed");
        let p = &w.get::<Mesh>(e).expect("mesh").primitives()[0];
        assert_eq!(p.indices.as_deref(), Some(&[0, 2, 1][..]));
        let Some(Stream::F32x3(v)) = p.streams.get(&Semantic::Position) else {
            panic!("positions")
        };
        let Some(Stream::F32x3(n)) = p.streams.get(&Semantic::Normal) else {
            panic!("normals")
        };
        let [a, b, c] = [0, 2, 1].map(|i| v[i]);
        let (u, t) = (
            [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
            [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
        );
        let front = [
            u[1] * t[2] - u[2] * t[1],
            u[2] * t[0] - u[0] * t[2],
            u[0] * t[1] - u[1] * t[0],
        ];
        assert!(
            front[0] * n[0][0] + front[1] * n[0][1] + front[2] * n[0][2] > 0.0,
            "{front:?} against {:?}",
            n[0]
        );
        assert_eq!(
            log.counts
                .get("primitives posed by a mirroring pose: triangles reversed"),
            Some(&1)
        );
    }
}
