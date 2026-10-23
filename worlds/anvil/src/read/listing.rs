//! What a forge or an archive holds, by kind and name: the entries of the forge tables, each with the kind its type id
//! names where a reader here reads that type. Nothing is decoded: the tables alone say it, so a listing of a 15 GB
//! forge takes the time of its index.
//!
//! A [`Kind`] is a name for a type id a reader of this crate reads (the type ids are CRC-32s of the game's class
//! names; the names here are those class names). A type no reader reads has no kind: its entry is listed with its
//! type id as stored, so a consumer lists what is there without a comparison of its own.

use crate::read::archive::Archive;
use crate::read::ddecal::DDECAL_MODEL;
use crate::read::entity::{ENTITY, ENTITY_GROUP};
use crate::read::floor::FLOOR_HEIGHT_DATA;
use crate::read::forge::Forge;
use crate::read::grid::{GRID_CELL_DATA_BLOCK, GRID_PARTITION, WORLD};
use crate::read::lod::LOD_SELECTOR;
use crate::read::material::{MATERIAL_TEMPLATE, TEXTURE_MAP, TEXTURE_SET};
use crate::read::mesh::{MATERIAL, MESH};
use crate::read::mesh_shape::MESH_SHAPE;
use crate::read::mip::COMPILED_MIP;
use crate::read::shape::{BOX_SHAPE, CAPSULE_SHAPE, REFERENCE_LIST_SHAPE, SPHERE_SHAPE};
use crate::read::skeleton::SKELETON;
use crate::read::terrain_material::{COLLISION_MATERIAL, TERRAIN_MATERIAL, TERRAIN_MATERIAL_BANK};
use crate::read::tnl::TERRAIN_NODE_LAYER;

/// A kind of entry some reader of this crate reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Mesh,
    Material,
    MaterialTemplate,
    TextureMap,
    TextureSet,
    CompiledMip,
    Entity,
    EntityGroup,
    LodSelector,
    MeshShape,
    SphereShape,
    CapsuleShape,
    BoxShape,
    ReferenceListShape,
    World,
    GridPartition,
    GridCellDataBlock,
    TerrainNodeLayer,
    TerrainMaterialBank,
    TerrainMaterial,
    CollisionMaterial,
    FloorHeightData,
    DecalModel,
    Skeleton,
}

/// Every kind, in declaration order.
pub const KINDS: [Kind; 24] = [
    Kind::Mesh,
    Kind::Material,
    Kind::MaterialTemplate,
    Kind::TextureMap,
    Kind::TextureSet,
    Kind::CompiledMip,
    Kind::Entity,
    Kind::EntityGroup,
    Kind::LodSelector,
    Kind::MeshShape,
    Kind::SphereShape,
    Kind::CapsuleShape,
    Kind::BoxShape,
    Kind::ReferenceListShape,
    Kind::World,
    Kind::GridPartition,
    Kind::GridCellDataBlock,
    Kind::TerrainNodeLayer,
    Kind::TerrainMaterialBank,
    Kind::TerrainMaterial,
    Kind::CollisionMaterial,
    Kind::FloorHeightData,
    Kind::DecalModel,
    Kind::Skeleton,
];

impl Kind {
    /// The kind of the type id `type_id`, where a reader here reads it.
    pub fn of(type_id: u32) -> Option<Kind> {
        KINDS.into_iter().find(|k| k.type_id() == type_id)
    }

