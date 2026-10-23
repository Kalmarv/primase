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

/// How a mesh-tree section names its vertex and primitive ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SectionLayout {
    /// 2016.2: `firstPackedVertex`, and `sharedVertices.data` / `primitives.data` each packing `(first << 8) | count`.
    Packed,
    /// 2018.1: separate `firstPackedVertexIndex`, `numPackedVertices`, `firstSharedVertexIndex`, `firstPrimitiveIndex`
    /// and `numPrimitives` fields.
    Fields,
}
/// The section layout of each supported SDK version.
const SECTION_LAYOUTS: [([u8; 8], SectionLayout); 2] = [
    (*b"20160200", SectionLayout::Packed),
    (*b"20180100", SectionLayout::Fields),
];
/// One section of a compressed mesh tree as stored, whichever SDK version stored it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshSection {
    /// The dequantisation offset (xyz) and scale (xyz) of the section's packed vertices.
    pub codec: [f32; 6],
    pub first_packed_vertex: usize,
    pub packed_vertices: usize,
    /// The section's first entry of the shared-vertex index.
    pub first_shared_vertex: usize,
    pub first_primitive: usize,
    pub primitives: usize,
}
/// The sections of an hknpCompressedMeshShapeData mesh tree, as its SDK version names their ranges.
pub fn mesh_sections(data: Value<'_, '_>) -> Result<Vec<MeshSection>> {
    let layout = SECTION_LAYOUTS
        .iter()
        .find(|(version, _)| *version == data.tag.version)
        .map(|(_, layout)| *layout)
        .ok_or_else(|| invalid(data.offset, "no mesh section layout for this SDK version"))?;
    let sections = data.field("meshTree")?.field("sections")?.array()?;
    (0..sections.len())
        .map(|i| {
            let section = sections.get(i)?;
            let field = |name: &str| section.field(name)?.unsigned().map(|v| v as usize);
            let [
                first_packed_vertex,
                packed_vertices,
                first_shared_vertex,
                first_primitive,
                primitives,
            ] = match layout {
                SectionLayout::Packed => {
                    let shared = section.field("sharedVertices")?.field("data")?.unsigned()? as usize;
                    let range = section.field("primitives")?.field("data")?.unsigned()? as usize;
                    [
                        field("firstPackedVertex")?,
                        shared & 255,
                        shared >> 8,
                        range >> 8,
                        range & 255,
                    ]
                }
                SectionLayout::Fields => [
                    field("firstPackedVertexIndex")?,
                    field("numPackedVertices")?,
                    field("firstSharedVertexIndex")?,
                    field("firstPrimitiveIndex")?,
                    field("numPrimitives")?,
                ],
            };
            Ok(MeshSection {
                codec: section.field("codecParms")?.vector::<6>()?,
                first_packed_vertex,
                packed_vertices,
                first_shared_vertex,
                first_primitive,
                primitives,
            })
        })
        .collect()
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
    let ranges = mesh_sections(data)?;
    let mut result = CompressedMesh::default();
    for (section_index, range) in ranges.iter().enumerate() {
        let section = sections.get(section_index)?;
        let &MeshSection {
            codec,
            first_packed_vertex: first,
            packed_vertices: local_count,
            first_shared_vertex: shared_base,
            first_primitive: start,
            primitives: count,
        } = range;
        let offset = [codec[0], codec[1], codec[2]];
        let scale = [codec[3], codec[4], codec[5]];
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
