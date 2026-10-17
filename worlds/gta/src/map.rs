//! Map data (ymap: `CMapData`, placed entities) and map types (ytyp: `CMapTypes`, archetypes), as stored.
//!
//! Read from a decoded META root ([`crate::meta::Meta::root`]) by field name; a missing field or one of another
//! kind is an error. Values are kept as the files store them (flags, enums and hashes raw); what they mean is the
//! passes' business. Parts the first milestone does not use (occluders, grass, lights, car generators, interiors'
//! rooms and portals, extensions) stay in the META tree.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::meta::Struct;
use crate::{Result, invalid};

fn name_is(s: &Struct, names: &[&str]) -> Result<usize> {
    names
        .iter()
        .position(|n| primase_hash::joaat_exact(n.as_bytes()) == s.name)
        .ok_or_else(|| {
            invalid(
                "META",
                0,
                format!("structure {:#010x} where one of {names:?} belongs", s.name),
            )
        })
}

/// What an entity definition is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityKind {
    /// `CEntityDef`.
    Entity,
    /// `CMloInstanceDef`: a placed interior.
    Interior,
}

/// A placed entity (`CEntityDef`, or the entity part of a `CMloInstanceDef`).
#[derive(Clone, Debug, PartialEq)]
pub struct EntityDef {
    pub kind: EntityKind,
    pub archetype: u32,
    pub flags: u32,
    pub guid: u32,
    pub position: [f32; 3],
    /// A quaternion `x, y, z, w` as stored (the game uses its conjugate for the entity's orientation).
    pub rotation: [f32; 4],
    pub scale_xy: f32,
    pub scale_z: f32,
    /// The index of the LOD parent in the parent map's entities, -1 for none.
    pub parent_index: i32,
    pub lod_dist: f32,
    pub child_lod_dist: f32,
    /// `rage__eLodType`: 0 HD, 1 LOD, 2 SLOD1, 3 SLOD2, 4 SLOD3, 5 ORPHANHD, 6 SLOD4.
    pub lod_level: i32,
    pub num_children: u32,
    /// `rage__ePriorityLevel`: 0 required, 1 optional high, 2 optional medium, 3 optional low.
    pub priority_level: i32,
    pub ambient_occlusion_multiplier: i32,
    pub artificial_ambient_occlusion: i32,
    pub tint_value: u32,
    /// The number of extensions (lights, particle effects, ladders, ...), kept in the META tree.
    pub extensions: usize,
}

impl EntityDef {
    pub fn from_struct(s: &Struct) -> Result<Self> {
        let kind = match name_is(s, &["CEntityDef", "CMloInstanceDef"])? {
            0 => EntityKind::Entity,
            _ => EntityKind::Interior,
        };
        Ok(EntityDef {
            kind,
            archetype: s.hash("archetypeName")?,
            flags: s.u32("flags")?,
            guid: s.u32("guid")?,
            position: s.vec3("position")?,
            rotation: s.vec4("rotation")?,
            scale_xy: s.f32("scaleXY")?,
            scale_z: s.f32("scaleZ")?,
            parent_index: s.i32("parentIndex")?,
            lod_dist: s.f32("lodDist")?,
            child_lod_dist: s.f32("childLodDist")?,
            lod_level: s.enum_value("lodLevel")?,
            num_children: s.u32("numChildren")?,
            priority_level: s.enum_value("priorityLevel")?,
            ambient_occlusion_multiplier: s.i32("ambientOcclusionMultiplier")?,
            artificial_ambient_occlusion: s.i32("artificialAmbientOcclusion")?,
            tint_value: s.u32("tintValue")?,
            extensions: s.array("extensions")?.len(),
        })
    }
}

/// A map (`CMapData`).
#[derive(Clone, Debug, PartialEq)]
pub struct MapData {
    pub name: u32,
    /// The LOD parent map's name, 0 for none.
    pub parent: u32,
    pub flags: u32,
    pub content_flags: u32,
    pub streaming_extents: [[f32; 3]; 2],
    pub entities_extents: [[f32; 3]; 2],
    pub entities: Vec<EntityDef>,
    pub physics_dictionaries: Vec<u32>,
}

impl MapData {
    pub fn from_root(s: &Struct) -> Result<Self> {
        name_is(s, &["CMapData"])?;
        Ok(MapData {
            name: s.hash("name")?,
            parent: s.hash("parent")?,
            flags: s.u32("flags")?,
            content_flags: s.u32("contentFlags")?,
            streaming_extents: [s.vec3("streamingExtentsMin")?, s.vec3("streamingExtentsMax")?],
            entities_extents: [s.vec3("entitiesExtentsMin")?, s.vec3("entitiesExtentsMax")?],
            entities: s
                .pointed("entities")?
                .into_iter()
                .map(EntityDef::from_struct)
                .collect::<Result<_>>()?,
            physics_dictionaries: s.hashes("physicsDictionaries")?,
        })
    }
}

