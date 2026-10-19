//! The pause-map picture's resources: the scene's own picture as loaded, and the picture a map writer writes, as
//! `prepare-reskate-pause-map` chose it.

use primase_core::Resource;

/// The scene's own pause-map picture, decoded (RGBA8, row 0 north), as the reader's loader puts it in the world, with
/// the file it was read from when it was ([`crate::components::loaded`]: its bytes, written back while the picture is
/// unchanged).
#[derive(Clone, Debug, PartialEq)]
pub struct SkatePauseImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub file: Option<crate::components::LoadedFile>,
}

impl Resource for SkatePauseImage {
    const NAME: &'static str = "skate.pause_image";
}

/// The pause-map picture a map writer writes ([`crate::passes::PrepareReskatePauseMap`]).
#[derive(Clone, Debug, PartialEq)]
pub enum SkatePausePicture {
    /// This picture (RGBA8, row 0 north) covering ground `bounds` `[east0, north0, east1, north1]`.
    Image {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        bounds: [f32; 4],
    },
    /// The map's geometry drawn top-down, `height` pixels high, over ground `extent` (`None`: the map's own content
    /// bounds as the writer measures them).
    Geometry { height: usize, extent: Option<[f32; 4]> },
}

impl Resource for SkatePausePicture {
    const NAME: &'static str = "skate.pause_picture";
}
