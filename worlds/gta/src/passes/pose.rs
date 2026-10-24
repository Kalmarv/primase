//! `gta-pose-fragments`: a fragment's models drawn at their bones' poses, as the game draws a fragment at rest.
//!
//! The reader keeps the bones as stored: each primitive's bone ([`GtaBones`]) and the prototype's skeleton and
//! fragment pose ([`GtaSkeleton`]). The pass composes each bone's pose ([`crate::rage::drawable::Bones::pose`]: a fragment's
//! own pose of the bone, else the skeleton's rest pose up to the root; rows in GTA's frame), places the posed
//! primitives' positions by it, turns their normals and tangents by its 3x3 part as core turns local space
//! ([`primase_core::Linear`]: normals by the inverse transpose, tangents by the part, both of unit length; a mirror
//! flips the handedness and reverses the triangles; a singular pose flattens the model, and its normals and tangents
//! are left out, counted), in GTA's frame (the frame conversion is a signed permutation, so this is bit-exact with
//! posing before it), and removes `gta.bones` (the skeleton stays). A primitive on a bone that has no pose is left as
//! stored, counted. Every other GTA pass on meshes refuses a mesh still holding bones, so this one runs first.
use primase_core::pass::{Pass, PassLog, PassSpec};
use std::collections::BTreeMap;

use primase_core::{Entity, Linear, Lods, Mesh, Semantic, Stream, World};

use crate::components::GTA;
use crate::components::{GtaBones, GtaSkeleton};

/// A model's transform in GTA's frame: rows `[x, y, z, translation]`.
pub type Pose = [[f32; 4]; 3];

/// A pose's 3x3 part as core's linear map: its columns are the images of GTA's local axes.
pub(crate) fn linear(pose: &Pose) -> Linear {
    Linear::new(std::array::from_fn(|j| [pose[0][j], pose[1][j], pose[2][j]]))
}

