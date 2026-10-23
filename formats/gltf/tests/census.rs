//! Reader and writer census over third-party files, the Khronos glTF-Sample-Assets
//! (`oracles.khronos.general/sample-assets`: one folder per model, its `.glb`, else its `.gltf` with the buffers beside
//! it): every mesh of every model is read by the strict reader, re-written by our writer and read back; the gate is a
//! bit-exact match of every stream and index list. Each mesh is also encoded alone by primase-glb's single-mesh
//! encoder, which must write what the writer writes for it as an unplaced prototype without materials, byte for byte
//! but the generator (each names its crate). The configured folder must yield at least one model.
//!
//! A skinned model's `JOINTS_n`/`WEIGHTS_n` streams are not part of the mesh-alone round trip: glTF holds them only
//! with a skin (the writer and the encoder both leave them out of a mesh placed without a rig, counted), so the
//! re-written mesh must equal the original without them, and the primitives whose skin streams were left out are
//! counted in the report.

// Test code: helpers outside #[test] functions unwrap too (clippy.toml allows it only inside them).
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use primase_core::gate::{GateReport, Tolerance};
use primase_core::{Mesh, Name, Primitive, Semantic, Transform, World};
use primase_glb::Gltf;
use primase_gltf::{GroupBy, WriteOptions, write};
use primase_testdata::{Game, OracleSet, Query, expect_found, resolve};

/// A model folder's document: its `.glb`, else its `.gltf` with the buffers beside it.
fn model(dir: &Path) -> Result<Gltf, String> {
    let name = dir.file_name().unwrap().to_string_lossy().into_owned();
    let glb = dir.join(format!("{name}.glb"));
    if glb.exists() {
        return Gltf::from_glb(&std::fs::read(&glb).unwrap()).map_err(|e| e.to_string());
    }
    let json = std::fs::read(dir.join(format!("{name}.gltf"))).map_err(|e| e.to_string())?;
    Gltf::from_gltf(&json, |_, uri| {
        std::fs::read(dir.join(uri)).map_err(|e| e.to_string())
    })
    .map_err(|e| e.to_string())
}

/// A primitive's streams as the mesh-alone round trip keeps them: without joints and weights, which glTF holds only
/// with a skin.
fn kept_streams(p: &Primitive) -> String {
    let kept: std::collections::BTreeMap<_, _> = p
        .streams
        .iter()
        .filter(|(s, _)| !matches!(s, Semantic::Joints(_) | Semantic::Weights(_)))
        .collect();
    format!("{kept:?}")
}

