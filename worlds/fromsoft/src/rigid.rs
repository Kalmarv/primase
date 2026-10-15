//! Rigid FLVER lowering. Static vertices use NORMAL.w as one implicit unit-weight
//! bone index; dynamic vertices are already in model bind space. Multi-influence
//! or non-unit explicit weights requested for rigid lowering are refused. Dynamic
//! meshes are animated at runtime: their model-space bind pose is preserved and
//! counted, not submitted to this rigid-only transformation.
use crate::flver::{Bone, Model};
use primase_core::{
    Component, World,
    pass::{Pass, PassLog},
};

#[derive(Clone, Debug)]
pub struct SourceModel(pub Model);
impl Component for SourceModel {
    const NAME: &'static str = "fromsoft.flver";
}
#[derive(Clone, Debug)]
pub struct RigidModel(pub Model);
impl Component for RigidModel {
    const NAME: &'static str = "fromsoft.rigid_model";
}
#[derive(Debug)]
pub struct ApplyRigidSkinning;
impl Pass for ApplyRigidSkinning {
    fn name(&self) -> &'static str {
        "fromsoft-apply-rigid-skinning"
    }
    fn requires(&self) -> &[&'static str] {
        &[SourceModel::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[RigidModel::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[SourceModel::NAME]
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let staged = world
            .iter::<SourceModel>()
            .map(|(e, m)| lower_model(&m.0, log).map(|m| (e, m)))
            .collect::<Result<Vec<_>, _>>()?;
        for (e, m) in staged {
            world.remove::<SourceModel>(e);
            world.insert(e, RigidModel(m));
        }
        Ok(())
    }
}

type Matrix = [[f64; 3]; 4];
const ID: Matrix = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0.; 3]];
fn vector(m: &Matrix, p: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|a| (0..3).map(|i| m[i][a] * p[i]).sum())
}
fn point(m: &Matrix, p: [f64; 3]) -> [f64; 3] {
    let v = vector(m, p);
    std::array::from_fn(|i| v[i] + m[3][i])
}
fn compose(parent: &Matrix, local: &Matrix) -> Matrix {
    [
        vector(parent, local[0]),
        vector(parent, local[1]),
        vector(parent, local[2]),
        point(parent, local[3]),
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
fn normal_matrix(m: &Matrix) -> Result<(Matrix, f64), String> {
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
/// The same implementation used by the declared world pass. The input is never
/// mutated; errors count refused vertices before returning any lowered geometry.
pub fn lower_model(source: &Model, log: &mut PassLog) -> Result<Model, String> {
    let mut refused = 0;
    for m in &source.meshes {
        if m.dynamic {
            continue;
        }
        for v in &m.vertices {
            let weights: Vec<_> = v.bone_weights.iter().copied().filter(|&w| w != 0.).collect();
            if !weights.is_empty() && (weights.len() != 1 || weights[0] != 1.) {
                refused += 1;
            }
        }
    }
    log.count("refused non-rigid vertices", refused);
    if refused != 0 {
        return Err(format!(
            "rigid skinning refused {refused} vertices: requires one bone with weight 1"
        ));
    }
    let mut worlds = Vec::with_capacity(source.bones.len());
    for i in 0..source.bones.len() {
        let mut m = local(&source.bones[i]);
        let mut parent = source.bones[i].parent;
        let mut seen = vec![i];
        while parent >= 0 {
            let p = parent as usize;
            if p >= source.bones.len() || seen.contains(&p) {
                return Err(format!("invalid rigid bone hierarchy at {i}"));
            }
            seen.push(p);
            m = compose(&local(&source.bones[p]), &m);
            parent = source.bones[p].parent;
        }
        worlds.push(m);
    }
    let mut output = source.clone();
    for m in &mut output.meshes {
        let mut mirrored = Vec::with_capacity(m.vertices.len());
        for v in &mut m.vertices {
            if m.dynamic {
                log.count("model-space bind vertices", 1);
                mirrored.push(false);
                continue;
            }
            let matrix = usize::try_from(v.normal_w).ok().and_then(|i| worlds.get(i));
            let Some(matrix) = matrix else {
                log.count("unbound model-space vertices", 1);
                mirrored.push(false);
                continue;
            };
            if *matrix == ID {
                mirrored.push(false);
                continue;
            }
            let (nm, det) = normal_matrix(matrix)?;
            v.position = point(matrix, v.position.map(f64::from)).map(|x| x as f32);
            v.normal = vector(&nm, v.normal.map(f64::from)).map(|x| x as f32);
            for t in &mut v.tangents {
                let xyz = vector(matrix, [t[0] as f64, t[1] as f64, t[2] as f64]);
                for i in 0..3 {
                    t[i] = xyz[i] as f32;
                }
                if det < 0. {
                    t[3] = -t[3];
                }
            }
            if v.position
                .iter()
                .chain(&v.normal)
                .chain(v.tangents.iter().flatten())
                .any(|x| !x.is_finite())
            {
                return Err("nonfinite rigid result".into());
            }
            mirrored.push(det < 0.);
            log.count("rigid vertices transformed", 1);
        }
        for f in &mut m.faces {
            if mirrored.iter().any(|&m| m) {
                let mut indices = f.triangulate();
                for t in indices.as_chunks_mut::<3>().0 {
                    if mirrored[t[0] as usize] {
                        t.swap(1, 2);
                    }
                }
                f.indices = indices;
                f.strip = false;
            }
        }
    }
    log.count("models lowered", 1);
    Ok(output)
}
