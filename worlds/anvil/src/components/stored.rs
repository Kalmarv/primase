//! Owned source records. Archive access and byte decoding belong to the readers, not these data types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reference { pub mode: u8, pub id: u64 }
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub path: Vec<u32>, pub kind: u32, pub times: Vec<u16>, pub values: Vec<[f32; 4]>,
    pub path_flags: [u8; 2], pub binding: u64, pub trailer: [u8; 5],
}
#[derive(Clone, Debug, PartialEq)]
pub struct Controller { pub tracks: Vec<Track> }
#[derive(Clone, Debug, PartialEq)]
pub struct StoredImpostor {
    pub head: [u8; 71], pub maps: [Option<Reference>; 4], pub table: [u32; 21],
    pub top: usize, pub side: usize, pub cards: Vec<[f32; 27]>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct StoredSpotLight {
    pub color: [f32; 4], pub intensity: f32, pub luminance: f32,
    pub inner: f32, pub outer: f32, pub range: f32, pub near: f32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct SkyKeyframe {
    pub hour: f32, pub crc: u32, pub inscatter: Vec<u8>, pub optical_depth: Vec<u8>,
    pub irradiance: Vec<u8>, pub sun_depth: Vec<u8>, pub sun_irradiance: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct StoredSky {
    pub sun_position_mode: u32, pub sun_azimuth: f32, pub sun_tilt: f32, pub sun_elevation: f32,
    pub sun_color: [f32; 4], pub sun_intensity: f32, pub mie: [f32; 3], pub keyframes: Vec<SkyKeyframe>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct StoredGiDyn {
    pub world_id: u64, pub cell_id: u32, pub position: [f32; 3], pub samples: u32, pub probes: u32,
    pub indoor: bool, pub voxel_bounds: [[i32; 3]; 2], pub subvolume_word: u32,
    pub streams: [Vec<u8>; 4], pub decoded_bytes: [u32; 3],
}
#[derive(Clone, Debug, PartialEq)]
pub enum StoredLook {
    Ambiance { season: u64, forecast: Option<Reference>, hour: f32, angle: f32, flag: u8 },
    Forecast { values: [f32; 12] },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridLayout { pub words: Vec<u16>, pub cell_size: i32, pub bottom_left: [i32; 3], pub dimension: i32, pub after: [u8; 3] }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridDescription { pub id: u64, pub layout: GridLayout, pub partition: Option<Reference> }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredGridPartition { pub levels: u32, pub cells: Vec<u64> }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredGridCellDataBlock { pub objects: Vec<Reference>, pub word: u32, pub key: u64 }
