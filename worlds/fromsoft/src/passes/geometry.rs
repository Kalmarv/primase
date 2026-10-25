//! The repairs a geometry lowering may choose for a decoded FLVER part (its UV tiling applied, degenerate triangles
//! dropped, its winding inferred from the stored normals). `lower-fromsoft-geometry` makes each choice explicit.
use crate::flver::Part;
use crate::{Result, invalid};
use primase_meshops::vector::Vec3;

impl Part {
    /// Multiply UV0 by `scale` (a material's tiling, applied to the coordinates instead of the texture binding).
    pub fn apply_uv_scale(&mut self, scale: [f32; 2]) -> Result<()> {
        if !scale.iter().all(|f| f.is_finite()) {
            return Err(invalid("FLVER geometry", self.source_mesh, "non-finite UV scale"));
        }
        for uv in &mut self.mesh.uv {
            uv[0] *= scale[0];
            uv[1] *= scale[1];
        }
        Ok(())
    }

    /// The triangles' areas (squared, doubled) and their agreement with the stored normals, in triangle order.
    fn scores(&self) -> Result<Vec<(f32, f32)>> {
        let m = &self.mesh;
        m.indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| Vec3::from(m.positions[i as usize]));
                let cross = (b - a).cross(c - a);
                let area_squared = cross.length_squared();
                if !area_squared.is_finite() {
                    return Err(invalid(
                        "FLVER geometry",
                        self.source_mesh,
                        "non-finite face area",
                    ));
                }
                let normal = std::array::from_fn(|i| {
                    m.normals[t[0] as usize][i] + m.normals[t[1] as usize][i] + m.normals[t[2] as usize][i]
                });
                Ok((area_squared, cross.dot(Vec3::from(normal))))
            })
            .collect()
    }

    /// Remove triangles whose doubled area squared is at most 1e-14 m^4; returns how many went. The vertices stay.
    pub fn drop_degenerates(&mut self) -> Result<usize> {
        let scores = self.scores()?;
        let mut kept = Vec::with_capacity(self.mesh.indices.len());
        for (t, (area_squared, _)) in self.mesh.indices.as_chunks::<3>().0.iter().zip(&scores) {
            if *area_squared > 1e-14 {
                kept.extend_from_slice(t);
            }
        }
        let dropped = scores.len() - kept.len() / 3;
        self.mesh.indices = kept;
        Ok(dropped)
    }

    /// Reverse every triangle when, over the triangles that are not degenerate, the stored normals point against
    /// the faces' winding on balance (an inferred repair: the file's own winding is the default).
    pub fn repair_winding(&mut self) -> Result<bool> {
        let mut score = 0f64;
        for (area_squared, agreement) in self.scores()? {
            if area_squared > 1e-14 {
                score += f64::from(agreement);
            }
        }
        if !score.is_finite() {
            return Err(invalid(
                "FLVER geometry",
                self.source_mesh,
                "non-finite winding score",
            ));
        }
        if score < 0. {
            for t in self.mesh.indices.as_chunks_mut::<3>().0 {
                t.swap(1, 2);
            }
            self.winding_flipped = true;
        }
        Ok(self.winding_flipped)
    }
}
