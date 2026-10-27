//! Unity meshes as world primitives: a mesh read by `unity_formats::mesh` converted from Unity's frame (positions and
//! directions mirrored, triangle winding reversed, v flipped), its submeshes made primitives, its skin and blend shapes
//! made streams and `core.morphs`.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use primase_core::{Deltas, MorphFrame, MorphId, MorphTarget, Morphs, Primitive, Semantic, Stream, Topology};
use primase_meshops::compact_primitive;
use unity_formats::Error;
use unity_formats::Result;
use unity_formats::mesh::{CHANNELS, MeshShapes, ShapeVertex, UnityMeshData};
use unity_formats::player::SkinWeights;

use crate::components::UNITY;
use crate::invalid;

/// `core.morphs` for primitives made of this mesh's vertices: target `c` is channel `c` (`MorphId(c)`), its frames at
/// Unity's weights (0 to 100 for a full shape), and per primitive the sparse deltas of the vertices it kept, in its
/// own numbering (`gathers[i][new]` is the mesh vertex of vertex `new` of primitive `i`). Positions are deltas in
/// the world's frame; normal and tangent deltas only where the frame stores them.
pub fn shapes_morphs(shapes: &MeshShapes, vertex_count: usize, gathers: &[&[u32]]) -> Morphs {
    let numbering: Vec<Vec<u32>> = gathers
        .iter()
        .map(|g| {
            let mut to_new = vec![u32::MAX; vertex_count];
            for (new, &old) in g.iter().enumerate() {
                to_new[old as usize] = new as u32;
            }
            to_new
        })
        .collect();
    let targets = shapes
        .channels
        .iter()
        .enumerate()
        .map(|(c, channel)| MorphTarget {
            id: MorphId(c as u32),
            name: channel.name.clone(),
            default_weight: 0.0,
            frames: shapes.frames[channel.first_frame..channel.first_frame + channel.frames]
                .iter()
                .map(|frame| MorphFrame {
                    weight: frame.weight,
                    primitives: numbering
                        .iter()
                        .map(|to_new| {
                            let mut entries: Vec<(u32, &ShapeVertex)> = shapes.vertices
                                [frame.first..frame.first + frame.count]
                                .iter()
                                .filter_map(|v| {
                                    let new = to_new[v.index as usize];
                                    (new != u32::MAX).then_some((new, v))
                                })
                                .collect();
                            entries.sort_by_key(|(new, _)| *new);
                            entries.dedup_by_key(|(new, _)| *new);
                            if entries.is_empty() {
                                return None;
                            }
                            Some(Deltas {
                                indices: Some(entries.iter().map(|(new, _)| *new).collect()),
                                positions: Some(
                                    entries.iter().map(|(_, v)| UNITY.point(v.position)).collect(),
                                ),
                                normals: frame.normals.then(|| {
                                    entries.iter().map(|(_, v)| UNITY.direction(v.normal)).collect()
                                }),
                                tangents: frame.tangents.then(|| {
                                    entries.iter().map(|(_, v)| UNITY.direction(v.tangent)).collect()
                                }),
                            })
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect();
    Morphs { targets }
}

/// Unity's MeshTopology as core's, with how a submesh's indices become core's: triangles (0), lines (3) and points (5)
/// as stored, quads (2) as two triangles each (a, b, c and a, c, d), a line strip (4) as its segments; both exact.
fn topology(t: i64) -> Result<(Topology, Option<&'static str>)> {
    Ok(match t {
        0 => (Topology::Triangles, None),
        2 => (
            Topology::Triangles,
            Some("quad submeshes read as triangles, two per quad"),
        ),
        3 => (Topology::Lines, None),
        4 => (
            Topology::Lines,
            Some("line-strip submeshes read as lines, one per segment"),
        ),
        5 => (Topology::Points, None),
        t => return Err(Error::Unsupported(format!("mesh topology {t}"))),
    })
}

/// A submesh's indices as `topology` gives them to core ([`topology`]).
fn core_indices(topology: i64, indices: Vec<u32>) -> Vec<u32> {
    match topology {
        2 => indices
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]])
            .collect(),
        4 => indices.windows(2).flat_map(|s| [s[0], s[1]]).collect(),
        _ => indices,
    }
}

/// The primitives read from submeshes: each with the index of the submesh it is.
pub type Drawn = Vec<(usize, Primitive)>;

/// What reading a submesh could not put into core as stored, by what: counts for [`crate::level::LevelReport::notices`].
pub type MeshNotices = std::collections::BTreeMap<&'static str, u64>;

fn note(notices: &mut MeshNotices, what: &'static str, n: u64) {
    if n > 0 {
        *notices.entry(what).or_default() += n;
    }
}

/// A submesh's primitive made finite, as core requires, by what each stream is, judged on the vertices its indices
/// use (`used`): a non-finite position leaves the primitive out (core cannot place it; `false`), a non-finite normal or
/// tangent takes its stream out (a zero direction is not one; passes can derive it again), a non-finite texture
/// coordinate or colour reads as 0. Each counted in `notices`. The other vertices' non-finite values read as 0 too,
/// uncounted: compaction leaves them out.
fn make_finite(p: &mut Primitive, used: &BTreeSet<u32>, notices: &mut MeshNotices) -> bool {
    fn values(s: &Stream, i: usize) -> &[f32] {
        match s {
            Stream::F32x2(v) => &v[i],
            Stream::F32x3(v) => &v[i],
            Stream::F32x4(v) => &v[i],
            _ => &[],
        }
    }
    let bad = |s: &Stream| -> u64 {
        used.iter()
            .map(|&i| values(s, i as usize).iter().filter(|x| !x.is_finite()).count() as u64)
            .sum()
    };
    if p.streams.get(&Semantic::Position).is_some_and(|s| bad(s) > 0) {
        note(notices, "primitives with non-finite positions left out", 1);
        return false;
    }
    for (semantic, what) in [
        (
            Semantic::Normal,
            "primitives' normal streams with non-finite values left out",
        ),
        (
            Semantic::Tangent(0),
            "primitives' tangent streams with non-finite values left out",
        ),
    ] {
        if p.streams.get(&semantic).is_some_and(|s| bad(s) > 0) {
            p.streams.remove(&semantic);
            note(notices, what, 1);
        }
    }
    for (semantic, stream) in p.streams.iter_mut() {
        let what = match semantic {
            Semantic::TexCoord(_) => Some("non-finite texture coordinates read as 0"),
            Semantic::Color(_) => Some("non-finite vertex colour values read as 0"),
            Semantic::Weights(_) => Some("non-finite skin weights read as 0"),
            _ => None,
        };
        if let Some(what) = what {
            note(notices, what, bad(stream));
        }
        let zero = |x: &mut f32| {
            if !x.is_finite() {
                *x = 0.0;
            }
        };
        match stream {
            Stream::F32x2(v) => v.iter_mut().flatten().for_each(zero),
            Stream::F32x3(v) => v.iter_mut().flatten().for_each(zero),
            Stream::F32x4(v) => v.iter_mut().flatten().for_each(zero),
            _ => {}
        }
    }
    true
}

/// The submeshes `range` as world primitives, each with its submesh index, compacted to the vertices it uses,
/// without materials; a submesh without indices draws nothing and is left out. Unity's frame is converted ([`UNITY`]): positions and directions mirrored, triangle winding reversed
/// so front faces stay front, and a tangent's w negated, since a mirror flips the bitangent cross(n, t) * w (both
/// conventions point the bitangent along the normal map's +Y). Texture coordinates move to the world's top-left
/// origin, v becoming 1 - v; a channel of 3 or 4 components gives its u and v. Normals keep three components
/// (stored as half4, the fourth is padding). Quads become triangles and line strips lines ([`topology`]); each
/// primitive is made finite as core requires ([`make_finite`]). What could not be read as stored is counted in the
/// returned notices, over the primitives returned (and those left out).
pub fn primitives(mesh: &UnityMeshData, range: Range<usize>) -> Result<(Drawn, MeshNotices)> {
    let (out, _, notices) = primitives_with(mesh, range, None)?;
    Ok((out, notices))
}

/// [`primitives`] (or, with `skinned`, [`skinned_primitives`]) and the mesh's blend
/// shapes as `core.morphs` over the primitives returned, `None` for a mesh without any
/// ([`shapes_morphs`]).
pub fn primitives_and_morphs(
    mesh: &UnityMeshData,
    range: Range<usize>,
    skinned: Option<SkinWeights>,
) -> Result<(Drawn, Option<Morphs>, MeshNotices)> {
    if skinned.is_some() && mesh.skin.is_none() {
        return Err(invalid(format!("{}: no skin to read", mesh.name)));
    }
    let (out, gathers, notices) = primitives_with(mesh, range, skinned)?;
    let morphs = mesh.shapes.as_ref().map(|s| {
        let gathers: Vec<&[u32]> = gathers.iter().map(Vec::as_slice).collect();
        shapes_morphs(s, mesh.vertex_count, &gathers)
    });
    Ok((out, morphs, notices))
}

/// [`primitives`] with the skin's streams as `limit` draws them: `Joints(k)` (the bone slots, 16
/// bits) and `Weights(k)` (floats) for each set of four influences ([`MeshSkin::sets`]); a vertex with more
/// influences than `limit` allows keeps its first ones renormalised, and the bone slot of a zero-weight influence
/// beyond the bind poses is set to 0 (both counted). An error for a mesh without a skin.
pub fn skinned_primitives(
    mesh: &UnityMeshData,
    range: Range<usize>,
    limit: SkinWeights,
) -> Result<(Vec<(usize, Primitive)>, MeshNotices)> {
    if mesh.skin.is_none() {
        return Err(invalid(format!("{}: no skin to read", mesh.name)));
    }
    let (out, _, notices) = primitives_with(mesh, range, Some(limit))?;
    Ok((out, notices))
}

/// The primitives of the submeshes in `range` with, for each, the mesh vertex of each of its vertices.
fn primitives_with(
    mesh: &UnityMeshData,
    range: Range<usize>,
    skinned: Option<SkinWeights>,
) -> Result<(Drawn, Vec<Vec<u32>>, MeshNotices)> {
    let mut notices = MeshNotices::new();
    let n = mesh.vertex_count;
    let mut streams = BTreeMap::new();
    for (ch, c) in mesh.channels.iter().enumerate() {
        let Some(c) = c else { continue };
        let xyz = |i: usize| -> [f32; 3] {
            let v = c.vertex(i);
            [v[0], v[1], v[2]]
        };
        let (semantic, stream) = match (ch, c.dim) {
            (0, 3..) => (
                Semantic::Position,
                Stream::F32x3((0..n).map(|i| UNITY.point(xyz(i))).collect()),
            ),
            (1, 3..) => (
                Semantic::Normal,
                Stream::F32x3((0..n).map(|i| UNITY.direction(xyz(i))).collect()),
            ),
            (2, 4) => (
                Semantic::Tangent(0),
                Stream::F32x4(
                    (0..n)
                        .map(|i| {
                            let [x, y, z] = UNITY.direction(xyz(i));
                            [x, y, z, -c.vertex(i)[3]]
                        })
                        .collect(),
                ),
            ),
            (3, 4) => (
                Semantic::Color(0),
                Stream::F32x4(
                    (0..n)
                        .map(|i| {
                            let v = c.vertex(i);
                            [v[0], v[1], v[2], v[3]]
                        })
                        .collect(),
                ),
            ),
            // u and v; components beyond them are not read (counted below, per primitive)
            (4..12, 2..=4) => (
                Semantic::TexCoord(ch as u8 - 4),
                Stream::F32x2(
                    (0..n)
                        .map(|i| {
                            let v = c.vertex(i);
                            [v[0], 1.0 - v[1]]
                        })
                        .collect(),
                ),
            ),
            (_, d) => {
                return Err(Error::Unsupported(format!(
                    "{} channel of {d} components",
                    CHANNELS[ch]
                )));
            }
        };
        streams.insert(semantic, stream);
    }
    if let Some((skin, limit)) = mesh.skin.as_ref().zip(skinned) {
        let poses = skin.bind_poses.len() as u32;
        let (weights, bones, cut) = skin.padded(limit);
        note(
            &mut notices,
            "skin influences beyond the quality level's limit cut and the rest renormalised (vertices)",
            cut,
        );
        let mut beyond = 0u64;
        for (k, (weights, bones)) in weights.into_iter().zip(bones).enumerate() {
            let joints = bones
                .iter()
                .zip(&weights)
                .map(|(b, w)| {
                    std::array::from_fn(|i| {
                        if b[i] >= poses {
                            debug_assert_eq!(w[i], 0.0, "checked when read");
                            beyond += 1;
                            0
                        } else {
                            b[i] as u16
                        }
                    })
                })
                .collect();
            streams.insert(Semantic::Joints(k as u8), Stream::U16x4(joints));
            streams.insert(Semantic::Weights(k as u8), Stream::F32x4(weights));
        }
        note(
            &mut notices,
            "zero-weight skin influences naming a bone beyond the bind poses set to bone 0 (vertices)",
            beyond,
        );
    }
    let subs = mesh.submeshes.get(range.clone()).ok_or_else(|| {
        invalid(format!(
            "{}: submeshes {range:?} of {}",
            mesh.name,
            mesh.submeshes.len()
        ))
    })?;
    let mut full = Primitive {
        topology: Topology::Triangles,
        streams,
        indices: None,
        material: None,
    };
    let mut out = Vec::with_capacity(subs.len());
    let mut gathers = Vec::with_capacity(subs.len());
    for (k, s) in subs.iter().enumerate() {
        let k = range.start + k;
        if s.index_count == 0 {
            continue;
        }
        let what = || format!("{} submesh {k}", mesh.name);
        let (core_topology, conversion) = topology(s.topology)?;
        full.topology = core_topology;
        if let Some(what) = conversion {
            note(&mut notices, what, 1);
        }
        if s.first_byte % mesh.index_size != 0 {
            return Err(invalid(format!("{}: first byte {}", what(), s.first_byte)));
        }
        let first = (s.first_byte / mesh.index_size) as usize;
        let indices = mesh
            .indices
            .get(first..first + s.index_count as usize)
            .ok_or_else(|| invalid(format!("{}: indices past the buffer", what())))?
            .iter()
            .map(|&i| i.checked_add(s.base_vertex))
            .collect::<Option<Vec<u32>>>()
            .ok_or_else(|| invalid(format!("{}: base vertex overflows", what())))?;
        let mut indices = core_indices(s.topology, indices);
        if full.topology == Topology::Triangles && UNITY.flips_winding() {
            for t in indices.as_chunks_mut::<3>().0 {
                t.swap(1, 2);
            }
        }
        if let Some(&i) = indices.iter().find(|&&i| i as usize >= mesh.vertex_count) {
            return Err(invalid(format!(
                "{}: index {i} past the mesh's {} vertices",
                what(),
                mesh.vertex_count
            )));
        }
        let used: BTreeSet<u32> = indices.iter().copied().collect();
        let mut part = Primitive {
            indices: Some(indices),
            ..full.clone()
        };
        if !make_finite(&mut part, &used, &mut notices) {
            continue;
        }
        let compacted = compact_primitive(&part).map_err(|e| invalid(format!("{}: {}", what(), e.0)))?;
        gathers.push(compacted.gather);
        let compacted = compacted.primitive;
        let vertices = used.len() as u64;
        let wide_uvs = mesh.channels[4..12]
            .iter()
            .flatten()
            .filter(|c| c.dim > 2)
            .count() as u64;
        note(
            &mut notices,
            "texture coordinates of 3 or 4 components: those beyond u and v not read (vertices)",
            vertices * wide_uvs,
        );
        out.push((k, compacted));
    }
    Ok((out, gathers, notices))
}

#[cfg(test)]
mod tests {
    use unity_formats::mesh::{Blend, Channel, MeshSkin, ShapeChannel, ShapeFrame, SubMesh};

    use super::*;

    fn channel(dim: usize, values: &[f32]) -> Option<Channel> {
        Some(Channel {
            dim,
            values: values.to_vec(),
        })
    }

    fn shapes() -> MeshShapes {
        let entry = |index: u32, x: f32| ShapeVertex {
            index,
            position: [x, 0.0, 0.0],
            normal: [0.0, x, 0.0],
            tangent: [0.0, 0.0, x],
        };
        MeshShapes {
            // two channels: one frame at 100, and one with frames at 50 and 100
            channels: vec![
                ShapeChannel {
                    name: "smile".into(),
                    name_hash: 7,
                    first_frame: 0,
                    frames: 1,
                },
                ShapeChannel {
                    name: "blink".into(),
                    name_hash: 9,
                    first_frame: 1,
                    frames: 2,
                },
            ],
            frames: vec![
                ShapeFrame {
                    weight: 100.0,
                    first: 0,
                    count: 2,
                    normals: true,
                    tangents: false,
                },
                ShapeFrame {
                    weight: 50.0,
                    first: 2,
                    count: 1,
                    normals: false,
                    tangents: false,
                },
                ShapeFrame {
                    weight: 100.0,
                    first: 3,
                    count: 1,
                    normals: false,
                    tangents: false,
                },
            ],
            vertices: vec![entry(1, 1.0), entry(3, 2.0), entry(1, 0.5), entry(3, 4.0)],
        }
    }

    /// A mesh's blend shapes become `core.morphs` over the vertices a primitive kept, in its own numbering and the
    /// world's frame: a target a channel, its frames at Unity's weights, sparse deltas of the kept vertices only,
    /// normals and tangents only where the frame stores them.
    #[test]
    fn blend_shapes_become_core_morphs_over_the_kept_vertices() {
        // the primitive kept mesh vertices 3, 0, 1 (in that order): new vertex 0 is 3, 2 is 1
        let morphs = shapes_morphs(&shapes(), 4, &[&[3, 0, 1]]);
        assert_eq!(morphs.targets.len(), 2);
        assert_eq!(morphs.targets[1].name, "blink");
        assert_eq!(morphs.targets[1].frames.len(), 2);
        let smile = &morphs.targets[0].frames[0];
        assert_eq!(smile.weight, 100.0);
        let d = smile.primitives[0].as_ref().unwrap();
        // entries of mesh vertices 1 and 3, as new vertices 2 and 0, ascending
        assert_eq!(d.indices, Some(vec![0, 2]));
        // x is the same in both frames
        assert_eq!(d.positions, Some(vec![[2.0, 0.0, 0.0], [1.0, 0.0, 0.0]]));
        assert_eq!(d.normals.as_ref().map(Vec::len), Some(2));
        assert!(d.tangents.is_none());
        let blink = &morphs.targets[1];
        assert_eq!(
            blink.frames.iter().map(|f| f.weight).collect::<Vec<_>>(),
            vec![50.0, 100.0]
        );
        assert!(blink.frames[0].primitives[0].as_ref().unwrap().normals.is_none());
        // a primitive that kept none of a frame's vertices has no deltas for it
        let none = shapes_morphs(&shapes(), 4, &[&[0, 2]]);
        assert!(none.targets[0].frames[0].primitives[0].is_none());
    }

    #[test]
    fn primitives_convert_unity_frame() {
        let mut channels: [Option<Channel>; 12] = Default::default();
        // four vertices, the first unused by the drawn submesh
        channels[0] = channel(3, &[9.0, 9.0, 9.0, 0.0, 0.0, 1.0, 1.0, 0.0, 2.0, 0.0, 1.0, 3.0]);
        channels[1] = channel(3, &[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0]);
        channels[2] = channel(
            4,
            &[
                1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, -1.0, 0.0, 0.0, 1.0, 1.0,
            ],
        );
        channels[4] = channel(2, &[0.5, 0.5, 0.0, 0.0, 1.0, 0.25, 0.0, 1.0]);
        let m = UnityMeshData {
            name: "t".into(),
            vertex_count: 4,
            channels,
            skin: None,
            compressed: false,
            shapes: None,
            indices: vec![0, 1, 2],
            index_size: 2,
            submeshes: vec![
                SubMesh {
                    first_byte: 0,
                    index_count: 0,
                    topology: 0,
                    base_vertex: 0,
                },
                SubMesh {
                    first_byte: 0,
                    index_count: 3,
                    topology: 0,
                    base_vertex: 1,
                },
            ],
        };
        let (prims, notices) = primitives(&m, 0..2).unwrap();
        assert!(notices.is_empty(), "{notices:?}");
        assert_eq!(prims.len(), 1, "the empty submesh is left out");
        let (k, p) = &prims[0];
        assert_eq!(*k, 1);
        // Unity's triangle 1, 2, 3 reversed to 1, 3, 2, then compacted in first use
        assert_eq!(p.indices, Some(vec![0, 1, 2]));
        assert_eq!(
            p.positions().unwrap(),
            &[[0.0, 0.0, -1.0], [0.0, 1.0, -3.0], [1.0, 0.0, -2.0]]
        );
        assert_eq!(
            p.streams[&Semantic::Normal],
            Stream::F32x3(vec![[0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]])
        );
        assert_eq!(
            p.streams[&Semantic::Tangent(0)],
            Stream::F32x4(vec![
                [1.0, 0.0, 0.0, -1.0],
                [0.0, 0.0, -1.0, -1.0],
                [0.0, 0.0, -1.0, 1.0]
            ])
        );
        assert_eq!(
            p.streams[&Semantic::TexCoord(0)],
            Stream::F32x2(vec![[0.0, 1.0], [0.0, 0.0], [1.0, 0.75]])
        );
        assert!(primitives(&m, 1..3).is_err(), "submeshes past the mesh");
    }

    /// An index past the mesh's vertices is an error naming it, not a panic in the finite pass.
    #[test]
    fn an_index_past_the_vertices_is_refused() {
        let mut channels: [Option<Channel>; 12] = Default::default();
        channels[0] = channel(3, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0]);
        let ok = mesh(channels.clone(), vec![0, 1, 2], 0);
        assert!(primitives(&ok, 0..1).is_ok());
        let bad = mesh(channels, vec![0, 1, 7], 0);
        let e = primitives(&bad, 0..1).unwrap_err().to_string();
        assert!(e.contains("index 7 past the mesh's 4 vertices"), "{e}");
    }

    fn mesh(channels: [Option<Channel>; 12], indices: Vec<u32>, topology: i64) -> UnityMeshData {
        UnityMeshData {
            name: "t".into(),
            vertex_count: channels[0].as_ref().map_or(0, |c| c.values.len() / c.dim),
            channels,
            skin: None,
            compressed: false,
            shapes: None,
            index_size: 2,
            submeshes: vec![SubMesh {
                first_byte: 0,
                index_count: indices.len() as u32,
                topology,
                base_vertex: 0,
            }],
            indices,
        }
    }

    /// A UV channel of four components gives u and v, its other two not read (counted per vertex); a non-finite u or
    /// v reads as 0 (counted); a second, two-component channel is untouched.
    #[test]
    fn texture_coordinates_of_four_components_and_non_finite_values() {
        let mut channels: [Option<Channel>; 12] = Default::default();
        channels[0] = channel(3, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        channels[4] = channel(
            4,
            &[
                0.25,
                0.5,
                7.0,
                8.0,
                f32::NAN,
                1.0,
                7.0,
                8.0,
                1.0,
                f32::INFINITY,
                7.0,
                8.0,
            ],
        );
        channels[5] = channel(2, &[0.0, 0.0, 0.5, 0.5, 1.0, 1.0]);
        let (prims, notices) = primitives(&mesh(channels, vec![0, 1, 2], 0), 0..1).unwrap();
        let p = &prims[0].1;
        let Stream::F32x2(uv) = &p.streams[&Semantic::TexCoord(0)] else {
            panic!("two components")
        };
        let mut uv = uv.clone();
        uv.sort_by(|a, b| a.partial_cmp(b).unwrap());
        // v becomes 1 - v: the infinite v's 1 - v is -inf, read as 0
        assert_eq!(uv, vec![[0.0, 0.0], [0.25, 0.5], [1.0, 0.0]]);
        assert!(p.streams.contains_key(&Semantic::TexCoord(1)));
        assert_eq!(
            notices,
            MeshNotices::from([
                ("non-finite texture coordinates read as 0", 2),
                (
                    "texture coordinates of 3 or 4 components: those beyond u and v not read (vertices)",
                    3
                ),
            ])
        );
    }

    /// A skinned mesh's primitives carry `Joints(0)` (16-bit slots) and `Weights(0)` only when asked, compacted with the
    /// vertices; a zero-weight influence beyond the bind poses reads as bone 0, counted.
    #[test]
    fn skinned_primitives_carry_the_skin_streams() {
        let mut channels: [Option<Channel>; 12] = Default::default();
        channels[0] = channel(3, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 5.0, 5.0, 5.0]);
        let mut m = mesh(channels, vec![0, 1, 3], 0);
        m.skin = Some(MeshSkin {
            channel: Blend::from_padded(
                2,
                &[
                    [0.5, 0.5, 0.0, 0.0],
                    [1.0, 0.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0, 0.0],
                    [0.25, 0.75, 0.0, 0.0],
                ],
                &[[1, 0, 0, 0], [0, 9, 0, 0], [0, 0, 0, 0], [0, 1, 0, 0]],
            ),
            unlimited: None,
            bind_poses: vec![[[0.0; 4]; 4]; 2],
        });
        let (plain, _) = primitives(&m, 0..1).unwrap();
        assert!(!plain[0].1.streams.contains_key(&Semantic::Joints(0)));
        let (skinned, notices) = skinned_primitives(&m, 0..1, SkinWeights::Bones(4)).unwrap();
        let p = &skinned[0].1;
        // vertices 0, 1 and 3 are used; the mirror reverses the triangle's winding (0, 3, 1), so the compaction meets
        // them in that order
        assert_eq!(
            p.streams[&Semantic::Joints(0)],
            Stream::U16x4(vec![[1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 0, 0]])
        );
        assert_eq!(
            p.streams[&Semantic::Weights(0)],
            Stream::F32x4(vec![
                [0.5, 0.5, 0.0, 0.0],
                [0.25, 0.75, 0.0, 0.0],
                [1.0, 0.0, 0.0, 0.0]
            ])
        );
        assert_eq!(
            notices,
            MeshNotices::from([(
                "zero-weight skin influences naming a bone beyond the bind poses set to bone 0 (vertices)",
                1
            )])
        );
        m.skin = None;
        assert!(skinned_primitives(&m, 0..1, SkinWeights::Bones(4)).is_err());
    }

    /// A quad submesh is two triangles per quad and a line strip one line per segment, both counted.
    #[test]
    fn quads_and_line_strips_become_triangles_and_lines() {
        let mut channels: [Option<Channel>; 12] = Default::default();
        channels[0] = channel(3, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0]);
        let (prims, notices) = primitives(&mesh(channels.clone(), vec![0, 1, 2, 3], 2), 0..1).unwrap();
        let p = &prims[0].1;
        assert_eq!(p.topology, Topology::Triangles);
        assert_eq!(p.indices.as_ref().map(Vec::len), Some(6));
        assert_eq!(
            notices,
            MeshNotices::from([("quad submeshes read as triangles, two per quad", 1)])
        );
        let (prims, notices) = primitives(&mesh(channels, vec![0, 1, 2, 3], 4), 0..1).unwrap();
        let p = &prims[0].1;
        assert_eq!(p.topology, Topology::Lines);
        assert_eq!(p.indices.as_ref().map(Vec::len), Some(6));
        assert_eq!(
            notices,
            MeshNotices::from([("line-strip submeshes read as lines, one per segment", 1)])
        );
    }

    /// A non-finite tangent takes the primitive's tangent stream out; a non-finite position leaves the primitive out;
    /// a non-finite colour reads as 0; each counted.
    #[test]
    fn non_finite_vertex_values_by_what_they_are() {
        let tri = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let mut channels: [Option<Channel>; 12] = Default::default();
        channels[0] = channel(3, &tri);
        channels[1] = channel(3, &[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]);
        channels[2] = channel(
            4,
            &[1.0, 0.0, 0.0, 1.0, f32::NAN, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0],
        );
        channels[3] = channel(
            4,
            &[
                1.0,
                1.0,
                1.0,
                1.0,
                f32::INFINITY,
                1.0,
                1.0,
                1.0,
                1.0,
                1.0,
                1.0,
                1.0,
            ],
        );
        let (prims, notices) = primitives(&mesh(channels, vec![0, 1, 2], 0), 0..1).unwrap();
        let p = &prims[0].1;
        assert!(!p.streams.contains_key(&Semantic::Tangent(0)));
        assert!(p.streams.contains_key(&Semantic::Normal));
        assert_eq!(
            notices,
            MeshNotices::from([
                ("non-finite vertex colour values read as 0", 1),
                ("primitives' tangent streams with non-finite values left out", 1),
            ])
        );
        let mut channels: [Option<Channel>; 12] = Default::default();
        let mut bad = tri;
        bad[4] = f32::NAN;
        channels[0] = channel(3, &bad);
        let (prims, notices) = primitives(&mesh(channels, vec![0, 1, 2], 0), 0..1).unwrap();
        assert!(prims.is_empty());
        assert_eq!(
            notices,
            MeshNotices::from([("primitives with non-finite positions left out", 1)])
        );
    }
}
