//! The STUNTBOOST reader against the game's own code (`installs.stuntboost`, the install, and
//! `oracles.game-code.stuntboost/levels`, the game's parsers' dumps of it: `<L>_<N>.json` and `<L>_<N>.tris.bin`),
//! every level part the oracle dumped.
//!
//! - G1: every node name as the game's `ParsedName` reads it (visible, aggregate, bake shadows, inner name, code).
//! - G2: which nodes collide and how (the game's collision meshes, in node order, against the names), and every
//!   collision triangle of the game's collision model as this crate reads the file, exactly (f32 bits, load order),
//!   with its grippable bits and ramp (ramps compared by first appearance: the oracle numbers them across its whole
//!   run, the file per part).
//! - G3: the grind ledges (type, corners, normal, tangent, binormal, angles, length, group), the corner count and each
//!   part's grippable-bit counts: a solid part's as read, a trigger part's none read and every one 0 in the game (its
//!   file stores grippable bits for solid triangles only, and the game leaves the others at 0).

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

mod inputs;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use inputs::{Game, OracleSet, install, oracle};
use primase_stuntboost::components::names::parse;
use primase_stuntboost::components::{StuntboostBake, StuntboostCollision};
use primase_stuntboost::read::derived;
use serde_json::Value;

/// Every dumped part: (level, part, its JSON).
fn parts(oracle: &Path) -> Vec<(String, u32, Value)> {
    let mut out = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(oracle)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    for f in files {
        let v: Value = serde_json::from_slice(&std::fs::read(&f).unwrap()).unwrap();
        let level = v["level"].as_str().unwrap().to_owned();
        let part = v["part"].as_u64().unwrap() as u32;
        out.push((level, part, v));
    }
    assert!(!out.is_empty(), "no dumps in {}", oracle.display());
    out
}

fn inputs() -> Option<(PathBuf, PathBuf)> {
    Some((
        install(Game::Stuntboost)?,
        oracle(OracleSet::GameCode, Game::Stuntboost, "levels")?,
    ))
}

fn collision_file(install: &Path, level: &str, part: u32) -> PathBuf {
    install
        .join("Content")
        .join("Models")
        .join("Export")
        .join(format!("Level_{level}_{part}Collision.xnb"))
}

fn f32s(v: &Value) -> Vec<f32> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap() as f32)
        .collect()
}

fn report(what: &str, mismatches: &[String]) {
    for m in mismatches.iter().take(20) {
        println!("{what}: {m}");
    }
    assert!(mismatches.is_empty(), "{what}: {} mismatches", mismatches.len());
}

/// G1: every node name parses as the game parses it.
#[test]
fn g1_node_names_parse_as_the_game_parses_them() {
    let Some((_, oracle)) = inputs() else { return };
    let mut mismatches = Vec::new();
    let mut names = 0;
    for (level, part, v) in parts(&oracle) {
        for node in v["nodes"].as_array().unwrap() {
            let Some(raw) = node["name"].as_str() else {
                continue;
            };
            names += 1;
            let n = parse(raw);
            let aggregate = match n.collision {
                StuntboostCollision::Solid => "Solid",
                StuntboostCollision::Trigger => "Transmissive",
                StuntboostCollision::None => "None",
            };
            let bake = match n.bake {
                StuntboostBake::Baked => "CastAndReceive",
                StuntboostBake::NotBakedTo => "Cast",
                StuntboostBake::HiddenInBake => "None",
            };
            let ours = (!n.invisible, aggregate, bake, n.name.as_str(), n.code.as_str());
            let theirs = (
                node["visible"].as_bool().unwrap(),
                node["aggregate"].as_str().unwrap(),
                node["bakeShadows"].as_str().unwrap(),
                node["innerName"].as_str().unwrap(),
                node["code"].as_str().unwrap(),
            );
            if ours != theirs {
                mismatches.push(format!(
                    "{level}_{part} {raw:?}: ours {ours:?}, the game's {theirs:?}"
                ));
            }
        }
    }
    println!("{names} names");
    report("G1", &mismatches);
}

