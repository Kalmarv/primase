//! Handedness of the GTA reader's frame change, on a real drawable against CodeWalker's reading of it.
//!
//! `oracles.codewalker.gta/frames.json` is a thin dump of CodeWalker's reading of one ydr's high-detail geometries as
//! stored, in GTA's frame: per vertex position, normal, tangent (xyzw), first texture coordinate, and the index buffer.
//! The facts checked against:
//! - the frame: east, north, up (right-handed); the world is `C (x, y, z) = (x, z, -y)` of it, a rotation;
//! - which side is a triangle's front: along `cross(b - a, c - a)` in GTA's coordinates, the side its stored normals
//!   face (nearly every geometry of the install; `gta-winding` turns the few that do not);
//! - the bitangent: GTA's vertex shaders take the binormal as `cross(T, N) * w` (glTF's is `cross(N, T) * w`).
//!
//! Checked: our primitives are CodeWalker's geometries in order, vertex for vertex; positions, normals and tangent
//! directions are `C` of the stored ones bit for bit (`C` only moves and negates); triangles keep their corners and
//! their front side `C` of GTA's; at every vertex our bitangent `cross(N, T) * w` is `C` of GTA's binormal.
#![allow(clippy::unwrap_used)]

use primase_core::{Instance, Mesh, Semantic, Stream, World};
use primase_gta::components::GtaArchetype;
use primase_gta::rage::map::{ArchetypeDef, ArchetypeKind};
use primase_gta::read::archive::archive_set::{ArchiveSet, Install, MapState};
use primase_gta::read::archive::keys::Keys;
use primase_gta::read::assets::attach_assets;
use primase_testdata::{Game, OracleSet, install, keys, oracle};
use serde_json::Value as Json;