/// Every mesh of each model folder's document read, re-written and read back, and encoded alone: the meshes, their
/// triangles and the primitives whose skin streams the round trip leaves out.
fn census(models: &[PathBuf], gate: &mut GateReport) -> (u64, u64, u64) {
    let (mut meshes, mut triangles, mut skinned) = (0u64, 0u64, 0u64);
    for dir in models {
        let item = dir.display().to_string();
        let gltf = match model(dir) {
            Ok(g) => g,
            Err(e) => {
                gate.fail(&item, e);
                continue;
            }
        };
        let mut world = World::new();
        let mut originals = Vec::new();
        for i in 0..gltf.mesh_count() {
            match gltf.read_mesh(i) {
                Ok(m) => {
                    let mesh = m.to_mesh();
                    triangles += mesh
                        .primitives()
                        .iter()
                        .map(|p| p.indices.as_ref().map_or(p.vertex_count(), Vec::len) as u64 / 3)
                        .sum::<u64>();
                    let name = format!("mesh_{i}");
                    match single_mesh_agrees(&name, &mesh) {
                        Ok(()) => gate.record(format!("{item} mesh {i} alone"), 0.0),
                        Err(why) => gate.fail(format!("{item} mesh {i} alone"), why),
                    }
                    let e = world.spawn();
                    world.insert(e, Transform::IDENTITY);
                    world.insert(e, mesh.clone());
                    originals.push(mesh);
                }
                Err(e) => gate.fail(format!("{item} mesh {i}"), e.to_string()),
            }
        }
        meshes += originals.len() as u64;
        skinned += originals
            .iter()
            .flat_map(|m| m.primitives())
            .filter(|p| {
                p.streams
                    .keys()
                    .any(|s| matches!(s, Semantic::Joints(_) | Semantic::Weights(_)))
            })
            .count() as u64;
        let options = WriteOptions {
            group_by: GroupBy::None,
            ..WriteOptions::default()
        };
        let back = write(&world, &options)
            .map_err(|e| e.to_string())
            .and_then(|(bytes, _)| Gltf::from_glb(&bytes).map_err(|e| e.to_string()));
        let back = match back {
            Ok(b) => b,
            Err(e) => {
                gate.fail(&item, format!("re-write: {e}"));
                continue;
            }
        };
        for (i, want) in originals.iter().enumerate() {
            let got: Mesh = back.read_mesh(i).unwrap().to_mesh();
            let same = got.primitives().len() == want.primitives().len()
                && got.primitives().iter().zip(want.primitives()).all(|(a, b)| {
                    kept_streams(a) == kept_streams(b)
                        && !a
                            .streams
                            .keys()
                            .any(|s| matches!(s, Semantic::Joints(_) | Semantic::Weights(_)))
                        && a.indices == b.indices
                        && a.topology == b.topology
                });
            if same {
                gate.record(format!("{item} mesh {i}"), 0.0);
            } else {
                gate.fail(format!("{item} mesh {i}"), "re-written mesh differs");
            }
        }
    }
    (meshes, triangles, skinned)
}

/// primase-glb's single-mesh encoder against the writer's GLB of `mesh` as the only, unplaced prototype (its
/// primitives' materials cleared): the same JSON, key order included, and the same buffer, the generator aside.
fn single_mesh_agrees(name: &str, mesh: &Mesh) -> Result<(), String> {
    let mut bare = mesh.clone();
    for mut p in bare.primitives_mut() {
        *p.material_mut() = None;
    }
    let mut w = World::new();
    let e = w.spawn();
    w.insert(e, bare);
    w.insert(e, Name(name.to_owned()));
    let options = WriteOptions {
        include_unplaced: true,
        ..WriteOptions::default()
    };
    let theirs = write(&w, &options).map_err(|e| format!("writer: {e}"))?.0;
    let ours = primase_glb::encode_mesh(name, mesh).map_err(|e| format!("encoder: {e}"))?;
    let mut docs = [ours, theirs].map(|b| primase_glb::Document::parse(&b).unwrap());
    let generators = docs
        .each_mut()
        .map(|d| std::mem::take(&mut d.json["asset"]["generator"]));
    let version = env!("CARGO_PKG_VERSION");
    let want = [
        format!("primase-glb {version}"),
        format!("primase-gltf {version}"),
    ]
    .map(serde_json::Value::from);
    if generators != want {
        return Err(format!("generators {generators:?}"));
    }
    let [a, b] = &docs;
    if serde_json::to_string(&a.json).unwrap() != serde_json::to_string(&b.json).unwrap() {
        return Err("the JSON differs".into());
    }
    if a.bin != b.bin {
        return Err("the buffer differs".into());
    }
    Ok(())
}

#[test]
fn khronos_meshes_read_and_round_trip() {
    let samples = Query::Oracle(OracleSet::Khronos, Game::General, "sample-assets");
    let Some(root) = resolve(samples) else {
        return;
    };
    let mut models: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    models.sort();
    expect_found(samples, "model folders", models.len());
    let mut gate = GateReport::new(format!("glb census {}", root.display()), Tolerance::Exact);
    let (meshes, triangles, skinned) = census(&models, &mut gate);
    println!(
        "{gate}\n  {} models, {meshes} meshes, {triangles} triangles, {skinned} primitives with skin streams left out",
        models.len()
    );
    gate.assert_passed();
}
