use crate::invalid;
use fromsoft_formats::Result;
use fromsoft_formats::flver::{Model, SpeedTreeLayout, speedtree_layout};
use primase_core::{Primitive, Semantic, Stream, Topology};
use primase_meshops::{Mesh, compact_primitive, vector::Vec3};
use std::collections::{BTreeMap, BTreeSet};

use crate::pose::FRAME;

/// One face set of one mesh, decoded: the vertices its triangles use, compacted in first-use order, in the world
/// frame (positions and directions mirrored in Z, normals unit length, tangent handedness reflected with the frame),
/// UV0 as stored. Triangles in stored order and winding; strips triangulated with the file's alternation.
#[derive(Clone, Debug)]
pub struct Part {
    pub source_mesh: usize,
    pub face_set: usize,
    pub material: usize,
    pub mesh: Mesh,
    /// Stored tangent frame after coordinate conversion, in compacted vertex order; empty without tangents.
    pub tangents: Vec<[f32; 4]>,
    /// Further tangent members, keyed by their layout's semantic index.
    pub further_tangents: BTreeMap<u8, Vec<[f32; 4]>>,
    /// Original vertex indices in compacted order, for retained source attributes.
    pub source_vertices: Vec<u32>,
    /// Whether [`Part::repair_winding`] reversed it.
    pub winding_flipped: bool,
}

/// Decode face set `face_set` of mesh `source_mesh` of `model` ([`Part`]): no triangle is dropped or reordered, and no
/// material decision applies. `None` when it has no whole triangle. A SpeedTree billboard card's is refused: it has
/// no position of its own ([`SpeedTreeLayout::Billboard`]).
pub fn decode(model: &Model, source_mesh: usize, face_set: usize) -> Result<Option<Part>> {
    let m = model
        .meshes
        .get(source_mesh)
        .ok_or_else(|| invalid("FLVER geometry", source_mesh, "mesh index"))?;
    if model.speedtree
        && let Some(&l) = m.layouts.first()
        && speedtree_layout(&model.layouts[l])? == SpeedTreeLayout::Billboard
    {
        // its program places its corners by the camera: it is the `billboard` material class, left out
        return Err(invalid(
            "FLVER geometry",
            source_mesh,
            "a SpeedTree billboard card has no position of its own (its program turns it to the camera)",
        ));
    }
    let f = m
        .faces
        .get(face_set)
        .ok_or_else(|| invalid("FLVER geometry", source_mesh, "face set index"))?;
    let mut indices = f.triangulate();
    indices.truncate(indices.len() / 3 * 3);
    if indices.is_empty() {
        return Ok(None);
    }
    let mut streams = BTreeMap::from([
        (
            Semantic::Position,
            Stream::F32x3(m.vertices.iter().map(|v| v.position).collect()),
        ),
        (
            Semantic::Normal,
            Stream::F32x3(m.vertices.iter().map(|v| v.normal).collect()),
        ),
        (
            Semantic::TexCoord(0),
            Stream::F32x2(
                m.vertices
                    .iter()
                    .map(|v| v.uvs.first().map_or([0.; 2], |uv| [uv[0], uv[1]]))
                    .collect(),
            ),
        ),
    ]);
    if m.vertices
        .iter()
        .any(|v| v.tangent_indices.len() != v.tangents.len())
    {
        return Err(invalid(
            "FLVER geometry",
            source_mesh,
            "tangent values and semantic indices have different counts",
        ));
    }
    let tangent_sets: BTreeSet<_> = m
        .vertices
        .iter()
        .flat_map(|v| v.tangent_indices.iter().copied())
        .collect();
    for set in tangent_sets {
        let semantic = Semantic::Tangent(set);
        let tangents = m
            .vertices
            .iter()
            .map(|v| {
                v.tangent_indices
                    .iter()
                    .position(|&index| index == set)
                    .and_then(|at| v.tangents.get(at))
                    .copied()
                    .ok_or_else(|| {
                        invalid(
                            "FLVER geometry",
                            source_mesh,
                            format!("partial tangent stream set {set}"),
                        )
                    })
            })
            .collect::<Result<Vec<_>>>()?;
        streams.insert(semantic, Stream::F32x4(tangents));
    }
    let compacted = compact_primitive(&Primitive {
        topology: Topology::Triangles,
        streams,
        indices: Some(indices),
        material: None,
    })
    .map_err(|e| invalid("FLVER geometry", source_mesh, e))?;
    let mut streams = compacted.primitive.streams;
    let Some(Stream::F32x3(positions)) = streams.remove(&Semantic::Position) else {
        unreachable!()
    };
    let Some(Stream::F32x3(normals)) = streams.remove(&Semantic::Normal) else {
        unreachable!()
    };
    let Some(Stream::F32x2(uv)) = streams.remove(&Semantic::TexCoord(0)) else {
        unreachable!()
    };
    let mut tangents = match streams.remove(&Semantic::Tangent(0)) {
        Some(Stream::F32x4(v)) => v,
        None => Vec::new(),
        _ => unreachable!(),
    };
    for t in &mut tangents {
        *t = tangent_to_scene(*t);
    }
    let further_tangents = streams
        .into_iter()
        .filter_map(|(semantic, stream)| match (semantic, stream) {
            (Semantic::Tangent(set), Stream::F32x4(mut tangents)) => {
                for t in &mut tangents {
                    *t = tangent_to_scene(*t);
                }
                Some((set, tangents))
            }
            _ => None,
        })
        .collect();
    let mut mesh = Mesh {
        positions,
        normals,
        uv,
        indices: compacted.primitive.indices.expect("indexed primitive"),
    };
    for p in &mut mesh.positions {
        *p = FRAME.point(*p);
    }
    for n in &mut mesh.normals {
        *n = FRAME.direction(*n);
        *n = Vec3::from(*n).normalize_or(Vec3::UNIT_Y).into();
    }
    Ok(Some(Part {
        source_mesh,
        face_set,
        material: m.material,
        mesh,
        tangents,
        further_tangents,
        source_vertices: compacted.gather,
        winding_flipped: false,
    }))
}

