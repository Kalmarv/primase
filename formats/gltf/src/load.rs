//! The pass that loads `core.mesh_file` prototypes into `core.mesh`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_core::{Entity, Mesh, MeshFile, World};

use crate::read::Gltf;
use crate::{Error, Result};

/// What [`load_mesh_files`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadReport {
    /// Prototypes that got a `core.mesh`.
    pub meshes: u64,
    /// Distinct GLB files read.
    pub files: u64,
    pub primitives: u64,
}

/// For every entity with a `core.mesh_file`, read that mesh of its GLB (paths relative to `base_dir`) and attach it
/// as `core.mesh`, with `Primitive::material` from the file's `materials` list. Requires: `core.mesh_file`.
/// Provides: `core.mesh` (replacing any). Keeps `core.mesh_file` as the mesh's origin. A GLB that several prototypes
/// share is parsed once. Errors name the entity and file: unreadable or invalid GLB, a mesh index the file does not
/// have, or a `materials` list whose length is not the mesh's primitive count.
pub fn load_mesh_files(world: &mut World, base_dir: &Path) -> Result<LoadReport> {
    let jobs: Vec<(Entity, MeshFile)> = world.iter::<MeshFile>().map(|(e, f)| (e, f.clone())).collect();
    let mut report = LoadReport::default();
    let mut files: BTreeMap<PathBuf, Gltf> = BTreeMap::new();
    for (e, file) in jobs {
        let path = base_dir.join(&file.path);
        if !files.contains_key(&path) {
            let bytes = std::fs::read(&path).map_err(|source| Error::Io {
                path: path.clone(),
                source,
            })?;
            let gltf = Gltf::from_glb(&bytes).map_err(|e| e.in_file(&path))?;
            files.insert(path.clone(), gltf);
            report.files += 1;
        }
        let gltf = &files[&path];
        let read = gltf
            .read_mesh(file.mesh as usize)
            .map_err(|err| err.in_file(&path))?;
        if read.primitives.len() != file.materials.len() {
            return Err(Error::World {
                entity: e,
                detail: format!(
                    "{} mesh {} has {} primitives, core.mesh_file lists {} materials",
                    path.display(),
                    file.mesh,
                    read.primitives.len(),
                    file.materials.len()
                ),
            });
        }
        let mut mesh: Mesh = read.to_mesh();
        for (p, m) in mesh.primitives.iter_mut().zip(&file.materials) {
            p.material = *m;
        }
        report.primitives += mesh.primitives.len() as u64;
        report.meshes += 1;
        world.insert(e, mesh);
    }
    Ok(report)
}