fn v3(j: &Json) -> [f32; 3] {
    std::array::from_fn(|k| j[k].as_f64().unwrap() as f32)
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

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn unit(a: [f64; 3]) -> [f64; 3] {
    let l = dot(a, a).sqrt();
    if l == 0.0 { a } else { a.map(|x| x / l) }
}

/// GTA's frame to the world's.
fn c<T: Copy + std::ops::Neg<Output = T>>(v: [T; 3]) -> [T; 3] {
    [v[0], v[2], -v[1]]
}

#[test]
fn frame_change_keeps_fronts_and_bitangents() {
    let (Some(dir), Some(keys), Some(frames)) = (
        install(Game::Gta),
        keys(Game::Gta),
        oracle(OracleSet::CodeWalker, Game::Gta, "frames.json"),
    ) else {
        return;
    };
    let oracle: Json = serde_json::from_slice(&std::fs::read(&frames).unwrap()).unwrap();
    let path = oracle["path"].as_str().unwrap();
    let stem = path.rsplit('\\').next().unwrap().strip_suffix(".ydr").unwrap();
    let keys = Keys::from_dir(&keys).unwrap();
    let set = ArchiveSet::build(Install::open(&dir, &keys).unwrap(), &keys, MapState::Story).unwrap();
    // one prototype naming the drawable, one placement of it: what attach_assets reads for
    let mut w = World::new();
    let proto = w.spawn();
    let def = ArchetypeDef {
        kind: ArchetypeKind::Base,
        name: primase_hash::joaat(stem.as_bytes()),
        lod_dist: 100.0,
        flags: 0,
        special_attribute: 0,
        bb_min: [0.0; 3],
        bb_max: [0.0; 3],
        bs_centre: [0.0; 3],
        bs_radius: 0.0,
        hd_texture_dist: 0.0,
        texture_dictionary: 0,
        clip_dictionary: 0,
        drawable_dictionary: 0,
        physics_dictionary: 0,
        asset_type: 2,
        asset_name: primase_hash::joaat(stem.as_bytes()),
        time_flags: None,
        extensions: 0,
        interior: None,
    };
    w.insert(proto, GtaArchetype::new("test", &def));
    let p = w.spawn();
    w.insert(
        p,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    attach_assets(&mut w, &set, &keys).unwrap();
    let mesh = w.get::<Mesh>(proto).expect("the drawable's mesh");
    let geometries: Vec<&Json> = oracle["value"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| m["geometries"].as_array().unwrap())
        .collect();
    assert_eq!(mesh.primitives().len(), geometries.len(), "{stem}: primitives");
    let (mut triangles, mut tangents, mut negative) = (0usize, 0usize, 0usize);
    for (i, (prim, g)) in mesh.primitives().iter().zip(&geometries).enumerate() {
        let src_p: Vec<[f32; 3]> = g["positions"].as_array().unwrap().iter().map(v3).collect();
        let pos = prim.positions().unwrap();
        assert_eq!(pos.len(), src_p.len(), "{stem} geometry {i}: vertices");
        for (k, (a, b)) in pos.iter().zip(&src_p).enumerate() {
            assert_eq!(
                a.map(f32::to_bits),
                c(*b).map(f32::to_bits),
                "{stem} geometry {i} vertex {k}: position"
            );
        }
        let src_n: Vec<[f32; 3]> = g["normals"].as_array().unwrap().iter().map(v3).collect();
        let Some(Stream::F32x3(nor)) = prim.streams.get(&Semantic::Normal) else {
            panic!("{stem} geometry {i}: no normals")
        };
        for (k, (a, b)) in nor.iter().zip(&src_n).enumerate() {
            assert_eq!(
                a.map(f32::to_bits),
                c(*b).map(f32::to_bits),
                "{stem} geometry {i} vertex {k}: normal"
            );
        }
        let idx: Vec<u32> = g["indices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_u64().unwrap() as u32)
            .collect();
        assert_eq!(
            prim.indices.as_deref(),
            Some(&idx[..]),
            "{stem} geometry {i}: triangles in GTA's order"
        );
        for t in idx.as_chunks::<3>().0 {
            let [a, b, cc] = t.map(|j| pos[j as usize].map(f64::from));
            let [ga, gb, gc] = t.map(|j| src_p[j as usize].map(f64::from));
            let ours = cross(sub(b, a), sub(cc, a));
            let game = c(cross(sub(gb, ga), sub(gc, ga)));
            if dot(game, game) > 0.0 {
                assert!(
                    dot(ours, game) > 0.0,
                    "{stem} geometry {i}: a triangle faces the other way"
                );
            }
            triangles += 1;
        }
        let Some(src_t) = g["tangents"].as_array() else {
            continue;
        };
        let Some(Stream::F32x4(tan)) = prim.streams.get(&Semantic::Tangent) else {
            panic!("{stem} geometry {i}: stored tangents not read")
        };
        for (k, (t, s)) in tan.iter().zip(src_t).enumerate() {
            let s: [f64; 4] = std::array::from_fn(|j| s[j].as_f64().unwrap());
            assert_eq!(
                [t[0], t[1], t[2]].map(f32::to_bits),
                c([s[0], s[1], s[2]].map(|x| x as f32)).map(f32::to_bits),
                "{stem} geometry {i} vertex {k}: tangent direction"
            );
            let n = nor[k].map(f64::from);
            let ours = unit(cross(n, [t[0], t[1], t[2]].map(f64::from)).map(|x| x * f64::from(t[3])));
            let sn = src_n[k].map(f64::from);
            let gta = unit(c(cross([s[0], s[1], s[2]], sn).map(|x| x * s[3])));
            assert!(
                (0..3).all(|j| (ours[j] - gta[j]).abs() < 1e-6),
                "{stem} geometry {i} vertex {k}: bitangent {ours:?}, GTA's binormal under C {gta:?}"
            );
            tangents += 1;
            negative += usize::from(s[3] < 0.0);
        }
    }
    assert!(tangents > 0, "{stem}: no tangents to check");
    println!(
        "{stem}: {} geometries, {triangles} triangles, {tangents} tangents ({negative} with negative w) match",
        geometries.len()
    );
}
