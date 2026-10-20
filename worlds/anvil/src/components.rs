//! The Anvil dialect: what the reader puts in a world that has no core meaning yet, as stored.

use primase_core::{Component, Entity, World};

/// A MeshShape's triangle geometry ([`crate::read::mesh_shape::Geometry`]) on its prototype: positions converted into
/// core's frame through the frame the reader was given (stated by the program, as the shape's units and frame are not
/// established), faces as position-index triangles (reversed when that frame mirrors, so their fronts stay), and each
/// face's edge ids and each edge's face ids as stored. `entry` and `index_id` name the forge entry it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilMeshShape {
    pub entry: u32,
    pub index_id: u64,
    pub positions: Vec<[f32; 3]>,
    pub faces: Vec<[u32; 3]>,
    pub face_edges: Vec<[u32; 3]>,
    pub edge_faces: Vec<[u16; 2]>,
}

impl Component for AnvilMeshShape {
    const NAME: &'static str = "anvil.mesh_shape";
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        let n = self.positions.len();
        match self
            .faces
            .iter()
            .flatten()
            .find(|&&i| usize::try_from(i).map_or(true, |i| i >= n))
        {
            Some(i) => Err(format!("a face names position {i} of {n}")),
            None => Ok(()),
        }
    }
    fn payload_bytes(&self) -> u64 {
        let bytes = self.positions.len() * 12 + self.faces.len() * 24 + self.edge_faces.len() * 4;
        u64::try_from(bytes).unwrap_or(u64::MAX)
    }
}

/// A Mesh's compiled buffers ([`crate::read::mesh::StoredMesh`]) on its prototype: positions decoded
/// ([`crate::read::mesh::StoredVertices::positions`]) and converted into core's frame through the frame the reader was
/// given (`frame`, stated by the program); triangles as vertex indices (each stored range counted from its vertex base,
/// reversed when that frame mirrors); `ranges` each stored range's first triangle; `materials` descriptor *k*'s
/// Material ClassID for range *k* (in order; descriptor *k*'s vertex count equals range *k*'s low 16 bits; how the
/// engine binds them is not established); `stride`, and each vertex's bytes after its first 8 as stored (`attributes`,
/// stride minus 8 bytes per vertex), their meaning not established; and for stride 24 bytes 20..24 as two
/// little-endian u16 (`words_20`, empty for another stride). `entry` and `index_id` name the forge entry it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilMesh {
    pub entry: u32,
    pub index_id: u64,
    pub frame: primase_core::frame::SourceFrame,
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub ranges: Vec<u32>,
    pub materials: Vec<u64>,
    pub stride: u32,
    pub attributes: Vec<u8>,
    pub words_20: Vec<[u16; 2]>,
}

impl Component for AnvilMesh {
    const NAME: &'static str = "anvil.mesh";
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        let n = self.positions.len();
        let rest = usize::try_from(self.stride).unwrap_or(0).saturating_sub(8);
        if self.stride < 8 || self.attributes.len() != n.saturating_mul(rest) {
            return Err(format!(
                "{n} positions of stride {}, {} attribute bytes",
                self.stride,
                self.attributes.len()
            ));
        }
        if !self.words_20.is_empty() && self.words_20.len() != n {
            return Err(format!(
                "{n} positions, {} texture-coordinate words",
                self.words_20.len()
            ));
        }
        let starts_in_order = self.ranges.first().is_none_or(|&s| s == 0)
            && self.ranges.is_sorted()
            && self
                .ranges
                .last()
                .is_none_or(|&s| usize::try_from(s).is_ok_and(|s| s <= self.triangles.len()));
        if self.materials.len() != self.ranges.len() {
            return Err(format!(
                "{} materials for {} ranges",
                self.materials.len(),
                self.ranges.len()
            ));
        }
        if !starts_in_order {
            return Err(format!(
                "range starts {:?}: not from 0 in order within {} triangles",
                self.ranges,
                self.triangles.len()
            ));
        }
        match self
            .triangles
            .iter()
            .flatten()
            .find(|&&i| usize::try_from(i).map_or(true, |i| i >= n))
        {
            Some(i) => Err(format!("a triangle names vertex {i} of {n}")),
            None => Ok(()),
        }
    }
    fn payload_bytes(&self) -> u64 {
        let bytes = self.positions.len() * 24 + self.triangles.len() * 12;
        u64::try_from(bytes).unwrap_or(u64::MAX)
    }
}
