//! Handedness of the Unity reader's frame change, on small real meshes against UnityPy's decoding of them.
//!
//! `oracles.unitypy.sledding/frames.json` is a thin UnityPy dump (MeshHandler) of a few meshes in Unity's own frame:
//! vertices, normals, tangents (xyzw), and each submesh's triangles in Unity's order. Unity's conventions, from its
//! documentation and shader library: a left-handed frame (+Y up, +Z forward); clockwise front faces, so the front side
//! of a triangle `(a, b, c)` is along `cross(b - a, c - a)` computed in Unity's coordinates; the bitangent is
//! `cross(normal, tangent.xyz) * tangent.w` (UnityCG / URP `CreateTangentToWorld`), the normal map's +Y along it. The
//! world is right-handed with counter-clockwise fronts (glTF), reached from Unity by `C = diag(1, 1, -1)`.
//!
//! Checked for every vertex and triangle: positions, normals and tangent directions are `C` of Unity's (bit for bit:
//! `C` only negates); each of our triangles is one of Unity's, with its front side `C` of Unity's front side; our
//! bitangent `cross(N, T) * w` is `C` of Unity's (to 1e-5 after normalising). So a mirror is undone in the geometry,
//! not left in a transform, and a surface lit through its normal map is lit the same way.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::{Semantic, Stream};
use primase_testdata::{Game, OracleSet, Tool, install_file, oracle, tool};
use primase_unity::TypeTreePackage;
use primase_unity::mesh::UnityMeshData;
use primase_unity::player::{ObjRef, Player};
use serde_json::Value as Json;

