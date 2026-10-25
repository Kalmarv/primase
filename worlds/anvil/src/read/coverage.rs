//! Archive coverage contracts, not a claim that every payload of a type parses or renders.
//! Embedded Entity components have a separate denominator; they are not forge entries.
use super::listing::Kind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status { Read, Dropped, Unread }
impl Status {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Dropped => "read-but-dropped-before-World",
            Self::Unread => "unread",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Coverage {
    pub type_id: u32,
    pub name: &'static str,
    pub status: Status,
    /// The retained representation, or the boundary that discards it.
    pub destination: &'static str,
    /// The next observation, including cases whose retained representation is partial.
    pub question: &'static str,
}

pub const EXTRA: &[Coverage] = &[
    Coverage { type_id: super::tnl::TNL_CHUNK, name: "TNLChunk", status: Status::Read,
        destination: "AnvilTerrain tiles through read::terrain", question: "Which stored chunks are reached by each selected area/level?" },
    Coverage { type_id: super::grass::GRASS_PATTERN_LIBRARY, name: "GrassPatternLibrary", status: Status::Read,
        destination: "AnvilGrass.library", question: "Which setups reference the library; does the caller run lower-anvil-grass?" },
    Coverage { type_id: super::light::SPOT_LIGHT, name: "SpotLight", status: Status::Read,
        destination: "AnvilSpotLight via LightInstance", question: "Which light owners are selected; what caller calibration is stated?" },
    Coverage { type_id: super::controller::TIME_OF_DAY, name: "TimeOfDayPropertyControllerData", status: Status::Read,
        destination: "AnvilSkyControllers.time_of_day (selected look)", question: "Which path-node profiles refuse; which look owns each controller?" },
    Coverage { type_id: super::controller::WEATHER, name: "WeatherPropertyControllerData", status: Status::Read,
        destination: "AnvilSkyControllers.weather (selected look)", question: "Which path-node profiles refuse; which look owns each controller?" },
    Coverage { type_id: super::build_table::BUILD_TABLE, name: "BuildTable", status: Status::Read,
        destination: "StoredBuildTable; character work excluded from this round", question: "Count only; no character implementation in this round" },
    Coverage { type_id: super::animation::ANIMATION, name: "Animation", status: Status::Read,
        destination: "AnvilClip; animation work excluded from this round", question: "Count only; no animation implementation in this round" },
    Coverage { type_id: 0x755A_CE14, name: "SplashFX", status: Status::Unread,
        destination: "Visual owner has AnvilRenderStatus only", question: "Group body sizes and state/action class sequences; establish exact extent of every polymorphic action before decoding" },
    Coverage { type_id: 0x824A_23BA, name: "FX", status: Status::Unread,
        destination: "FXComponent body retained, referenced FX graph not decoded", question: "Record command/node class sequences and lengths for referenced FX; a class name alone does not define the graph grammar" },
    Coverage { type_id: 0x8F3D_5BB6, name: "WorldParticlesDescriptor", status: Status::Unread,
        destination: "no World representation", question: "Extract one body per size/profile and its registry field sequence; no particle simulation in scope" },
    Coverage { type_id: 0x2919_1D7B, name: "TerrainSediments", status: Status::Unread,
        destination: "runtime terrain views unresolved", question: "Join referenced sediment records to t16/t20/t22 and establish record stride and texture ownership" },
    Coverage { type_id: 0x2F45_393E, name: "TerrainProceduralSet", status: Status::Unread,
        destination: "no World representation", question: "Join set references to TerrainMaterialBank and runtime terrain cache producers" },
    Coverage { type_id: 0x4323_218D, name: "TerrainProceduralMaterial", status: Status::Unread,
        destination: "no World representation", question: "Record per-profile parameter/texture references; identify virtual-detail cache producer" },
    Coverage { type_id: 0x65C3_C7D0, name: "TerrainProceduralSetBank", status: Status::Unread,
        destination: "no World representation", question: "Establish the counted reference vector and verify full-body closure" },
    Coverage { type_id: 0x11BF_3F3A, name: "RiverWorldComponentData", status: Status::Unread,
        destination: "river mesh/material is separate from this resource", question: "Establish resource-body grammar and join to placed RiverQuadTree visuals; do not mistake the component type for an entry" },
    Coverage { type_id: 0x5390_3478, name: "SpawnPointSettings", status: Status::Unread,
        destination: "settings are not a placed transform", question: "Find referencing Entity component and exact settings profile; determine position owner before creating a spawn marker" },
    Coverage { type_id: 0xAC8A_F394, name: "SpawnEntityBuilderDescriptor", status: Status::Unread,
        destination: "no World representation", question: "Establish builder/reference grammar and activation conditions; archive presence does not imply active props" },
    Coverage { type_id: 0xF748_5E9F, name: "Season", status: Status::Unread,
        destination: "look selection not resolved through event Season", question: "Validate 306/313-byte reference layouts and which event/state chooses each season" },
    Coverage { type_id: 0xF2BB_7836, name: "WeatherRegionCellData", status: Status::Unread,
        destination: "no World representation", question: "Establish cell-to-weather reference grammar, bounds and overlap policy" },
];

pub fn of(type_id: u32) -> Coverage {
    if let Some(row) = EXTRA.iter().find(|row| row.type_id == type_id) { return *row; }
    if let Some(kind) = Kind::of(type_id) {
        let (status, destination) = match kind {
            Kind::World => (Status::Dropped, "region reader consumes GridDescriptions; standalone catalog retains AnvilGridMetadata; remaining World fields unread"),
            Kind::GridPartition => (Status::Dropped, "region reader consumes cell selection; standalone catalog retains AnvilGridMetadata"),
            Kind::GridCellDataBlock => (Status::Read, "AnvilListed stores reached owner/cell/level; original full list not retained"),
            Kind::WorldAmbiance => (Status::Read, "owned AnvilSky and environment cube; flares/particles not decoded"),
            Kind::GiDynResource => (Status::Read, "AnvilGiDyn: one-volume/one-box profile, encoded streams and probe positions; irradiance unresolved"),
            Kind::AmbiancePreset | Kind::ForecastPreset => (Status::Read, "AnvilLookPreset: stored references/scalars; event/weather application unresolved"),
            Kind::MaterialTemplate => (Status::Read, "AnvilMaterial bindings and game_shader stages; not every template field"),
            Kind::CompiledMip => (Status::Read, "core.Texture/Core.TextureLayers mip payload through texture loader"),
            Kind::FloorHeightData => (Status::Read, "AnvilFloorHeight through StoredFloorHeight::component; area source uses TNL instead"),
            Kind::CollisionMaterial => (Status::Read, "CollisionMaterialRef identity/name; full physical material parameters not decoded"),
            _ => (Status::Read, "typed Anvil/core components; see docs/anvil-content-preview.md resource table"),
        };
        return Coverage { type_id, name: kind.name(), status, destination,
            question: "Inventory capability only: run the type's parser/area census to measure refusals and actual reachability" };
    }
    Coverage { type_id, name: "unknown or unclassified resource", status: Status::Unread,
        destination: "no audited World path", question: "Identify class in the runtime registry; obtain body-size/profile and reference evidence before writing a reader" }
}