/// What an archetype definition is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchetypeKind {
    /// `CBaseArchetypeDef`.
    Base,
    /// `CTimeArchetypeDef`: shown only in some hours (`time_flags`).
    Time,
    /// `CMloArchetypeDef`: an interior (rooms, portals and entity sets stay in the META tree).
    Interior,
}

/// An archetype (`CBaseArchetypeDef` and the base part of the time and interior kinds).
#[derive(Clone, Debug, PartialEq)]
pub struct ArchetypeDef {
    pub kind: ArchetypeKind,
    pub name: u32,
    pub lod_dist: f32,
    pub flags: u32,
    pub special_attribute: u32,
    pub bb_min: [f32; 3],
    pub bb_max: [f32; 3],
    pub bs_centre: [f32; 3],
    pub bs_radius: f32,
    pub hd_texture_dist: f32,
    pub texture_dictionary: u32,
    pub clip_dictionary: u32,
    pub drawable_dictionary: u32,
    pub physics_dictionary: u32,
    /// `rage__fwArchetypeDef__eAssetType`: 0 uninitialized, 1 fragment, 2 drawable, 3 drawable dictionary, 4 asset-less.
    pub asset_type: i32,
    pub asset_name: u32,
    /// The hours shown, for the time kind.
    pub time_flags: Option<u32>,
    /// The number of extensions, kept in the META tree.
    pub extensions: usize,
    /// For the interior kind: its flags, the entities placed in it (in the interior's space; the entity sets, which
    /// scripts switch, stay in the META tree) and how many entity sets it has.
    pub interior: Option<Interior>,
}

/// An interior archetype's own entities.
#[derive(Clone, Debug, PartialEq)]
pub struct Interior {
    pub mlo_flags: u32,
    pub entities: Vec<EntityDef>,
    pub entity_sets: usize,
}

impl ArchetypeDef {
    pub fn from_struct(s: &Struct) -> Result<Self> {
        let kind = match name_is(s, &["CBaseArchetypeDef", "CTimeArchetypeDef", "CMloArchetypeDef"])? {
            0 => ArchetypeKind::Base,
            1 => ArchetypeKind::Time,
            _ => ArchetypeKind::Interior,
        };
        Ok(ArchetypeDef {
            kind,
            name: s.hash("name")?,
            lod_dist: s.f32("lodDist")?,
            flags: s.u32("flags")?,
            special_attribute: s.u32("specialAttribute")?,
            bb_min: s.vec3("bbMin")?,
            bb_max: s.vec3("bbMax")?,
            bs_centre: s.vec3("bsCentre")?,
            bs_radius: s.f32("bsRadius")?,
            hd_texture_dist: s.f32("hdTextureDist")?,
            texture_dictionary: s.hash("textureDictionary")?,
            clip_dictionary: s.hash("clipDictionary")?,
            drawable_dictionary: s.hash("drawableDictionary")?,
            physics_dictionary: s.hash("physicsDictionary")?,
            asset_type: s.enum_value("assetType")?,
            asset_name: s.hash("assetName")?,
            time_flags: match kind {
                ArchetypeKind::Time => Some(s.u32("timeFlags")?),
                _ => None,
            },
            extensions: s.array("extensions")?.len(),
            interior: match kind {
                ArchetypeKind::Interior => Some(Interior {
                    mlo_flags: s.u32("mloFlags")?,
                    entities: s
                        .pointed("entities")?
                        .into_iter()
                        .map(EntityDef::from_struct)
                        .collect::<Result<_>>()?,
                    entity_sets: s.array("entitySets")?.len(),
                }),
                _ => None,
            },
        })
    }
}

/// Map types (`CMapTypes`).
#[derive(Clone, Debug, PartialEq)]
pub struct MapTypes {
    pub name: u32,
    pub archetypes: Vec<ArchetypeDef>,
    pub dependencies: Vec<u32>,
}

impl MapTypes {
    pub fn from_root(s: &Struct) -> Result<Self> {
        name_is(s, &["CMapTypes"])?;
        Ok(MapTypes {
            name: s.hash("name")?,
            archetypes: s
                .pointed("archetypes")?
                .into_iter()
                .map(ArchetypeDef::from_struct)
                .collect::<Result<_>>()?,
            dependencies: s.hashes("dependencies")?,
        })
    }
}
