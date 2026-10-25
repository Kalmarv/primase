//! Handedness of the FLVER geometry's frame change, on a real Sekiro model against SoulsFormats' reading of it.
//!
//! `oracles.soulsformats.sekiro/frames` is a folder holding `frames.jsonl`: a thin dump of SoulsFormats' reading of a
//! few models as stored, in the game's frame: per mesh its LOD0 face set (`CullBackfaces`, the triangles of
//! `FaceSet.Triangulate(true)`) and per vertex position, normal, tangents (xyzw) and UV0. The facts checked against:
//! - the frame: left-handed, +Y up; the world is `C = diag(1, 1, -1)` of it;
//! - which side is a triangle's front: for a face set the game culls, the side its stored normals face, which for
//!   FromSoft's index order is opposite `cross(b - a, c - a)` computed in the game's coordinates (in the Sekiro install,
//!   26,286 culled LOD0 face sets wind so; the others are shadow proxies, which the reader skips, and two meshes of
//!   8,588 triangles in all);
//! - the bitangent: `cross(N, T) * w`, the normal map's red along it and green along T (the G-buffer shaders,
//!   `sekiro::normals`).
//!
//! Checked: each of our triangles is one of the game's with its corners' positions, normals, tangent directions and UVs
//! `C` of the game's; on a culled face set its front side is `C` of the game's front side; at every corner our
//! bitangent `cross(N, T) * w` is `C` of the game's. So the mirror is undone in the geometry and the stored tangent
//! frame keeps its meaning. Every triangle of the game's but those without area is one of ours.
//!
//! The dump is the model as stored, before any bone transform, so the test needs a model none of whose meshes is
//! bone-bound (it asserts the reader's rigid skinning moved no vertex); m11_01_00_00/m911200 is one, and holds
//! tangents of both handedness signs.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_fromsoft::flver::Model;
use primase_fromsoft::{Archive, Binder, dcx::Decoder};
use primase_testdata::{Game, OracleSet, Tool, install, oodle, oracle};
use serde_json::Value as Json;

#[allow(unsafe_code)]
mod native {
    pub fn load(
        path: primase_testdata::OodleLibrary,
    ) -> primase_compress::Result<primase_compress::oodle::Oodle> {
        // SAFETY: caller explicitly supplies a trusted version-6 native library.
        unsafe { primase_compress::oodle::Oodle::open(path.path(), primase_compress::oodle::Version::V6) }
    }
}

fn v(j: &Json) -> Vec<f64> {
    j.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect()
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
    if l == 0.0 { a } else { a.map(|x| x / l) }
}

/// The game's frame to the world's.
fn c(v: [f64; 3]) -> [f64; 3] {
    [v[0], v[1], -v[2]]
}

fn key(p: [f32; 3]) -> [u32; 3] {
    p.map(f32::to_bits)
}

fn close(a: [f64; 3], b: [f64; 3], tol: f64) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() <= tol)
}

/// One stored vertex: position, unit normal, unit tangent direction and w, UV0.
struct Corner {
    p: [f64; 3],
    n: [f64; 3],
    t: [f64; 3],
    w: f64,
    uv: [f64; 2],
}

