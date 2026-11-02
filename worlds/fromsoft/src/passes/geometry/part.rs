use crate::invalid;
use fromsoft_formats::Result;
use fromsoft_formats::flver::{Model, SpeedTreeLayout, speedtree_layout};
use primase_core::{Moves, Primitive, Semantic, Stream, Topology};
use primase_meshops::{Mesh, compact_primitive, vector::Vec3};
use std::collections::{BTreeMap, BTreeSet};

use crate::pose::FRAME;

/// One face set of one mesh, decoded: the vertices its triangles use, compacted in first-use order, in the world
/// frame (positions, points and directions mirrored in Z, normals unit length, tangent handedness reflected with the
/// frame), UV0 as stored. Triangles in stored order and winding; strips triangulated with the file's alternation.
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
    /// The Half4 UV members, points in the model's space moved into the world frame as the positions are, keyed by
    /// their layout's semantic index (the program's `TEXCOORD<n>`): a billboard's or a swaying plant's pivot
    /// ([`crate::sekiro::source::LayerLayout::points`]).
    pub points: BTreeMap<u8, Vec<[f32; 3]>>,
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
    for (index, slot) in crate::sekiro::source::LayerLayout::of(model, source_mesh)?.points {
        let points = m
            .vertices
            .iter()
            .map(|v| match v.uvs.get(slot) {
                // every Half4 value of both games' installs stores w 0: the point is its xyz
                Some(&[x, y, z, 0.0]) => Ok([x, y, z]),
                Some(p) => Err(invalid(
                    "FLVER geometry",
                    source_mesh,
                    format!(
                        "a Half4 point {index} with w {} (the point stream keeps xyz)",
                        p[3]
                    ),
                )),
                None => Err(invalid(
                    "FLVER geometry",
                    source_mesh,
                    format!("a vertex without its Half4 member {index}"),
                )),
            })
            .collect::<Result<Vec<_>>>()?;
        streams.insert(Semantic::Point(index), Stream::F32x3(points));
    }
    let compacted = compact_primitive(&Primitive {
        topology: Topology::Triangles,
        streams,
        indices: Some(indices),
        material: None,
    })
    .map_err(|e| invalid("FLVER geometry", source_mesh, e))?;
    let mut streams = compacted.primitive.streams;
    // every stream into the world frame by its meaning: positions and points, normals, tangents with the handedness
    // the frame reflects; then the directions of unit length
    primase_core::move_streams(&mut streams, &FRAME)
        .map_err(|e| invalid("FLVER geometry", source_mesh, e))?;
    for (semantic, stream) in &mut streams {
        match (semantic.moves(), stream) {
            (Moves::Normal, Stream::F32x3(v)) => {
                for n in v {
                    *n = Vec3::from(*n).normalize_or(Vec3::UNIT_Y).into();
                }
            }
            (Moves::Tangent, Stream::F32x4(v)) => {
                for t in v {
                    let d: [f32; 3] = Vec3::from([t[0], t[1], t[2]]).normalize().into();
                    *t = [d[0], d[1], d[2], t[3]];
                }
            }
            _ => {}
        }
    }
    let Some(Stream::F32x3(positions)) = streams.remove(&Semantic::Position) else {
        unreachable!()
    };
    let Some(Stream::F32x3(normals)) = streams.remove(&Semantic::Normal) else {
        unreachable!()
    };
    let Some(Stream::F32x2(uv)) = streams.remove(&Semantic::TexCoord(0)) else {
        unreachable!()
    };
    let tangents = match streams.remove(&Semantic::Tangent(0)) {
        Some(Stream::F32x4(v)) => v,
        None => Vec::new(),
        _ => unreachable!(),
    };
    let mut further_tangents = BTreeMap::new();
    let mut points = BTreeMap::new();
    for (semantic, stream) in streams {
        match (semantic, stream) {
            (Semantic::Tangent(set), Stream::F32x4(v)) => {
                further_tangents.insert(set, v);
            }
            (Semantic::Point(index), Stream::F32x3(v)) => {
                points.insert(index, v);
            }
            _ => unreachable!("decode puts no other stream in"),
        }
    }
    let mesh = Mesh {
        positions,
        normals,
        uv,
        indices: compacted.primitive.indices.expect("indexed primitive"),
    };
    Ok(Some(Part {
        source_mesh,
        face_set,
        material: m.material,
        mesh,
        tangents,
        further_tangents,
        points,
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

    /// A Half4 UV member is a point in the model's space: decoded by its semantic index (the program's TEXCOORD<n>, not
    /// its storage slot), in compacted order, mirrored into the world frame as the positions are; no UV set.
    #[test]
    fn a_half4_member_is_a_point_moved_as_the_positions() {
        let mut model = two_tangent_model();
        let uv = |kind, index| fromsoft_formats::flver::Member {
            unknown: 0,
            stored: true,
            offset: 0,
            kind,
            semantic: 5,
            index,
        };
        // a Float4 (two storage slots) before the Half4: slot 2, index 3
        model.layouts = vec![vec![uv(3, 0), uv(0x13, 2), uv(0x2e, 3)]];
        model.meshes[0].layouts = vec![0];
        for (i, v) in model.meshes[0].vertices.iter_mut().enumerate() {
            let i = i as f32;
            v.uvs = [[0.0; 4], [0.0; 4], [0.5; 4], [i, 2.0, 3.0 + i, 0.0]].into();
        }
        let part = decode(&model, 0, 0).unwrap().unwrap();
        assert_eq!(part.points.keys().copied().collect::<Vec<_>>(), [3]);
        assert_eq!(
            part.points[&3],
            [2, 0, 1].map(|i| FRAME.point([i as f32, 2.0, 3.0 + i as f32]))
        );
        assert_eq!(part.points[&3][0], [2.0, 2.0, -5.0]);
        assert_eq!(
            part.mesh.positions,
            [2, 0, 1].map(|i| FRAME.point(model.meshes[0].vertices[i].position))
        );
        // a w the point stream would not keep is refused, not dropped
        model.meshes[0].vertices[1].uvs[3][3] = 1.0;
        let refused = decode(&model, 0, 0).unwrap_err().to_string();
        assert!(refused.contains("Half4 point 3 with w 1"), "{refused}");
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