/// Relabels ramp ids by first appearance (-1 stays): the oracle's and the file's numbering compared as partitions.
fn relabel(ids: impl Iterator<Item = i32>) -> Vec<i32> {
    let mut seen: BTreeMap<i32, i32> = BTreeMap::new();
    ids.map(|r| {
        if r < 0 {
            return -1;
        }
        let next = seen.len() as i32;
        *seen.entry(r).or_insert(next)
    })
    .collect()
}

/// A collision triangle as compared: mesh (node), part, corners, grippable bits, ramp.
type Tri = (i32, i32, [[f32; 3]; 3], u8, i32);

/// G2: the collision nodes and every collision triangle, exactly.
#[test]
fn g2_collision_nodes_and_triangles_are_the_games() {
    let Some((install, oracle)) = inputs() else { return };
    let mut mismatches = Vec::new();
    let mut triangles = 0usize;
    for (level, part, v) in parts(&oracle) {
        let Some(c) = v.get("collision") else { continue };
        let tag = format!("{level}_{part}");
        let model = derived::collision_model(&std::fs::read(collision_file(&install, &level, part)).unwrap())
            .unwrap();
        // which nodes collide, against the names, in node order
        let nodes = v["nodes"].as_array().unwrap();
        for m in c["meshes"].as_array().unwrap() {
            let i = m["node"].as_u64().unwrap() as usize;
            let theirs = m["aggregate"].as_str().unwrap();
            let name = nodes[i]["name"].as_str().unwrap_or("");
            let ours = match parse(name).collision {
                StuntboostCollision::Solid => "Solid",
                StuntboostCollision::Trigger => "Transmissive",
                StuntboostCollision::None => "None",
            };
            if ours != theirs {
                mismatches.push(format!(
                    "{tag} node {i} {name:?}: named {ours}, the game's mesh {theirs}"
                ));
            }
            let read = model.meshes[i].as_ref().map(|m| m.aggregate);
            let expected = match theirs {
                "Solid" => 0,
                "Transmissive" => 1,
                _ => 2,
            };
            if read != Some(expected) {
                mismatches.push(format!(
                    "{tag} node {i}: the file reads {read:?}, the game's mesh {theirs}"
                ));
            }
        }
        // every triangle, in load order: mesh, part, corners (bits), grippable, ramp
        let bin = std::fs::read(oracle.join(format!("{level}_{part}.tris.bin"))).unwrap();
        let record = 4 + 4 + 36 + 1 + 4;
        assert_eq!(bin.len() % record, 0, "{tag}: tris.bin length");
        let theirs: Vec<&[u8]> = bin.chunks(record).collect();
        let mut ours: Vec<Tri> = Vec::new();
        for (i, mesh) in model.meshes.iter().enumerate() {
            let Some(mesh) = mesh else { continue };
            for (j, p) in mesh.parts.iter().enumerate() {
                for (k, t) in p.triangles.iter().enumerate() {
                    let grip = p.grippable.get(k).copied().unwrap_or(0);
                    let ramp = p.ramps.get(k).copied().unwrap_or(-1);
                    ours.push((i as i32, j as i32, *t, grip, ramp));
                }
            }
        }
        if ours.len() != theirs.len() {
            mismatches.push(format!(
                "{tag}: {} triangles read, the game's {}",
                ours.len(),
                theirs.len()
            ));
            continue;
        }
        let le_i32 = |b: &[u8], at: usize| i32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        let le_f32 = |b: &[u8], at: usize| f32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        let our_ramps = relabel(ours.iter().map(|t| t.4));
        let their_ramps = relabel(theirs.iter().map(|b| le_i32(b, 45)));
        for (n, (o, b)) in ours.iter().zip(&theirs).enumerate() {
            let corners: [[f32; 3]; 3] =
                std::array::from_fn(|c| std::array::from_fn(|x| le_f32(b, 8 + 12 * c + 4 * x)));
            let same_corners = (0..3).all(|c| (0..3).all(|x| o.2[c][x].to_bits() == corners[c][x].to_bits()));
            if (o.0, o.1) != (le_i32(b, 0), le_i32(b, 4))
                || !same_corners
                || o.3 != b[44]
                || our_ramps[n] != their_ramps[n]
            {
                mismatches.push(format!("{tag} triangle {n}: ours {o:?}, the game's {corners:?}"));
            }
        }
        triangles += ours.len();
    }
    println!("{triangles} triangles");
    report("G2", &mismatches);
}