#[test]
fn mirror_is_undone_in_the_geometry() {
    let (Some(root), Some(dll), Some(frames)) = (
        install(Game::Sekiro),
        oodle(Tool::Oodle6),
        oracle(OracleSet::SoulsFormats, Game::Sekiro, "frames"),
    ) else {
        return;
    };
    let dll = native::load(dll).unwrap();
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 512 * 1024 * 1024,
    };
    let archive = Archive::open(root).unwrap();
    let text = std::fs::read_to_string(frames.join("frames.jsonl")).unwrap();
    let (mut models, mut triangles, mut culled, mut corners_checked) = (0, 0, 0, 0);
    let (mut zero_area, mut signs) = (0, [false; 2]);
    for line in text.lines() {
        let o: Json = serde_json::from_str(line).unwrap();
        let k = o["key"].as_str().unwrap();
        let (area, model) = k.split_once('/').unwrap();
        let (path, leaf) = if area == "obj" {
            (format!("/obj/{model}.objbnd.dcx"), format!("{model}.flver"))
        } else {
            let stem = format!("{area}_{}", &model[1..]);
            (format!("/map/{area}/{stem}.mapbnd.dcx"), format!("{stem}.flver"))
        };
        let binder = Binder::read(&archive.read(&path, &decoder).unwrap(), &decoder).unwrap();
        let f = binder
            .files
            .iter()
            .find(|f| f.name.as_deref().unwrap_or("").rsplit(['/', '\\']).next() == Some(leaf.as_str()))
            .unwrap();
        let parsed = Model::read(&f.bytes).unwrap();
        let mut log = primase_core::pass::PassLog::default();
        let lowered = primase_fromsoft::rigid::lower_model(&parsed, &mut log).unwrap();
        assert_eq!(
            log.counts.get("rigid vertices transformed").copied().unwrap_or(0),
            0,
            "{k}: choose a model whose meshes are not bone-bound (the dump is before bones)"
        );
        // each mesh's first level-0 face set, its winding repaired from the stored normals and its degenerate
        // triangles dropped (the choices `lower-fromsoft-geometry{winding=normals,degenerates=drop}` states)
        let mut parts = Vec::new();
        for (source_mesh, m) in lowered.meshes.iter().enumerate() {
            let Some(face_set) = m.faces.iter().position(|f| f.is_lod0()) else {
                continue;
            };
            let Some(mut part) = lowered.decode(source_mesh, face_set).unwrap() else {
                continue;
            };
            part.repair_winding().unwrap();
            part.drop_degenerates().unwrap();
            if !part.mesh.indices.is_empty() {
                parts.push(part);
            }
        }
        let meshes = o["meshes"].as_array().unwrap();
        for part in &parts {
            let m = &meshes[part.source_mesh];
            let src: Vec<Corner> = m["vertices"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| {
                    let p = v(&x["p"]);
                    let n = v(&x["n"]);
                    let t = x["t"].as_array().unwrap().first().map(v).unwrap_or(vec![0.0; 4]);
                    let uv = v(&x["uv"]);
                    Corner {
                        p: [p[0], p[1], p[2]],
                        n: unit([n[0], n[1], n[2]]),
                        t: unit([t[0], t[1], t[2]]),
                        w: t[3],
                        uv: [uv[0], uv[1]],
                    }
                })
                .collect();
            let tris: Vec<[usize; 3]> = m["triangles"]
                .as_array()
                .unwrap()
                .as_chunks::<3>()
                .0
                .iter()
                .map(|t| std::array::from_fn(|i| t[i].as_u64().unwrap() as usize))
                .collect();
            let cull = m["cull"].as_bool().unwrap();
            let mut game: BTreeMap<[[u32; 3]; 3], Vec<[usize; 3]>> = BTreeMap::new();
            for t in &tris {
                let mut s = t.map(|i| key(c(src[i].p).map(|x| x as f32)));
                s.sort();
                game.entry(s).or_default().push(*t);
            }
            let mesh = &part.mesh;
            let has_tangents = !part.tangents.is_empty();
            let same_corner = |i: usize, s: &Corner| {
                let n = mesh.normals[i].map(f64::from);
                key(mesh.positions[i]) == key(c(s.p).map(|x| x as f32))
                    && close(n, c(s.n), 1e-5)
                    && (!has_tangents || {
                        let t = part.tangents[i];
                        close([t[0], t[1], t[2]].map(f64::from), c(s.t), 1e-5)
                    })
                    && (f64::from(mesh.uv[i][0]) - s.uv[0]).abs() < 1e-6
                    && (f64::from(mesh.uv[i][1]) - s.uv[1]).abs() < 1e-6
            };
            for (j, t) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
                let t = t.map(|i| i as usize);
                let mut pk = t.map(|i| key(mesh.positions[i]));
                pk.sort();
                let candidates = game.get_mut(&pk).unwrap_or_else(|| {
                    panic!(
                        "{k} mesh {}: triangle {j} is none of the game's",
                        part.source_mesh
                    )
                });
                let [a, b, cc] = t.map(|i| mesh.positions[i].map(f64::from));
                let ours = cross(sub(b, a), sub(cc, a));
                let found = candidates.iter().enumerate().find_map(|(x, g)| {
                    // the game's front side: opposite cross(b - a, c - a) in its own frame
                    let front =
                        c(cross(sub(src[g[1]].p, src[g[0]].p), sub(src[g[2]].p, src[g[0]].p)).map(|x| -x));
                    if cull && dot(ours, front) <= 0.0 {
                        return None;
                    }
                    let corners: Option<Vec<usize>> = t
                        .iter()
                        .map(|&i| g.iter().copied().find(|&s| same_corner(i, &src[s])))
                        .collect();
                    corners.map(|cs| (x, cs))
                });
                let Some((x, corners)) = found else {
                    panic!(
                        "{k} mesh {} ({}culled): triangle {j} faces the other way from the game's, or its corners differ",
                        part.source_mesh,
                        if cull { "" } else { "not " }
                    )
                };
                candidates.swap_remove(x);
                triangles += 1;
                culled += usize::from(cull);
                if !has_tangents {
                    continue;
                }
                for (&i, &s) in t.iter().zip(&corners) {
                    let tw = part.tangents[i];
                    let ours_b = unit(
                        cross(
                            mesh.normals[i].map(f64::from),
                            [tw[0], tw[1], tw[2]].map(f64::from),
                        )
                        .map(|x| x * f64::from(tw[3])),
                    );
                    let game_b = unit(c(cross(src[s].n, src[s].t).map(|x| x * src[s].w)));
                    assert!(
                        close(ours_b, game_b, 1e-4),
                        "{k} mesh {} vertex {i}: bitangent {ours_b:?}, the game's under C {game_b:?}",
                        part.source_mesh
                    );
                    corners_checked += 1;
                    signs[usize::from(tw[3] < 0.0)] = true;
                }
            }
            // and the other way: every triangle of the game's is one of ours, but those without area (the reader
            // drops them)
            for (pk, left) in &game {
                for g in left {
                    let f = cross(sub(src[g[1]].p, src[g[0]].p), sub(src[g[2]].p, src[g[0]].p));
                    assert!(
                        dot(f, f) <= 1e-14,
                        "{k} mesh {}: the game's triangle {g:?} at {pk:?} is none of ours",
                        part.source_mesh
                    );
                    zero_area += 1;
                }
            }
        }
        models += 1;
    }
    assert!(models > 0 && culled > 0 && corners_checked > 0);
    assert!(
        signs == [true; 2],
        "the model must hold tangents of both handedness signs"
    );
    println!(
        "{models} models: {triangles} triangles ({culled} culled, {zero_area} of the game's without area left out) \
         and {corners_checked} tangent corners of both handedness signs match"
    );
}
