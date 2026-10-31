//! The `trackmania.*` dialect: what a map and the game's block models state, as stored, beside the core data the reader
//! makes of them. Positions are in the core frame in metres ([Frame](crate#frame)).
use primase_core::{Component, Entity, JsonError, Resource, Transform};
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

/// The mood of the map's decoration (`trackmania.mood`, a resource): its `CGameCtnDecorationMood` as stored, with the
/// light map's mood and the sun light its remap folder holds. The game places its sun at run time from the latitude, the
/// sun's rise and fall and the day time; how is not established, so no core light is made of it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TrackmaniaMood {
    /// The decoration's file (`Stadium\ConstructionDecoration\StadiumBase32x32Day.TMDecoration.Gbx`).
    pub decoration: String,
    /// The mood's file, `None` when the decoration stores its mood inside itself.
    pub file: Option<String>,
    /// Chunk `0x0303A000`.
    pub sun: Option<TrackmaniaSun>,
    /// Chunk `0x0303A001`: the day time the remapping starts at (GBX.NET's `RemappedStartDayTime`; Stadium's Day
    /// 0.65, Sunrise 0.53, Sunset 0.75, Night 0.15), and the folder whose files the remapping substitutes.
    pub remapped_start_day_time: Option<f32>,
    pub remap_folder: Option<String>,
    /// Chunk `0x0303A002`.
    pub shadows: Option<TrackmaniaMoodShadows>,
    /// Chunk `0x0303A006` (no Stadium mood stores it).
    pub sun_moon_intensity: Option<f32>,
    /// The light map's mood (`CHmsLightMapMood` of chunk `0x0303A004`'s `CHmsLightMap`).
    pub light_map: Option<TrackmaniaLightMapMood>,
    /// The directional light of the remap folder's `Directional.SceneLight.Gbx`, when the folder holds one (Stadium's
    /// Sunrise and Sunset do, Day and Night do not).
    pub sun_light: Option<TrackmaniaSunLight>,
}

impl Resource for TrackmaniaMood {
    const NAME: &'static str = "trackmania.mood";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
}

/// A mood's sun: the latitude and longitude in degrees, the offset from GMT, and the times of sunrise and sunfall in
/// milliseconds since midnight.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TrackmaniaSun {
    pub latitude: f32,
    pub longitude: f32,
    pub delta_gmt: f32,
    pub sun_rise_ms: i32,
    pub sun_fall_ms: i32,
}

/// A mood's shadows: how many the player's and the opponents' cars cast, the cars' shadow intensity, whether the
/// scene casts shadows and whether the background is lit locally.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TrackmaniaMoodShadows {
    pub count_car_human: i32,
    pub count_car_opponent: i32,
    pub car_intensity: f32,
    pub scene: bool,
    pub background_is_locally_lighted: bool,
}

/// A light map's mood as stored: its first pair (unnamed), its ambient range and its directional scale.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TrackmaniaLightMapMood {
    pub file: Option<String>,
    pub u01: Option<[f32; 2]>,
    pub ambient_range: Option<[f32; 2]>,
    pub directional_scale: Option<f32>,
}

/// A mood's sun light (`CGxLightDirectional` of a `CSceneLight`), as stored: it states no direction.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TrackmaniaSunLight {
    pub file: String,
    pub common: Option<TrackmaniaLightProperties>,
    pub double_sided_rgb: Option<[f32; 3]>,
    pub reverse_rgb: Option<[f32; 3]>,
    pub reverse_intensity: Option<f32>,
    pub emitting_angular_size: Option<f32>,
    pub flare_angular_size: Option<f32>,
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

/// A prototype's lights (`trackmania.lights`): the light trees (`CPlugTreeLight`) of its solid, each its `CPlugLight`'s
/// Gx light as stored, at the tree's location in the prototype's space. A lowering places them per instance.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TrackmaniaLights(pub Vec<TrackmaniaLight>);

impl Component for TrackmaniaLights {
    const NAME: &'static str = "trackmania.lights";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// Each light's location in the prototype's space: mapped with it.
    fn apply_linear(&self, l: &primase_core::Linear) -> primase_core::Linearity<Self> {
        let lights = self
            .0
            .iter()
            .map(|light| TrackmaniaLight {
                location: Transform {
                    cols: light.location.cols.map(|c| l.point(c)),
                },
                ..light.clone()
            })
            .collect();
        primase_core::Linearity::Applied(TrackmaniaLights(lights))
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
}

/// One light of a solid (see [`TrackmaniaLights`]), its Gx light's chunks as stored (`None`: the light has no such
/// chunk). Colours are as the game stores them (0 to 1); angles in degrees.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TrackmaniaLight {
    /// The light tree's name, when it has one.
    pub tree: Option<String>,
    /// The light tree's location in the prototype's space, composed from the solid's root: its position, and its axes
    /// (the light shines along its +Z; see `lower-trackmania-lights`).
    pub location: Transform,
    /// The Gx light's class: `ball` (`CGxLightBall`), `spot`, `frustum` or `directional`.
    pub kind: TrackmaniaLightKind,
    /// `CGxLight`'s properties (chunk `0x04001009`).
    pub common: Option<TrackmaniaLightProperties>,
    /// `CGxLightBall`'s properties (a ball, spot or frustum light; chunk `0x04002006`).
    pub ball: Option<TrackmaniaBallProperties>,
    /// A spot's cone (chunk `0x0400B002`, else the older `0x0400B001`).
    pub spot: Option<TrackmaniaSpotProperties>,
}

/// The Gx light class of a [`TrackmaniaLight`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackmaniaLightKind {
    Ball,
    Spot,
    Frustum,
    Directional,
}

/// `CGxLight`'s stored properties.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TrackmaniaLightProperties {
    pub color: [f32; 3],
    pub flags: u32,
    pub intensity: f32,
    pub diffuse_intensity: f32,
    pub specular_intensity: f32,
    pub specular_power: f32,
    pub shadow_intensity: f32,
    pub flare_intensity: f32,
    pub shadow_rgb: [f32; 3],
}

impl From<&gbx_formats::plug::light::LightProperties> for TrackmaniaLightProperties {
    fn from(c: &gbx_formats::plug::light::LightProperties) -> Self {
        TrackmaniaLightProperties {
            color: c.color,
            flags: c.flags,
            intensity: c.intensity,
            diffuse_intensity: c.diffuse_intensity,
            specular_intensity: c.specular_intensity,
            specular_power: c.specular_power,
            shadow_intensity: c.shadow_intensity,
            flare_intensity: c.flare_intensity,
            shadow_rgb: c.shadow_rgb,
        }
    }
}

/// `CGxLightBall`'s stored properties.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TrackmaniaBallProperties {
    pub flags: u32,
    pub radius: f32,
    pub radius_specular: f32,
    pub radius_shadow: f32,
    pub radius_flare: f32,
    pub emitting_radius: f32,
    pub attenuation1: f32,
    pub attenuation2: f32,
    pub ambient_rgb: [f32; 3],
}

/// A spot's stored cone: its inner and outer full apex angles in degrees, and its falloff exponent.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TrackmaniaSpotProperties {
    pub inner: f32,
    pub outer: f32,
    pub falloff_exponent: f32,
}
