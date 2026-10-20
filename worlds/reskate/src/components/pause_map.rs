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
    /// bounds as the writer measures them), as `drawing` says.
    Geometry {
        height: usize,
        extent: Option<[f32; 4]>,
        drawing: PauseDrawing,
    },
}

/// Which triangles a pause-map picture drawn from the geometry leaves out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PauseLeaveOut {
    /// Those whose material casts no shadow (`skate.material` `cast_shadows` false: a room's shell, its ceiling
    /// included, selected by `set-reskate-shadow-casting`), counted.
    Shadowless,
    /// Those of the placements whose name starts with one of these prefixes (a room's shell named by its path, while
    /// other structures that cast no shadow stay in the picture), counted.
    Placements(std::collections::BTreeSet<String>),
    /// None.
    Nothing,
}

/// The colour a pause-map picture drawn from the geometry gives a triangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseColour {
    /// Its material's base texture's mean colour.
    Mean,
    /// Its material's base texture sampled at each pixel (module documentation of `prepare-reskate-pause-map`).
    Texture,
}

/// How a pause-map picture drawn from the geometry draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PauseDrawing {
    pub leave_out: PauseLeaveOut,
    pub colour: PauseColour,
}

impl Resource for SkatePausePicture {
    const NAME: &'static str = "skate.pause_picture";
}
