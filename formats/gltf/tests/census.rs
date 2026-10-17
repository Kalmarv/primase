//! Reader census over real scene folders: every mesh of every `prototypes/*.glb` (written by the games' existing
//! exporters, not by us) is read by the strict reader, re-written by our writer and read back; the gate is a
//! bit-exact match of every stream and index list. Skipped unless `PRIMASE_SCENES_SEKIRO` (its `scene_*`
//! subfolders) or `PRIMASE_SCENES_GTA` (all subfolders) is set.

// Test code: helpers outside #[test] functions unwrap too (clippy.toml allows it only inside them).
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use primase_core::gate::{GateReport, Tolerance};
use primase_core::{Mesh, Transform, World};
use primase_gltf::{Gltf, GroupBy, WriteOptions, write};
use primase_testdata::real_data;

fn scene_folders(var: &str, prefix: &str) -> Vec<PathBuf> {
    let Some(root) = real_data(var) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.join("prototypes").is_dir() && p.file_name().unwrap().to_string_lossy().starts_with(prefix)
        })
        .collect();
    out.sort();
    out
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
        let back = write(&world, &options).and_then(|(bytes, _)| Gltf::from_glb(&bytes));
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

fn run(var: &str, prefix: &str) {
    for folder in scene_folders(var, prefix) {
        let mut gate = GateReport::new(format!("glb census {}", folder.display()), Tolerance::Exact);
        let (meshes, triangles) = census(&folder, &mut gate);
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
