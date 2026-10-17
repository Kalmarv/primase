//! `gta-winding`: triangles turned to face the way their stored normals do.
//!
//! Core's fronts are counter-clockwise (glTF), which is GTA's own index order for nearly every geometry. A primitive
//! whose triangles, summed over the primitive (each face normal, unnormalised, against the sum of its corners' stored
//! normals), face away from the stored normals has every triangle's order reversed: the earlier skate exporter's rule.
//! A primitive without normals keeps the game's order. Runs before `gta-tint`, so a geometry is judged whole.
use primase_core::pass::{Pass, PassLog};
use primase_core::{Primitive, Semantic, Stream, World};

use crate::lower::for_each_mesh;

/// Whether `p`'s triangles face away from its stored normals (`None`: no normals).
pub fn faces_away(p: &Primitive) -> Option<bool> {
    let Some(Stream::F32x3(n)) = p.streams.get(&Semantic::Normal) else {
        return None;
    };
    let pos = p.positions()?;
    let idx: Vec<u32> = p
        .indices
        .clone()
        .unwrap_or_else(|| (0..pos.len() as u32).collect());
    let f = |v: [f32; 3]| v.map(f64::from);
    let mut s = 0f64;
    for t in idx.as_chunks::<3>().0 {
        let [a, b, c] = t.map(|i| i as usize);
        if a >= pos.len() || b >= pos.len() || c >= pos.len() {
            continue;
        }
        let (p0, p1, p2) = (f(pos[a]), f(pos[b]), f(pos[c]));
        let (u, v) = (
            [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]],
            [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]],
        );
        let face = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let m: [f64; 3] =
            std::array::from_fn(|k| f64::from(n[a][k]) + f64::from(n[b][k]) + f64::from(n[c][k]));
        s += face[0] * m[0] + face[1] * m[1] + face[2] * m[2];
    }
    Some(s < 0.0)
}

/// `gta-winding` (see the module documentation).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GtaWinding;

impl Pass for GtaWinding {
    fn name(&self) -> &'static str {
        "gta-winding"
    }
    // no `requires`: it runs on every loaded part and leaves one without its input (a part holding only a water
    // surface, a cell whose shaders all draw nothing) as it is
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh", "gta.sources"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        for_each_mesh(w, |mesh, _| {
            for p in &mut mesh.primitives {
                match faces_away(p) {
                    None => log.count("primitives without normals, kept as stored", 1),
                    Some(false) => {}
                    Some(true) => {
                        let n = p.vertex_count() as u32;
                        let idx = p.indices.get_or_insert_with(|| (0..n).collect());
                        for t in idx.as_chunks_mut::<3>().0 {
                            t.swap(1, 2);
                        }
                        log.count("primitives turned to face their stored normals", 1);
                    }
                }
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use primase_core::{Primitive, Semantic, Stream, Topology};

    use super::faces_away;

    fn triangle(normal_y: f32) -> Primitive {
        let mut streams = BTreeMap::new();
        // (0, 0, 0), (0, 0, 1), (1, 0, 0) is counter-clockwise seen from +Y
        streams.insert(
            Semantic::Position,
            Stream::F32x3(vec![[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]]),
        );
        streams.insert(Semantic::Normal, Stream::F32x3(vec![[0.0, normal_y, 0.0]; 3]));
        Primitive {
            topology: Topology::Triangles,
            streams,
            indices: Some(vec![0, 1, 2]),
            material: None,
        }
    }

    #[test]
    fn a_triangle_against_its_normals_faces_away() {
        assert_eq!(faces_away(&triangle(1.0)), Some(false));
        assert_eq!(faces_away(&triangle(-1.0)), Some(true));
    }
}
