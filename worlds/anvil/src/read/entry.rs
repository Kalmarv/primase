//! One forge entry read alone, as a preview: a Mesh, LODSelector or Impostor placed with its Materials, an Entity or
//! EntityGroup placed, a Material or a TextureMap, a LayeredSky or WorldAmbiance with its look's controllers, a GIDyn
//! resource, a look or forecast preset, a World, GridPartition or GridCellDataBlock's stored layout, or a coarse floor.
//! What the entry's kind can't be drawn from, or what was refused, is a notice.

use anvil_formats::archive::{Archive, At};
use anvil_formats::listing::Kind;
use primase_core::World;
use primase_core::convert::{Notice, NoticeKind};
use primase_core::frame::SourceFrame;

use crate::components::{
    AnvilDeferredDecals, AnvilFloorHeight, AnvilGridMetadata, AnvilLookPreset, AnvilRenderStatus, AnvilSky,
    AnvilWater,
};
use crate::read::convert::ForgeFile;
use crate::read::placements::{InstancedCollision, InstancedLevels, Placed, Placer};
use crate::read::sky::attach_look_controllers;
use crate::read::textures::read_texture_map;

/// A forge entry's preview.
#[derive(Debug)]
pub struct EntryRead {
    pub world: World,
    /// The entry's name in its forge's table.
    pub name: String,
    /// Its type id.
    pub type_id: u32,
    pub notices: Vec<Notice>,
}