    /// The type id the kind's entries carry in the name table.
    pub const fn type_id(self) -> u32 {
        match self {
            Kind::Mesh => MESH,
            Kind::Material => MATERIAL,
            Kind::MaterialTemplate => MATERIAL_TEMPLATE,
            Kind::TextureMap => TEXTURE_MAP,
            Kind::TextureSet => TEXTURE_SET,
            Kind::CompiledMip => COMPILED_MIP,
            Kind::Entity => ENTITY,
            Kind::EntityGroup => ENTITY_GROUP,
            Kind::LodSelector => LOD_SELECTOR,
            Kind::MeshShape => MESH_SHAPE,
            Kind::SphereShape => SPHERE_SHAPE,
            Kind::CapsuleShape => CAPSULE_SHAPE,
            Kind::BoxShape => BOX_SHAPE,
            Kind::ReferenceListShape => REFERENCE_LIST_SHAPE,
            Kind::World => WORLD,
            Kind::GridPartition => GRID_PARTITION,
            Kind::GridCellDataBlock => GRID_CELL_DATA_BLOCK,
            Kind::TerrainNodeLayer => TERRAIN_NODE_LAYER,
            Kind::TerrainMaterialBank => TERRAIN_MATERIAL_BANK,
            Kind::TerrainMaterial => TERRAIN_MATERIAL,
            Kind::CollisionMaterial => COLLISION_MATERIAL,
            Kind::FloorHeightData => FLOOR_HEIGHT_DATA,
            Kind::DecalModel => DDECAL_MODEL,
            Kind::Skeleton => SKELETON,
        }
    }

    /// The game's class name.
    pub const fn name(self) -> &'static str {
        match self {
            Kind::Mesh => "Mesh",
            Kind::Material => "Material",
            Kind::MaterialTemplate => "MaterialTemplate",
            Kind::TextureMap => "TextureMap",
            Kind::TextureSet => "TextureSet",
            Kind::CompiledMip => "CompiledMip",
            Kind::Entity => "Entity",
            Kind::EntityGroup => "EntityGroup",
            Kind::LodSelector => "LODSelector",
            Kind::MeshShape => "MeshShape",
            Kind::SphereShape => "SphereShape",
            Kind::CapsuleShape => "CapsuleShape",
            Kind::BoxShape => "BoxShape",
            Kind::ReferenceListShape => "ReferenceListShape",
            Kind::World => "World",
            Kind::GridPartition => "GridPartition",
            Kind::GridCellDataBlock => "GridCellDataBlock",
            Kind::TerrainNodeLayer => "TerrainNodeLayer",
            Kind::TerrainMaterialBank => "TerrainMaterialBank",
            Kind::TerrainMaterial => "TerrainMaterial",
            Kind::CollisionMaterial => "CollisionMaterial",
            Kind::FloorHeightData => "FloorHeightData",
            Kind::DecalModel => "DDecalModel",
            Kind::Skeleton => "Skeleton",
        }
    }
}

/// One entry of a forge's tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Listed<'a> {
    /// Its position in the forge's tables (the `entry` of an [`At`](crate::read::archive::At)).
    pub entry: usize,
    /// Its ClassID: the index id other resources refer to it by.
    pub id: u64,
    /// Its name as the name table stores it.
    pub name: &'a str,
    /// The type id as stored, whether or not a reader reads it.
    pub type_id: u32,
    /// Its kind, where a reader here reads its type.
    pub kind: Option<Kind>,
    /// Its payload's size in the file, bytes.
    pub size: i32,
}

impl Forge {
    /// Every entry, in table order, with its kind where a reader reads its type: from the tables alone, nothing
    /// decoded.
    pub fn listing(&self) -> impl Iterator<Item = Listed<'_>> {
        self.entries().enumerate().map(|(entry, (index, name))| Listed {
            entry,
            id: index.id,
            name: name.name.as_str(),
            type_id: name.type_id,
            kind: Kind::of(name.type_id),
            size: index.size,
        })
    }
}

impl Archive {
    /// Every entry of every forge, forge by forge in the order given: the forge's position and the entry. What an id
    /// names is the first forge holding it ([`Archive::find`]); a listing holds an id once per forge that has it.
    pub fn listing(&self) -> impl Iterator<Item = (usize, Listed<'_>)> {
        (0..)
            .map_while(|forge| self.forge(forge).map(|f| (forge, f)))
            .flat_map(|(forge, f)| f.listing().map(move |l| (forge, l)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each kind's type id names it back, and no two kinds share an id.
    #[test]
    fn kinds_and_type_ids_correspond() {
        for k in KINDS {
            assert_eq!(Kind::of(k.type_id()), Some(k), "{k:?}");
        }
        let mut ids: Vec<u32> = KINDS.iter().map(|k| k.type_id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), KINDS.len());
        assert_eq!(Kind::of(0), None);
    }
}
