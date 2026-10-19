//! Reader census over real scene folders: every mesh of every `prototypes/*.glb` (written by the games' existing
//! exporters, not by us) is read by the strict reader, re-written by our writer and read back; the gate is a
//! bit-exact match of every stream and index list. Each mesh is also encoded alone by primase-glb's single-mesh
//! encoder, which must write what the writer writes for it as an unplaced prototype without materials, byte for byte
//! but the generator (each names its crate). Skipped unless `PRIMASE_SCENES_SEKIRO` (scene folders whose own
//! name starts with `scene_`) or `PRIMASE_SCENES_GTA` (every scene folder) is set. A configured root must yield at
//! least one scene folder (a folder holding `scene.json` and `prototypes`).

// Test code: helpers outside #[test] functions unwrap too (clippy.toml allows it only inside them).
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use primase_core::gate::{GateReport, Tolerance};
use primase_core::{Mesh, Name, Transform, World};
use primase_glb::Gltf;
use primase_gltf::{GroupBy, WriteOptions, write};
use primase_testdata::real_data;

/// Every folder under `root` holding a `scene.json` and a `prototypes` folder whose own name starts with `prefix`
/// (empty: every name).
fn scene_folders(root: &Path, prefix: &str, out: &mut Vec<PathBuf>) {
    let named = |p: &Path| {
        p.file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(prefix))
    };
    if root.join("scene.json").is_file() && root.join("prototypes").is_dir() && named(root) {
        out.push(root.to_owned());
    }
    let Ok(rd) = std::fs::read_dir(root) else { return };
    let mut dirs: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    for d in dirs {
        scene_folders(&d, prefix, out);
    }
}

fn census(folder: &Path, gate: &mut GateReport) -> (u64, u64) {
    let mut files: Vec<PathBuf> = std::fs::read_dir(folder.join("prototypes"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("glb")))
        .collect();
    files.sort();
    let (mut meshes, mut triangles) = (0u64, 0u64);
    for f in &files {
        let item = f.display().to_string();
        let gltf = match Gltf::from_glb(&std::fs::read(f).unwrap()) {
            Ok(g) => g,
            Err(e) => {
                gate.fail(&item, e.to_string());
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
                    format!("{:?}", a.streams) == format!("{:?}", b.streams)
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
    (meshes, triangles)
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

fn run(var: &str, prefix: &str) {
    let Some(root) = real_data(var) else {
        return;
    };
    let mut folders = Vec::new();
    scene_folders(&root, prefix, &mut folders);
    primase_testdata::expect_items(var, "scene folders with prototypes", folders.len());
    for folder in &folders {
        let mut gate = GateReport::new(format!("glb census {}", folder.display()), Tolerance::Exact);
        let (meshes, triangles) = census(folder, &mut gate);
        println!("{gate}\n  {meshes} meshes, {triangles} triangles");
        gate.assert_passed();
    }
}

#[test]
fn sekiro_prototypes_read_and_round_trip() {
    run("PRIMASE_SCENES_SEKIRO", "scene_");
}

#[test]
fn gta_prototypes_read_and_round_trip() {
    run("PRIMASE_SCENES_GTA", "");
}
