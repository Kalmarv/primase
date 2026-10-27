//! One asset of a player read into a world, as a preview: a Mesh, a Texture2D or a Material, without a scene.
//!
//! A mesh is a prototype (its submeshes that have indices as primitives, as the level read makes them, no material:
//! a mesh does not say which it is drawn with) and one placement of it at Unity's origin, so a world draws it. A
//! texture is a `core.texture` entity (pixels as the level read decodes them, or why not) with its `unity.texture`. A
//! material is a `unity.material` entity with the textures its properties name, read as they are in a level. Nothing
//! else of the player is read: what a mesh's renderers or a material's users draw it with is in the scenes.
//!
//! The reading is the level read's own ([`crate::level::read_level`] builds its prototypes, materials and textures with
//! the same code), so an asset is read the same whichever way it is reached.

use primase_core::{Entity, Instance, Mesh, Name, World};

use crate::components::{UNITY, UnityMesh, UnityObject};
use crate::invalid;
use crate::level::{Builder, LevelReport, mesh_source, transform_of};
use unity_formats::Result;
use unity_formats::mesh::UnityMeshData;
use unity_formats::player::{ObjRef, Player};
use unity_formats::scene::IDENTITY;

/// What reading an asset made.
#[derive(Clone, Debug)]
pub struct AssetRead {
    /// The class read: `Mesh`, `Texture2D` or `Material`.
    pub class: &'static str,
    /// What the asset is as a world entity: a mesh's placement, a texture's or a material's own entity.
    pub entity: Entity,
    /// The counts and notices the read gives, as a level's (only what applies is non-zero).
    pub report: LevelReport,
}

/// The classes [`read_asset`] reads.
pub const ASSET_CLASSES: [&str; 3] = ["Mesh", "Texture2D", "Material"];

/// Read the object `r` of `player` into `w` (see the module documentation). An object of another class is an error
/// naming it.
pub fn read_asset(player: &Player, r: &ObjRef, w: &mut World) -> Result<AssetRead> {
    let class = ASSET_CLASSES
        .into_iter()
        .find(|c| player.class_name(r) == Some(c))
        .ok_or_else(|| {
            invalid(format!(
                "{}/{}: class {} is not one read as an asset ({})",
                r.file,
                r.path_id,
                player.class_name(r).unwrap_or("unknown"),
                ASSET_CLASSES.join(", ")
            ))
        })?;
    let mut b = Builder::new(player, LevelReport::default());
    let entity = match class {
        "Texture2D" => b.texture(w, r)?,
        "Material" => b.material(w, r)?,
        _ => {
            let data = UnityMeshData::read(player, r)?;
            let (read, notices) = crate::read::mesh::primitives(&data, 0..data.submeshes.len())?;
            b.note_mesh(notices);
            let (drawn, primitives): (Vec<u32>, Vec<_>) =
                read.into_iter().map(|(k, p)| (k as u32, p)).unzip();
            let prototype = w.spawn();
            if !primitives.is_empty() {
                b.report.primitives += primitives.len();
                w.insert(prototype, Mesh::new(primitives));
            }
            w.insert(
                prototype,
                UnityMesh {
                    mesh: UnityObject::from(r),
                    submeshes: None,
                    drawn,
                },
            );
            w.insert(prototype, mesh_source(r, None));
            let placement = w.spawn();
            w.insert(placement, Name(data.name.clone()));
            w.insert(placement, UNITY.transform(&transform_of(&IDENTITY)));
            w.insert(
                placement,
                Instance {
                    prototype,
                    variant: 0,
                },
            );
            b.report.prototypes = 1;
            b.report.renderers = 1;
            placement
        }
    };
    Ok(AssetRead {
        class,
        entity,
        report: b.report,
    })
}
