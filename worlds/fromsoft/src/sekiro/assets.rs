use crate::{Result, invalid};
use primase_core::{Mesh, MeshFile, Name, Texture, TextureFile, World};
use primase_skate_scene::{SkatePrototype, SkateTexture};
use std::path::Path;
fn asset_path(directory: &Path, relative: &str) -> Result<std::path::PathBuf> {
    if relative.is_empty()
        || relative.contains(['\\', ':'])
        || relative.split('/').any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err(invalid("Sekiro assets", 0, "invalid relative asset path"));
    }
    Ok(directory.join(relative))
}
/// Materialize immutable prototype/texture files for the file-backed scene writer.
/// The directory must be empty. Geometry encoding is delegated to the glTF writer;
/// PNG encoding to the image toolkit. Source normal conventions stay explicit.
pub fn prepare_scene_assets(world: &mut World, directory: &Path) -> Result<()> {
    if directory.exists() && std::fs::read_dir(directory)?.next().is_some() {
        return Err(invalid("Sekiro assets", 0, "directory is not empty"));
    }
    std::fs::create_dir_all(directory)?;
    let prototypes: Vec<_> = world
        .iter::<SkatePrototype>()
        .map(|(e, p)| (e, p.clone()))
        .collect();
    for (e, p) in prototypes {
        let Some(file) = p.glb else { continue };
        let mesh = world
            .get::<Mesh>(e)
            .ok_or_else(|| invalid("Sekiro assets", 0, "missing mesh"))?
            .clone();
        let materials = mesh.primitives.iter().map(|p| p.material).collect();
        let mut glb_world = World::default();
        let mut local_mesh = mesh;
        // Scene materials are linked by the separate manifest; GLB geometry has
        // no fabricated PBR approximation or embedded texture duplication.
        for p in &mut local_mesh.primitives {
            p.material = None;
        }
        let prototype = glb_world.spawn();
        glb_world.insert(prototype, local_mesh);
        glb_world.insert(prototype, Name(p.id));
        let path = asset_path(directory, &file.path)?;
        std::fs::create_dir_all(path.parent().expect("asset parent"))?;
        primase_gltf::write_file(
            &glb_world,
            &primase_gltf::WriteOptions {
                include_unplaced: true,
                ..Default::default()
            },
            &path,
        )
        .map_err(|e| invalid("Sekiro GLB", 0, e))?;
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
        .map(|(e, t)| (e, t.file.path.clone()))
        .collect();
    for (e, file) in textures {
        let texture = world
            .get::<Texture>(e)
            .ok_or_else(|| invalid("Sekiro assets", 0, "missing texture"))?;
        let bytes = primase_image::texture_level_png(texture, 0).map_err(|e| invalid("Sekiro PNG", 0, e))?;
        let path = asset_path(directory, &file)?;
        std::fs::create_dir_all(path.parent().expect("asset parent"))?;
        std::fs::write(&path, bytes)?;
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
            asset_path(Path::new("output"), "textures/a.dds").unwrap(),
            Path::new("output/textures/a.dds")
        );
    }
}
