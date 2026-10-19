//! Files for the file-backed writer: the prototype GLBs and texture PNGs a skate world names but holds in memory.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "exception: writes assets and repoints the world before the writer runs"
)]

use std::path::{Path, PathBuf};

use primase_core::{Mesh, MeshFile, Name, Texture, TextureFile, World};

use crate::dialect::{SkatePrototype, SkateTexture};
use crate::{Error, Result};

/// `relative` inside `directory`, refused when it would leave it.
fn asset_path(directory: &Path, relative: &str) -> Result<PathBuf> {
    if relative.is_empty()
        || relative.contains(['\\', ':'])
        || relative.split('/').any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err(Error::Write(format!(
            "asset path '{relative}' leaves the output folder"
        )));
    }
    Ok(directory.join(relative))
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_owned(),
        source,
    }
}

/// Write every prototype's mesh (a `skate.prototype` with a GLB and a `core.mesh`) as its GLB and every texture's
/// level 0 (a `skate.texture` with a `core.texture`) as its PNG into `directory`, which must be empty, and point the
/// entities at the files (`core.mesh_file`, `core.texture_file`) for [`super::write`]. A GLB holds geometry only (the
/// manifest links the materials); the glTF writer encodes it, the image toolkit the PNGs.
pub fn prepare_assets(world: &mut World, directory: &Path) -> Result<()> {
    if directory.exists()
        && std::fs::read_dir(directory)
            .map_err(io(directory))?
            .next()
            .is_some()
    {
        return Err(Error::Write(format!("{} is not empty", directory.display())));
    }
    std::fs::create_dir_all(directory).map_err(io(directory))?;
    let prototypes: Vec<_> = world
        .iter::<SkatePrototype>()
        .map(|(e, p)| (e, p.clone()))
        .collect();
    for (e, p) in prototypes {
        let Some(file) = p.glb else { continue };
        let Some(mesh) = world.get::<Mesh>(e).cloned() else {
            continue;
        };
        let materials = mesh.primitives().iter().map(|p| p.material).collect();
        let mut glb = World::default();
        let mut local = mesh;
        for p in local.primitives_mut() {
            p.material = None;
        }
        let proto = glb.spawn();
        glb.insert(proto, local);
        glb.insert(proto, Name(p.id.clone()));
        let path = asset_path(directory, &file.path)?;
        let parent = path.parent().expect("an asset has a parent");
        std::fs::create_dir_all(parent).map_err(io(parent))?;
        primase_gltf::write_file(
            &glb,
            &primase_gltf::WriteOptions {
                include_unplaced: true,
                ..Default::default()
            },
            &path,
        )
        .map_err(|e| Error::Write(format!("prototype {}: {e}", p.id)))?;
        world.insert(
            e,
            MeshFile {
                path,
                mesh: 0,
                materials,
            },
        );
    }
    let textures: Vec<_> = world
        .iter::<SkateTexture>()
        .map(|(e, t)| (e, t.clone()))
        .collect();
    for (e, t) in textures {
        let Some(texture) = world.get::<Texture>(e) else {
            continue;
        };
        let bytes = primase_image::texture_level_png(texture, 0)
            .map_err(|err| Error::Write(format!("texture {}: {err}", t.key)))?;
        let path = asset_path(directory, &t.file.path)?;
        let parent = path.parent().expect("an asset has a parent");
        std::fs::create_dir_all(parent).map_err(io(parent))?;
        std::fs::write(&path, bytes).map_err(io(&path))?;
        world.insert(e, TextureFile { path });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_paths_stay_in_output() {
        for path in ["../outside", "/absolute", "C:/absolute", "a\\b", "a/../b", ""] {
            assert!(asset_path(Path::new("output"), path).is_err(), "{path}");
        }
        assert_eq!(
            asset_path(Path::new("output"), "textures/a.png").unwrap(),
            Path::new("output/textures/a.png")
        );
    }
}