fn v3(j: &Json) -> [f64; 3] {
    std::array::from_fn(|k| j[k].as_f64().unwrap())
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(a: [f64; 3]) -> [f64; 3] {
    let l = dot(a, a).sqrt();
    a.map(|x| x / l)
}

/// Unity's frame to the world's.
fn c(v: [f64; 3]) -> [f64; 3] {
    [v[0], v[1], -v[2]]
}

/// A position's bits, for matching ours to Unity's.
fn key(p: [f32; 3]) -> [u32; 3] {
    p.map(f32::to_bits)
}

#[test]
fn mirror_is_undone_in_the_geometry() {
    let (Some(data), Some(tpk), Some(frames)) = (
        install_file(Game::Sledding, "Sledding Game_Data"),
        tool(Tool::UnityTpk),
        oracle(OracleSet::UnityPy, Game::Sledding, "frames.json"),
    ) else {
        return;
    };
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let oracle: Json = serde_json::from_slice(&std::fs::read(&frames).unwrap()).unwrap();
    let meshes = oracle["meshes"].as_array().unwrap();
    assert!(!meshes.is_empty(), "no meshes in {}", frames.display());
    let (mut triangles, mut vertices, mut both_signs, mut degenerate) = (0usize, 0usize, 0usize, 0usize);
    for m in meshes {
        let name = m["name"].as_str().unwrap();
        let r = ObjRef {
            file: m["file"]
                .as_str()
                .unwrap()
                .rsplit('/')
                .next()
                .unwrap()
                .to_lowercase(),
            path_id: m["path_id"].as_i64().unwrap(),
        };
        let src_p: Vec<[f64; 3]> = m["vertices"].as_array().unwrap().iter().map(v3).collect();
        let src_n: Vec<[f64; 3]> = m["normals"].as_array().unwrap().iter().map(v3).collect();
        let src_t: Vec<[f64; 4]> = m["tangents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| std::array::from_fn(|k| t[k].as_f64().unwrap()))
            .collect();
        let src_uv: Vec<[f64; 2]> = m["uv0"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| [t[0].as_f64().unwrap(), t[1].as_f64().unwrap()])
            .collect();
        let submeshes = m["submeshes"].as_array().unwrap();
        let ours = UnityMeshData::read(&player, &r).unwrap();
        let prims = ours.primitives(0..submeshes.len()).unwrap();
        assert_eq!(prims.0.len(), submeshes.len(), "{name}: primitives");
        for ((sub_index, p), tris) in prims.0.iter().zip(submeshes) {
            let tris: Vec<[usize; 3]> = tris
                .as_array()
                .unwrap()
                .iter()
                .map(|t| std::array::from_fn(|k| t[k].as_u64().unwrap() as usize))
                .collect();
            let pos = p.positions().unwrap();
            let Some(Stream::F32x3(nor)) = p.streams.get(&Semantic::Normal) else {
                panic!("{name}: no normals")
            };
            let Some(Stream::F32x4(tan)) = p.streams.get(&Semantic::Tangent) else {
                panic!("{name}: no tangents")
            };
            let Some(Stream::F32x2(uvs)) = p.streams.get(&Semantic::TexCoord(0)) else {
                panic!("{name}: no uv0")
            };
            vertices += pos.len();
            both_signs += usize::from(tan.iter().any(|t| t[3] > 0.0) && tan.iter().any(|t| t[3] < 0.0));
            // Unity's triangles by their corners' positions under C; each of ours must be one of them, with its
            // corners' normals, tangent directions and texture coordinates (v from the top) those of Unity's corners
            // under C, and its front side Unity's front side under C
            let mut unity: BTreeMap<[[u32; 3]; 3], Vec<[usize; 3]>> = BTreeMap::new();
            for t in &tris {
                let mut k = t.map(|i| key(c(src_p[i]).map(|x| x as f32)));
                k.sort();
                unity.entry(k).or_default().push(*t);
            }
            let same_corner = |i: usize, s: usize| {
                let t = tan[i];
                let st = src_t[s];
                key(pos[i]) == key(c(src_p[s]).map(|x| x as f32))
                    && key(nor[i]) == key(c(src_n[s]).map(|x| x as f32))
                    && key([t[0], t[1], t[2]]) == key(c([st[0], st[1], st[2]]).map(|x| x as f32))
                    && (src_uv[s][0] - f64::from(uvs[i][0])).abs() < 1e-6
                    && (1.0 - src_uv[s][1] - f64::from(uvs[i][1])).abs() < 1e-6
            };
            let idx = p.indices.as_ref().unwrap();
            assert_eq!(
                idx.len() / 3,
                tris.len(),
                "{name} submesh {sub_index}: triangle count"
            );
            for (k, t) in idx.as_chunks::<3>().0.iter().enumerate() {
                let t = t.map(|i| i as usize);
                let mut pk = t.map(|i| key(pos[i]));
                pk.sort();
                let candidates = unity
                    .get_mut(&pk)
                    .unwrap_or_else(|| panic!("{name} submesh {sub_index}: triangle {k} is none of Unity's"));
                let [a, b, cc] = t.map(|i| pos[i].map(f64::from));
                let front_ours = cross(sub(b, a), sub(cc, a));
                let area = dot(front_ours, front_ours) > 0.0;
                // Unity's triangle and the corner of it that each of our corners is
                let found = candidates.iter().enumerate().find_map(|(j, u)| {
                    let front = c(cross(
                        sub(src_p[u[1]], src_p[u[0]]),
                        sub(src_p[u[2]], src_p[u[0]]),
                    ));
                    if area && dot(front_ours, front) <= 0.0 {
                        return None;
                    }
                    let corners: Option<Vec<usize>> = t
                        .iter()
                        .map(|&i| u.iter().copied().find(|&s| same_corner(i, s)))
                        .collect();
                    corners.map(|cs| (j, cs))
                });
                let Some((j, corners)) = found else {
                    panic!(
                        "{name} submesh {sub_index}: triangle {k} faces the other way from Unity's, or its corners differ"
                    )
                };
                candidates.swap_remove(j);
                degenerate += usize::from(!area);
                // the bitangent at each corner, ours against Unity's under C
                for (&i, &s) in t.iter().zip(&corners) {
                    let w = tan[i];
                    let ours_b = unit(
                        cross(nor[i].map(f64::from), [w[0], w[1], w[2]].map(f64::from))
                            .map(|x| x * f64::from(w[3])),
                    );
                    let st = src_t[s];
                    let unity_b = unit(c(cross(src_n[s], [st[0], st[1], st[2]]).map(|x| x * st[3])));
                    let off = (0..3)
                        .map(|k| (ours_b[k] - unity_b[k]).abs())
                        .fold(0f64, f64::max);
                    assert!(
                        off < 1e-5,
                        "{name} vertex {i}: bitangent {ours_b:?}, Unity's under C {unity_b:?}"
                    );
                }
                triangles += 1;
            }
        }
    }
    assert!(
        both_signs > 0,
        "the meshes must hold tangents of both handedness signs"
    );
    println!(
        "{} meshes, {vertices} vertices, {triangles} triangles ({degenerate} without area): positions, normals, fronts and bitangents match",
        meshes.len()
    );
}
