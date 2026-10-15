use crate::{Result, Value, invalid};

#[derive(Debug, Clone, Copy)]
pub struct Triangle {
    /// Right-aligned shape key: section, primitive, and quad half.
    pub key: u32,
    /// Shape-local coordinates, retaining the source coordinate frame.
    pub vertices: [[f32; 3]; 3],
}
#[derive(Debug, Default)]
pub struct CompressedMesh {
    pub triangles: Vec<Triangle>,
    pub dead_primitives: usize,
}

/// Decode an hknpCompressedMeshShapeData mesh tree. Materials and rigid-body
/// transforms belong to the caller. Packed section vertices are 11/11/10 bits;
/// shared vertices are 21/21/22 bits over the declared domain.
pub fn compressed_mesh(data: Value<'_, '_>) -> Result<CompressedMesh> {
    let tree = data.field("meshTree")?;
    let domain = tree.field("domain")?;
    let min = domain.field("min")?.vector::<4>()?;
    let max = domain.field("max")?.vector::<4>()?;
    let min = [min[0], min[1], min[2]];
    let max = [max[0], max[1], max[2]];
    let primitives = tree.field("primitives")?.array()?;
    let packed = tree.field("packedVertices")?.array()?;
    let shared = tree.field("sharedVertices")?.array()?;
    let shared_indices = tree.field("sharedVerticesIndex")?.array()?;
    let sections = tree.field("sections")?.array()?;
    let mut result = CompressedMesh::default();
    for section_index in 0..sections.len() {
        let section = sections.get(section_index)?;
        let codec = section.field("codecParms")?.vector::<6>()?;
        let offset = [codec[0], codec[1], codec[2]];
        let scale = [codec[3], codec[4], codec[5]];
        let first = section.field("firstPackedVertex")?.unsigned()? as usize;
        let shared_range = section.field("sharedVertices")?.field("data")?.unsigned()? as usize;
        let local_count = shared_range & 255;
        let shared_base = shared_range >> 8;
        let range = section.field("primitives")?.field("data")?.unsigned()? as usize;
        let count = range & 255;
        let start = range >> 8;
        // The primitive occupies seven key bits; its quad half occupies one.
        if count > 128 || section_index > 0x00ff_ffff {
            return Err(invalid(section.offset, "shape key range overflow"));
        }
        let vertex = |index: usize| -> Result<[f32; 3]> {
            if index < local_count {
                let index = first
                    .checked_add(index)
                    .ok_or_else(|| invalid(section.offset, "packed vertex index overflow"))?;
                let value = u32::try_from(packed.get(index)?.unsigned()?)
                    .map_err(|_| invalid(section.offset, "packed vertex exceeds u32"))?;
                primase_vertex::packed_position32(value, scale, offset)
                    .map_err(|e| invalid(section.offset, e.to_string()))
            } else {
                let index = (index - local_count)
                    .checked_add(shared_base)
                    .ok_or_else(|| invalid(section.offset, "shared vertex index overflow"))?;
                let index = shared_indices.get(index)?.unsigned()? as usize;
                primase_vertex::packed_position64(shared.get(index)?.unsigned()?, min, max)
                    .map_err(|e| invalid(section.offset, e.to_string()))
            }
        };
        for i in 0..count {
            let indices = primitives.get(start + i)?.field("indices")?.array()?;
            if indices.len() != 4 {
                return Err(invalid(section.offset, "primitive must have four indices"));
            }
            let mut ix = [0usize; 4];
            for (j, x) in ix.iter_mut().enumerate() {
                *x = indices.get(j)?.unsigned()? as usize;
            }
            if ix == [0xde, 0xad, 0xde, 0xad] {
                result.dead_primitives += 1;
                continue;
            }
            let a = vertex(ix[0])?;
            let b = vertex(ix[1])?;
            let c = vertex(ix[2])?;
            let key = ((section_index as u32) << 8) | ((i as u32) << 1);
            result.triangles.push(Triangle {
                key,
                vertices: [a, b, c],
            });
            if ix[2] != ix[3] {
                result.triangles.push(Triangle {
                    key: key | 1,
                    vertices: [a, c, vertex(ix[3])?],
                });
            }
        }
    }
    Ok(result)
}