/// Normalize the stored tangent and reflect its handedness with the scene frame.
/// Unlike a UV-derived frame this retains the basis used by the game shaders.
pub fn tangent_to_scene(t: [f32; 4]) -> [f32; 4] {
    let direction: [f32; 3] = Vec3::from(FRAME.direction([t[0], t[1], t[2]])).normalize().into();
    [direction[0], direction[1], direction[2], -t[3]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use fromsoft_formats::flver::{FaceSet, Mesh as SourceMesh, Vertex};

    fn two_tangent_model() -> Model {
        Model {
            bones: Vec::new(),
            dummies: Vec::new(),
            version: 0x2001a,
            speedtree: false,
            layouts: Vec::new(),
            materials: Vec::new(),
            meshes: vec![SourceMesh {
                dynamic: false,
                material: 0,
                node: -1,
                palette: Vec::new(),
                layouts: Vec::new(),
                vertices: (0..3)
                    .map(|i| Vertex {
                        position: [i as f32, (i == 1) as u8 as f32, 0.0],
                        normal: [0.0, 1.0, 0.0],
                        tangents: [
                            [1.0, 0.0, 0.0, 1.0],
                            [0.0, 1.0, 0.0, 1.0],
                            [0.0, 0.0, 1.0, if i == 1 { -1.0 } else { 1.0 }],
                        ]
                        .into(),
                        tangent_indices: [0, 1, 4].into(),
                        ..Vertex::default()
                    })
                    .collect(),
                faces: vec![FaceSet {
                    flags: 0,
                    strip: false,
                    cull: true,
                    unknown_06: 0,
                    indices: vec![2, 0, 1],
                }],
            }],
        }
    }

    #[test]
    fn every_tangent_set_follows_compaction_and_scene_handedness() {
        let model = two_tangent_model();
        let part = decode(&model, 0, 0).unwrap().unwrap();
        assert_eq!(part.source_vertices, [2, 0, 1]);
        assert_eq!(part.tangents, vec![tangent_to_scene([1.0, 0.0, 0.0, 1.0]); 3]);
        assert_eq!(
            part.further_tangents[&4],
            [2, 0, 1].map(|i| tangent_to_scene(model.meshes[0].vertices[i].tangents[2]))
        );
        assert_eq!(
            part.further_tangents[&1],
            vec![tangent_to_scene([0.0, 1.0, 0.0, 1.0]); 3]
        );
    }

    #[test]
    fn a_partial_further_tangent_stream_names_its_set() {
        let mut model = two_tangent_model();
        model.meshes[0].vertices[1].tangents.pop();
        model.meshes[0].vertices[1].tangent_indices.pop();
        assert!(
            decode(&model, 0, 0)
                .unwrap_err()
                .to_string()
                .contains("partial tangent stream set 4")
        );
    }
}