/// G3: the grind ledges, the corners and the grippable edges.
#[test]
fn g3_ledges_and_grippable_edges_are_the_games() {
    let Some((install, oracle)) = inputs() else { return };
    let mut mismatches = Vec::new();
    let mut ledges = 0usize;
    for (level, part, v) in parts(&oracle) {
        let Some(c) = v.get("collision") else { continue };
        let tag = format!("{level}_{part}");
        let model = derived::collision_model(&std::fs::read(collision_file(&install, &level, part)).unwrap())
            .unwrap();
        if model.corners.len() as u64 != c["ledgeCorners"].as_u64().unwrap() {
            mismatches.push(format!(
                "{tag}: {} corners, the game's {}",
                model.corners.len(),
                c["ledgeCorners"]
            ));
        }
        let theirs = c["ledges"].as_array().unwrap();
        if model.ledges.len() != theirs.len() {
            mismatches.push(format!(
                "{tag}: {} ledges, the game's {}",
                model.ledges.len(),
                theirs.len()
            ));
            continue;
        }
        for (i, (l, t)) in model.ledges.iter().zip(theirs).enumerate() {
            let kind = ["Edge", "RailNoBoost", "RailBoost"][l.kind as usize];
            let corners: Vec<f32> = l
                .corners
                .iter()
                .flat_map(|&k| model.corners[k as usize].position)
                .collect();
            let their_corners: Vec<f32> = t["corners"].as_array().unwrap().iter().flat_map(f32s).collect();
            let exact = |a: &[f32], b: &[f32]| {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
            };
            let near = |a: &[f32], b: &[f32]| {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() <= 1e-6)
            };
            let group = t["group"].as_str().map(str::to_owned);
            let ok = kind == t["type"].as_str().unwrap()
                && exact(&corners, &their_corners)
                && exact(&l.normal, &f32s(&t["normal"]))
                && exact(&l.tangent, &f32s(&t["tangent"]))
                && near(&l.binormal, &f32s(&t["binormal"]))
                && l.min_angle.to_bits() == (t["minAngle"].as_f64().unwrap() as f32).to_bits()
                && l.max_angle.to_bits() == (t["maxAngle"].as_f64().unwrap() as f32).to_bits()
                && l.length.to_bits() == (t["length"].as_f64().unwrap() as f32).to_bits()
                && group == Some(l.group.to_string());
            if !ok {
                mismatches.push(format!("{tag} ledge {i}: ours {l:?}, the game's {t}"));
            }
        }
        ledges += model.ledges.len();
        // each part's grippable-bit counts
        for m in c["meshes"].as_array().unwrap() {
            let i = m["node"].as_u64().unwrap() as usize;
            let Some(mesh) = &model.meshes[i] else { continue };
            for (j, p) in m["parts"].as_array().unwrap().iter().enumerate() {
                let part = &mesh.parts[j];
                let theirs: BTreeMap<String, u64> = (p["grippableBits"].as_object().unwrap().iter())
                    .map(|(k, v)| (k.clone(), v.as_u64().unwrap()))
                    .collect();
                // the file stores grippable bits for solid triangles only; the game leaves the others' at 0
                let ours: BTreeMap<String, u64> = if mesh.aggregate == 0 {
                    let mut counts = BTreeMap::new();
                    for g in &part.grippable {
                        *counts.entry(g.to_string()).or_default() += 1;
                    }
                    counts
                } else {
                    if !part.grippable.is_empty() {
                        mismatches.push(format!(
                            "{tag} node {i} part {j}: grippable bits read for a trigger"
                        ));
                    }
                    let n = part.triangles.len() as u64;
                    if n == 0 {
                        BTreeMap::new()
                    } else {
                        BTreeMap::from([("0".to_owned(), n)])
                    }
                };
                if ours != theirs {
                    mismatches.push(format!(
                        "{tag} node {i} part {j}: grippable {ours:?}, the game's {theirs:?}"
                    ));
                }
            }
        }
    }
    println!("{ledges} ledges");
    report("G3", &mismatches);
}
