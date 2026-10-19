//! The pass that loads `core.mesh_file` prototypes into `core.mesh`.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "exception: a loader pass reads GLB files; loading belongs to a reader's load step"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_core::{Entity, Mesh, MeshFile, World};

use crate::read::Gltf;
use crate::{Error, Result};

/// The box of a `core.mesh_file`'s mesh in its own space, `[min, max]` (metres), from its POSITION accessors' `min`
/// and `max` (glTF requires them), reading only the GLB's header and JSON chunk: what a planner that loads meshes
/// later needs to know first. `None` for a mesh without positions. The path is relative to `base_dir`.
pub fn mesh_file_box(base_dir: &Path, file: &MeshFile) -> Result<Option<[[f64; 3]; 2]>> {
    use std::io::Read;
    let path = base_dir.join(&file.path);
    let in_file = |e: Error| e.in_file(&path);
    let io = |source| Error::Io {
        path: path.clone(),
        source,
    };
    let mut f = std::fs::File::open(&path).map_err(io)?;
    let len = f.metadata().map_err(io)?.len();
    let mut head = [0u8; 20];
    f.read_exact(&mut head).map_err(io)?;
    let glb = |offset, detail: &str| {
        in_file(Error::Glb {
            offset,
            detail: detail.into(),
        })
    };
    if &head[..4] != b"glTF" {
        return Err(glb(0, "not a GLB"));
    }
    if &head[16..20] != b"JSON" {
        return Err(glb(16, "the first chunk is not JSON"));
    }
    let json_len = u64::from(u32::from_le_bytes([head[12], head[13], head[14], head[15]]));
    if json_len > len.saturating_sub(20) {
        return Err(glb(12, "the JSON chunk runs past the end of the file"));
    }
    let mut json = vec![0u8; json_len as usize];
    f.read_exact(&mut json).map_err(io)?;
    let doc: serde_json::Value = serde_json::from_slice(&json).map_err(|e| in_file(e.into()))?;
    let invalid = |item: String, detail: &str| {
        in_file(Error::Invalid {
            item,
            detail: detail.into(),
        })
    };
    let item = format!("meshes[{}].primitives()", file.mesh);
    let primitives = doc["meshes"][file.mesh as usize]["primitives"]
        .as_array()
        .ok_or_else(|| invalid(item, "missing"))?;
    let mut b = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
    for p in primitives {
        let Some(a) = p["attributes"]["POSITION"].as_u64() else {
            continue;
        };
        let accessor = &doc["accessors"][a as usize];
        for (side, key) in ["min", "max"].into_iter().enumerate() {
            for k in 0..3 {
                let v = accessor[key][k]
                    .as_f64()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| {
                        invalid(format!("accessors[{a}].{key}"), "POSITION needs 3 finite numbers")
                    })?;
                b[side][k] = if side == 0 { b[0][k].min(v) } else { b[1][k].max(v) };
            }
        }
    }
    Ok(b[0][0].is_finite().then_some(b))
}

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
        for (p, m) in mesh.primitives_mut().iter_mut().zip(&file.materials) {
            p.material = *m;
        }
        report.primitives += mesh.primitives().len() as u64;
        report.meshes += 1;
        world.insert(e, mesh);
    }
    Ok(report)
}

/// [`load_mesh_files`] as a pass (`load-mesh-files{base_dir=...}`; paths a reader resolved are absolute, so the
/// option is only for relative ones).
#[derive(Clone, Debug, Default)]
pub struct LoadMeshFiles {
    pub base_dir: PathBuf,
}

impl primase_core::pass::Pass for LoadMeshFiles {
    fn name(&self) -> &'static str {
        "load-mesh-files"
    }
    fn options(&self) -> primase_core::pass::Options {
        let mut o = primase_core::pass::Options::new();
        if !self.base_dir.as_os_str().is_empty() {
            o.insert("base_dir".into(), self.base_dir.to_string_lossy().into_owned());
        }
        o
    }
    fn requires(&self) -> &[&'static str] {
        &["core.mesh_file"]
    }
    fn provides(&self) -> &[&'static str] {
        &["core.mesh"]
    }
    fn run(&self, w: &mut World, log: &mut primase_core::pass::PassLog) -> std::result::Result<(), String> {
        let r = load_mesh_files(w, &self.base_dir).map_err(|e| e.to_string())?;
        log.count("meshes", r.meshes);
        log.count("files", r.files);
        log.count("primitives", r.primitives);
        Ok(())
    }
}

/// Register this crate's passes.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    crate::prepare::register(reg);
    reg.register("load-mesh-files", |o| {
        let mut p = LoadMeshFiles::default();
        for (k, v) in o {
            match k.as_str() {
                "base_dir" => p.base_dir = PathBuf::from(v),
                _ => return Err(format!("unknown option {k}")),
            }
        }
        Ok(Box::new(p))
    });
}
