//! The `trackmania.*` dialect: what a map and the game's block models state, as stored, beside the core data the reader
//! makes of them. Positions are in the core frame in metres ([Frame](crate#frame)).
use primase_core::{Component, Entity, JsonError, Resource};
use serde::Serialize;

fn to_json<T: Serialize>(v: &T) -> Result<Option<serde_json::Value>, JsonError> {
    primase_core::json::to_value(v).map(Some)
}

/// The map (`trackmania.map`, a resource): its identity and the decoration it is built in.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TrackmaniaMap {
    /// The map's name as stored (with its `$` formatting codes).
    pub name: String,
    /// The map's uid.
    pub uid: String,
    /// The author's login.
    pub author: String,
    /// The decoration's id (`Sunrise`, `Day`, `Sunset`, `Night`).
    pub decoration: String,
    /// The collection (environment) the map and its decoration belong to (`Stadium`).
    pub collection: String,
    /// The map's size in blocks.
    pub size: [u32; 3],
    /// The collection's square: its size along X and Z and its height, in metres (Stadium: 32 and 8).
    pub square: [f32; 2],
    /// The block data's version (chunk `0x0304301F`; 0 for the older `0x03043013`).
    pub block_data_version: u32,
}

impl Resource for TrackmaniaMap {
    const NAME: &'static str = "trackmania.map";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
}

/// A block the map places (`trackmania.block`), on its placement entity: the map's record and the block model it
/// names. A block of the collection's default zone the reader places where the map has no ground block is one too,
/// with [`TrackmaniaBlock::fill`] set: the map does not store it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TrackmaniaBlock {
    /// The block model's name (`StadiumRoadMain`).
    pub name: String,
    /// Its coordinate in block units, as meant: block data version 6 and later store it plus (1, 0, 1).
    pub coord: [i32; 3],
    /// Its direction: 0 north, 1 east, 2 south, 3 west.
    pub direction: u8,
    /// Its flags as stored.
    pub flags: u32,
    /// Flag bit 12: the block stands on the ground and takes its model's ground variant (else its air variant).
    pub ground: bool,
    /// Flags bits 0 to 5: the variant, the index of its mobil list.
    pub variant: u8,
    /// Flags bits 6 to 11: the sub-variant, the mobil in that list.
    pub sub_variant: u8,
    /// The block model's file (`Stadium\ConstructionBlockInfo\...`), `None` when the game has none of that name.
    pub info: Option<String>,
    /// What way point the block model is (`EWayPointType`: 0 start, 1 finish, 2 checkpoint, 3 none, 4 start and
    /// finish), when it states one.
    pub waypoint: Option<i32>,
    /// Placed by the reader for the default zone (the map stores no such block; its flags are the ground flag only).
    pub fill: bool,
}

impl Component for TrackmaniaBlock {
    const NAME: &'static str = "trackmania.block";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
}

/// Where a car starts or respawns on a block (`trackmania.spawn`): the block model's spawn location for the variant
/// placed (ground or air), on an entity of its own with that location as its `core.transform`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct TrackmaniaSpawn {
    /// The block's placement entity.
    pub block: Entity,
    /// The block model's way point type ([`TrackmaniaBlock::waypoint`]), `None` when it states none.
    pub waypoint: Option<i32>,
}

impl Component for TrackmaniaSpawn {
    const NAME: &'static str = "trackmania.spawn";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.block);
    }
}

/// One texture slot of a material, as its `CPlugMaterialCustom` names it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TrackmaniaTextureSlot {
    /// The slot's name (`Diffuse`, `Normal`, `Blend1`, `Lighting`, ...): the shader's sampler.
    pub slot: String,
    /// The texture file (`CPlugBitmap`, `...\X.Texture.gbx`).
    pub bitmap: Option<String>,
    /// The image it shows (`...\Image\X.dds`).
    pub image: Option<String>,
    /// The texture entity (`core.texture`), `None` when the image is not read (named in the reader's report).
    pub texture: Option<Entity>,
    /// The bitmap's texture coordinate transform: scale, translation and rotation (degrees: 180, -30 and -120 occur)
    /// as stored.
    pub scale: [f32; 2],
    pub translation: [f32; 2],
    pub rotation: f32,
}

/// A material (`trackmania.material`): its shader, its texture slots and its physics surface, as stored.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TrackmaniaMaterial {
    /// The material's file (`Stadium\Media\Material\StadiumRoad.Material.Gbx`).
    pub path: String,
    /// The shader it draws with: the template material's file (`Techno2\Media\Material\TDiff_Spec_Nrm TOcc
    /// CSpecSoft.Material.Gbx`), `None` when it draws with inline device materials.
    pub shader: Option<String>,
    /// The texture slots, in the order stored.
    pub slots: Vec<TrackmaniaTextureSlot>,
    /// The physics surface id (`CPlugSurface.MaterialId`).
    pub surface_id: Option<i16>,
}

impl TrackmaniaMaterial {
    /// The slot named `name`.
    pub fn slot(&self, name: &str) -> Option<&TrackmaniaTextureSlot> {
        self.slots.iter().find(|s| s.slot == name)
    }

    /// The shader's file name without its folder and `.Material.Gbx` (`TDiff_Spec_Nrm TOcc CSpecSoft`).
    pub fn shader_name(&self) -> Option<&str> {
        let shader = self.shader.as_deref()?;
        let file = shader.rsplit(['\\', '/']).next().unwrap_or(shader);
        Some(file.split('.').next().unwrap_or(file))
    }
}

impl Component for TrackmaniaMaterial {
    const NAME: &'static str = "trackmania.material";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.slots.iter().filter_map(|s| s.texture));
    }
}

/// What a prototype is (`trackmania.solid`): the solid (`CPlugSolid`) it was read from.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TrackmaniaSolid {
    /// The solid's file.
    pub path: String,
    /// The node of that file, 0 the file's main node.
    pub node: u32,
    /// The mip trees' far distances (`CPlugTreeVisualMip`), per tree in the order met: level `k` of a tree draws
    /// up to its `k`-th distance. The prototype's own mesh takes every tree's first level, its `core.lods` form `k`
    /// each tree's level `k` (or its last).
    pub mip_far: Vec<Vec<f32>>,
}

impl Component for TrackmaniaSolid {
    const NAME: &'static str = "trackmania.solid";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
}
