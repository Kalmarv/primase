//! A GIDynResource read onto an entity of its own.

use anvil_formats::archive::{Archive, At};
use anvil_formats::gidyn::{GI_DYN_RESOURCE, StoredGiDyn};
use primase_core::{Entity, Name, World, frame::SourceFrame};

/// Explicit single-resource read. Area readers must select cells before calling this: never load all GI by default.
pub fn read_gi_dyn(
    archive: &mut Archive,
    world: &mut World,
    at: At,
    frame: SourceFrame,
) -> Result<Entity, String> {
    let name = archive
        .forge(at.forge)
        .and_then(|f| f.names.get(at.entry))
        .map(|n| n.name.clone())
        .unwrap_or_default();
    let resource = archive.resource(at).map_err(|e| e.to_string())?;
    if resource.type_id != GI_DYN_RESOURCE {
        return Err(format!("GIDynResource: type {:#x}", resource.type_id));
    }
    let stored = StoredGiDyn::parse(&resource.body)?;
    let entity = world.spawn();
    world.insert(entity, Name(name));
    world.insert(
        entity,
        crate::components::AnvilGiDyn {
            index_id: resource.class_id,
            frame,
            stored,
        },
    );
    Ok(entity)
}