/// Entry `entry` of the forge `forge` (an index into `forges`, the forges searched in order for the ids it names),
/// read in `frame`.
pub fn read_entry(
    forges: &[ForgeFile],
    forge: usize,
    entry: usize,
    frame: SourceFrame,
) -> Result<EntryRead, String> {
    let file = forges
        .get(forge)
        .ok_or_else(|| format!("no forge {forge} of {}", forges.len()))?;
    let forge_name = file
        .path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let paths: Vec<_> = forges.iter().map(|f| &f.path).collect();
    let mut archive = Archive::open(&paths).map_err(|e| format!("opening the forges: {e}"))?;
    let at = At { forge, entry };
    let listed = archive
        .forge(forge)
        .and_then(|f| f.listing().nth(entry))
        .ok_or_else(|| format!("anvil: {forge_name} has no entry {entry}"))?;
    let (name, id, kind, type_id) = (
        listed.name.to_owned(),
        listed.id,
        listed.kind,
        listed
            .type_id
            .ok_or_else(|| format!("anvil: {forge_name} entry {entry} has no type id"))?,
    );
    let mut world = World::new();
    let mut notices = Vec::new();
    let mut placed = Placed::default();
    match kind {
        Some(Kind::Entity | Kind::EntityGroup) => {
            Placer::new(&mut archive, &mut world, frame, InstancedLevels::Every)
                .with_instanced_collision(InstancedCollision::PerInstance)
                .with_materials()
                .with_shaders()
                .place(at, &mut placed)
                .map_err(|e| format!("placing {name}: {e}"))?;
        }
        Some(Kind::Mesh | Kind::LodSelector | Kind::Impostor) => {
            Placer::new(&mut archive, &mut world, frame, InstancedLevels::Every)
                .with_materials()
                .with_shaders()
                .place_object(at, &mut placed)
                .map_err(|e| format!("placing {name}: {e}"))?
                .ok_or_else(|| format!("{name}: no mesh level could be read"))?;
        }
        Some(Kind::Material) => {
            Placer::new(&mut archive, &mut world, frame, InstancedLevels::Every)
                .with_materials()
                .with_shaders()
                .material(id, &mut placed)
                .ok_or_else(|| format!("{name}: not a readable Material"))?;
        }
        Some(Kind::TextureMap) => {
            read_texture_map(&mut archive, &mut world, id)
                .map_err(|e| format!("reading texture map {name}: {e}"))?
                .ok_or_else(|| format!("{name}: a TextureMap with no image"))?;
        }
        Some(Kind::LayeredSky) => {
            let resource = archive.resource(at).map_err(|e| format!("reading {name}: {e}"))?;
            let sky = anvil_formats::sky::StoredSky::parse(&resource.body).map_err(|e| e.to_string())?;
            let entity = world.spawn();
            world.insert(entity, primase_core::Name(name.clone()));
            world.insert(
                entity,
                AnvilSky {
                    index_id: id,
                    stored: sky,
                },
            );
            if let Some(look) = name.strip_prefix("LS_") {
                attach_look_controllers(&mut archive, &mut world, entity, look, &mut notices);
            }
        }
        Some(Kind::WorldAmbiance) => {
            let resource = archive.resource(at).map_err(|e| format!("reading {name}: {e}"))?;
            let (sky_id, cube_id) =
                anvil_formats::sky::ambiance(&resource.body).map_err(|e| e.to_string())?;
            read_texture_map(&mut archive, &mut world, cube_id)
                .map_err(|error| format!("{name} environment cube: {error}"))?
                .ok_or_else(|| format!("{name}: environment cube {cube_id:#x} is missing"))?;
            let sky_at = archive
                .find(sky_id)
                .ok_or_else(|| format!("{name}: LayeredSky {sky_id:#x} missing"))?;
            let resource = archive
                .resource(sky_at)
                .map_err(|e| format!("reading ambiance sky: {e}"))?;
            if resource.type_id != anvil_formats::sky::LAYERED_SKY {
                return Err(format!("{name}: sky reference is type {:#x}", resource.type_id));
            }
            let stored = anvil_formats::sky::StoredSky::parse(&resource.body).map_err(|e| e.to_string())?;
            let entity = world.spawn();
            world.insert(entity, primase_core::Name(name.clone()));
            world.insert(
                entity,
                AnvilSky {
                    index_id: sky_id,
                    stored,
                },
            );
            if let Some(look) = name.strip_prefix("WA_") {
                attach_look_controllers(&mut archive, &mut world, entity, look, &mut notices);
            }
            notices.push(Notice::new(
                "anvil ambiance",
                NoticeKind::Unsupported,
                "sky, owned cube and available time/weather curves decoded; automatic sun elevation, lens flares and \
                 particle simulation are not evaluated",
                1,
            ));
        }
        Some(Kind::GiDynResource) => {
            crate::read::gidyn::read_gi_dyn(&mut archive, &mut world, at, frame)
                .map_err(|why| format!("reading {name}: {why}"))?;
            notices.push(Notice::new(
                "anvil GI",
                NoticeKind::Unsupported,
                "GIDyn probe positions and stored streams decoded; overlay is geometry only, probe irradiance and GPU \
                 GIDynData construction unresolved",
                1,
            ));
        }
        Some(Kind::AmbiancePreset | Kind::ForecastPreset) => {
            let resource = archive.resource(at).map_err(|e| format!("reading {name}: {e}"))?;
            let stored =
                anvil_formats::look::StoredLook::parse(type_id, &resource.body).map_err(|e| e.to_string())?;
            let entity = world.spawn();
            world.insert(entity, primase_core::Name(name.clone()));
            world.insert(entity, AnvilLookPreset { index_id: id, stored });
            notices.push(Notice::new(
                "anvil look preset",
                NoticeKind::Unsupported,
                "stored look values shown in Properties > Data; event-to-Season selection and forecast lane lowering \
                 unresolved; no world position is authored by this preset",
                1,
            ));
        }
        Some(Kind::World | Kind::GridPartition | Kind::GridCellDataBlock) => {
            use anvil_formats::grid::{StoredGridCellDataBlock, StoredGridPartition, grid_descriptions};
            let resource = archive.resource(at).map_err(|e| format!("reading {name}: {e}"))?;
            let data = match kind {
                Some(Kind::World) => AnvilGridMetadata::World(
                    grid_descriptions(&resource.body, file.edition.grid_layout_words())
                        .map_err(|e| format!("{name}: {e}"))?,
                ),
                Some(Kind::GridPartition) => AnvilGridMetadata::Partition(
                    StoredGridPartition::parse(&resource.body).map_err(|e| format!("{name}: {e}"))?,
                ),
                _ => AnvilGridMetadata::Cell(
                    StoredGridCellDataBlock::parse(&resource.body).map_err(|e| format!("{name}: {e}"))?,
                ),
            };
            let entity = world.spawn();
            world.insert(entity, primase_core::Name(name.clone()));
            world.insert(entity, data);
            notices.push(Notice::new(
                "anvil grid metadata",
                NoticeKind::Unsupported,
                "stored layouts/cell references in Properties > Data; this structural entry is not an area read; \
                 World fields beyond GridDescriptions remain unread",
                1,
            ));
        }
        Some(Kind::FloorHeightData) => {
            let resource = archive.resource(at).map_err(|e| format!("reading {name}: {e}"))?;
            let stored = anvil_formats::floor::StoredFloorHeight::parse(&resource.body)
                .map_err(|e| format!("{name}: {e}"))?;
            let entity = world.spawn();
            world.insert(entity, primase_core::Name(name.clone()));
            world.insert(entity, AnvilFloorHeight::new(stored, frame));
            notices.push(Notice::new(
                "anvil coarse floor",
                NoticeKind::Unsupported,
                "coarse height grid retained in Properties > Data; vertical sampling policy is caller-defined; normal \
                 map areas use the TNL terrain reader",
                1,
            ));
        }
        _ => {
            return Err(format!(
                "anvil: {name} is type {type_id:#010x}: previews support meshes, selectors, impostors, entities, \
                 entity groups, materials, texture maps, skies, ambiance, GI probes, look presets, coarse floors and \
                 grid metadata"
            ));
        }
    }
    if !placed.refused.is_empty() {
        let mut n = Notice::new(
            "anvil reader",
            NoticeKind::Unsupported,
            "objects the reader couldn't read",
            placed.refused.len() as u64,
        );
        for (id, why) in &placed.refused {
            n = n.example(format!("{id:#x}: {why}"));
        }
        notices.push(n);
    }
    // nothing shows: no Instance, light or marker (a marker is already a render status; source records hold
    // components, not geometry)
    if placed.placements.is_empty()
        && placed.lights.is_empty()
        && placed.markers.is_empty()
        && matches!(Kind::of(type_id), Some(Kind::Entity | Kind::EntityGroup))
        && world.iter::<AnvilWater>().next().is_none()
        && world.iter::<AnvilDeferredDecals>().next().is_none()
    {
        let entity = world.spawn();
        world.insert(entity, primase_core::Name(name.clone()));
        world.insert(
            entity,
            AnvilRenderStatus {
                source: format!("{name} ({id:#x})"),
                template: None,
                unsupported: true,
                reason: format!(
                    "source entity/group produced no static placement: {} visual(s) skipped, {} reader refusal(s). \
                     No origin or replacement geometry was invented",
                    placed.visuals_skipped,
                    placed.refused.len()
                ),
            },
        );
    }
    Ok(EntryRead {
        world,
        name,
        type_id,
        notices,
    })
}
