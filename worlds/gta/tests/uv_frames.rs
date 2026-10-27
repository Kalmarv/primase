//! GTA's stored tangent frames are the frames its texture coordinates give, with the bitangent along +v: on them
//! `uv-frame-normals{bitangent=+v}` has nothing to do. Measured on `oracles.codewalker.gta/frames.json` (CodeWalker's
//! reading of one ydr's geometries as stored): per triangle, the stored frame its bump maps are drawn in (red along T,
//! green along GTA's binormal `cross(T, N) * w`, `GTA_NORMALS`) against the frame derived from UV0. The frame change to
//! the world is a rotation, so the comparison is made in GTA's frame. Triangles without a UV frame (degenerate
//! coordinates) are left out of the share.
#![allow(clippy::unwrap_used)]

mod inputs;

use inputs::{Game, OracleSet, oracle};
use primase_meshops::frames::{AxisMap, axis_map, uv_axes};
use primase_meshops::vector::Vec3;
use serde_json::Value as Json;
use std::collections::BTreeMap;

fn v3(j: &Json) -> [f32; 3] {
    std::array::from_fn(|k| j[k].as_f64().unwrap() as f32)
}

#[test]
fn stored_frames_are_the_uv_frames() {
    let Some(frames) = oracle(OracleSet::CodeWalker, Game::Gta, "frames.json") else {
        return;
    };
    let oracle: Json = serde_json::from_slice(&std::fs::read(&frames).unwrap()).unwrap();
    // per bitangent direction: area by map, and the area within 1e-3 of it
    let mut area = [BTreeMap::<AxisMap, (f64, f64)>::new(), BTreeMap::new()];
    let mut none = 0f64;
    for g in oracle["value"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| m["geometries"].as_array().unwrap())
    {
        let Some(tangents) = g["tangents"].as_array() else {
            continue;
        };
        let p: Vec<[f32; 3]> = g["positions"].as_array().unwrap().iter().map(v3).collect();
        let n: Vec<[f32; 3]> = g["normals"].as_array().unwrap().iter().map(v3).collect();
        let uv: Vec<[f32; 2]> = g["uv0"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| [c[0].as_f64().unwrap() as f32, c[1].as_f64().unwrap() as f32])
            .collect();
        let t: Vec<[f32; 4]> = tangents
            .iter()
            .map(|t| std::array::from_fn(|k| t[k].as_f64().unwrap() as f32))
            .collect();
        let indices: Vec<usize> = g["indices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i.as_u64().unwrap() as usize)
            .collect();
        for tri in indices.as_chunks::<3>().0 {
            let normal = tri.iter().fold(Vec3::ZERO, |s, &i| s + Vec3::from(n[i]));
            let weight = f64::from(
                (Vec3::from(p[tri[1]]) - Vec3::from(p[tri[0]]))
                    .cross(Vec3::from(p[tri[2]]) - Vec3::from(p[tri[0]]))
                    .length(),
            ) * 0.5;
            let Some((u, v)) = uv_axes(tri.map(|i| p[i]), tri.map(|i| uv[i]), normal.into()) else {
                none += weight;
                continue;
            };
            let mut st = Vec3::ZERO;
            let mut sb = Vec3::ZERO;
            for &i in tri {
                let tt = Vec3::new(t[i][0], t[i][1], t[i][2]);
                st += tt;
                sb += tt.cross(Vec3::from(n[i])) * t[i][3];
            }
            let nn = normal.normalize_or(Vec3::ZERO);
            let flat = |x: Vec3| <[f32; 3]>::from((x - nn * x.dot(nn)).normalize_or(Vec3::ZERO));
            for (k, side) in [1f32, -1.].into_iter().enumerate() {
                let target = [u, <[f32; 3]>::from(Vec3::from(v) * side)];
                let (map, error) = axis_map([flat(st), flat(sb)], target);
                let entry = area[k].entry(map).or_default();
                entry.0 += weight;
                if error <= 1e-3 {
                    entry.1 += weight;
                }
            }
        }
    }
    let total: f64 = area[1].values().map(|(a, _)| a).sum::<f64>() + none;
    assert!(total > 0., "no tangent frames in the dump");
    for (k, side) in ["+v", "-v"].into_iter().enumerate() {
        println!(
            "bitangent {side}: {:?} m2 by map (area, within 1e-3); no UV frame {none} m2; total {total} m2",
            area[k]
        );
    }
    let identity = area[0].get(&AxisMap::IDENTITY).copied().unwrap_or_default();
    assert!(
        identity.0 >= 0.98 * (total - none),
        "the +v frame is the stored one on {} of {} m2",
        identity.0,
        total - none
    );
}