/// `v` with homogeneous `w` (1 a point, 0 a direction) through the pose, in f64 and rounded once.
pub(crate) fn transform(r: &Pose, v: [f32; 3], w: f64) -> [f32; 3] {
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

/// `gta-pose-fragments`' declaration ([`primase_core::pass::PassRegistry::add`]): no options, it poses by the game's own rule.
pub const GTA_POSE_FRAGMENTS: PassSpec = PassSpec {
    name: "gta-pose-fragments",
    options: &[],
    make: |_| Ok(Box::new(PoseFragments)),
};

impl Pass for PoseFragments {
    fn name(&self) -> &'static str {
        "gta-pose-fragments"
    }
    /// Nothing: a world without fragments has no poses to place.
    fn requires(&self) -> &[&'static str] {
        &[]
    }
    fn removes(&self) -> &[&'static str] {
        &["gta.bones"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // a level-of-detail form's bones are its prototype's
        let owner: BTreeMap<Entity, Entity> = w
            .iter::<Lods>()
            .flat_map(|(proto, l)| l.forms.iter().map(move |f| (f.form, proto)))
            .collect();
        let holders: Vec<_> = w.iter::<GtaBones>().map(|(e, _)| e).collect();
        for e in holders {
            let bones = w.remove::<GtaBones>(e).expect("bones").0;
            let skeleton = [Some(e), owner.get(&e).copied()]
                .into_iter()
                .flatten()
                .find_map(|h| w.get::<GtaSkeleton>(h))
                .ok_or_else(|| format!("{e:?}: gta.bones without its prototype's gta.skeleton"))?;
            let poses = skeleton.0.pose();
            let mut mesh = w
                .remove::<Mesh>(e)
                .ok_or("gta.bones on an entity without core.mesh")?;
            if bones.len() != mesh.primitives().len() {
                return Err(format!(
                    "{e:?}: {} bones for {} primitives",
                    bones.len(),
                    mesh.primitives().len()
                ));
            }
            for (mut p, bone) in mesh.primitives_mut().zip(bones) {
                let Some(bone) = bone else { continue };
                let Some(&pose) = poses.get(usize::from(bone)) else {
                    log.count("primitives on a bone without a pose, left as stored", 1);
                    continue;
                };
                let t = linear(&pose);
                log.count("primitives posed", 1);
                if t.singular() {
                    log.count(
                        "primitives posed flat (no inverse): normals and tangents left out",
                        1,
                    );
                } else {
                    let rotation = !t.mirrors() && t.similarity().is_some_and(|s| (s - 1.0).abs() <= 1e-6);
                    if !rotation {
                        log.count("primitives posed by a pose that is not a rotation", 1);
                    }
                    if t.mirrors() {
                        // a mirror turns every triangle over: reverse them so their fronts stay where they were
                        let n = p.vertex_count() as u32;
                        for tri in p
                            .indices_mut()
                            .get_or_insert_with(|| (0..n).collect())
                            .as_chunks_mut::<3>()
                            .0
                        {
                            tri.swap(1, 2);
                        }
                        log.count("primitives posed by a mirroring pose: triangles reversed", 1);
                    }
                }
                if let Some(Stream::F32x3(v)) = p.streams_mut().get_mut(&Semantic::Position) {
                    for x in v.iter_mut() {
                        *x = GTA.point(transform(&pose, GTA.point_to_source(*x), 1.0));
                    }
                }
                if t.singular() {
                    p.streams_mut().remove(&Semantic::Normal);
                    p.streams_mut().remove(&Semantic::Tangent);
                    continue;
                }
                if let Some(Stream::F32x3(v)) = p.streams_mut().get_mut(&Semantic::Normal) {
                    for x in v.iter_mut() {
                        *x = GTA.direction(t.normal(GTA.direction_to_source(*x)).expect("not singular"));
                    }
                }
                if let Some(Stream::F32x4(v)) = p.streams_mut().get_mut(&Semantic::Tangent) {
                    for x in v.iter_mut() {
                        let s = GTA.direction_to_source([x[0], x[1], x[2]]);
                        let u = t.tangent([s[0], s[1], s[2], x[3]]).expect("not singular");
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

    /// A pose turns directions by its 3x3 part as core turns local space: as positions under a rotation, normals by
    /// the inverse transpose and tangents by the part otherwise (unit length), a mirror flipping the handedness, and
    /// none under a singular pose.
    #[test]
    fn poses_turn_normals_by_their_inverse_transpose() {
        // a rotation of 90 degrees about z: every direction turns as a position does
        let r = pose([[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
        let l = linear(&r);
        assert_eq!(l.normal([1.0, 0.0, 0.0]), Some([0.0, 1.0, 0.0]));
        assert_eq!(l.tangent([1.0, 0.0, 0.0, -1.0]), Some([0.0, 1.0, 0.0, -1.0]));
        assert_eq!(transform(&r, [0.0, 0.0, 0.0], 1.0), [5.0, 5.0, 5.0]);
        assert_eq!(transform(&r, [1.0, 0.0, 0.0], 1.0), [5.0, 6.0, 5.0]);
        // a shear: the normal stays perpendicular to the sheared surface's tangents
        let s = linear(&pose([[1.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]));
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
        let m = linear(&pose([[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]));
        assert!(m.mirrors());
        assert_eq!(m.tangent([0.0, 1.0, 0.0, 1.0]), Some([0.0, 1.0, 0.0, -1.0]));
        assert_eq!(m.normal([1.0, 0.0, 0.0]), Some([-1.0, 0.0, 0.0]));
        // no inverse: no normals or tangents
        let z = linear(&pose([[0.0; 3]; 3]));
        assert!(z.singular());
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
        // on bone 0, which a fragment pose mirrors in x: the triangle now lies the other way round, its normals still +z
        w.insert(e, GtaBones(vec![Some(0)]));
        w.insert(
            e,
            GtaSkeleton(crate::rage::drawable::Bones {
                skeleton: None,
                fragment_pose: vec![pose([[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]])],
            }),
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
