//! Readers for the data of Tony Hawk's SHRED (PS3), the RME engine's disc format.
//!
//! Bytes in, plain records out. Nothing here loads a game, builds a scene or chooses what a record means beyond what the
//! files state; fields whose role is unproven are kept as stored.
//!
//! - Containers: [`pak`] reads PAK v4 archives (big endian, named entries in sections, stored or [`cstr`] blocks), and
//!   [`cstr`] the CSTR framing of one compressed entry.
//! - Level geometry: a level's meshes are quartets of `.scp`, `.mlp`, `.mmp` and `.mgp` files. [`scene::decode`] reads
//!   a quartet into models (positions, [`attributes`], submeshes, processed [`skin`] records) and placements;
//!   [`scp_tables`] reads the SCP's node and world-matrix tables and its blob sections of spot chunks, [`scp_splines`],
//!   [`scp_cameras`], [`scp_lights`], [`scp_locators`] and [`scp_occluders`].
//! - Materials: [`material`] reads the MBP/MDP material banks, [`pixel_program`] classifies a compiled fragment program
//!   with [`rsx_reader`], [`texture`] indexes the packaged texture headers and pixel payloads, and [`library`] joins
//!   banks, programs and textures.
//! - Tables: [`database`] reads `MAIN.DB` (SQLite 3) whole through `quantydb-sqlite`; [`session`], [`surfaces`],
//!   [`manifest`] and [`names`] read the level, session, surface, drop-spot and rail rows of it.
//! - Scripts: [`level_params`] lists the constant-argument calls of a level's Lua 5.1 parameter script.
//! - Animation: [`reference_rig`] reads the Morpheme reference rigs of an animation container.
//!
//! Matrices are [`Affine`] values in the source frame (left-handed, Y up); converting them is the caller's.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod attributes;
pub mod cstr;
pub mod database;
pub mod level_params;
pub mod library;
pub mod manifest;
pub mod material;
pub mod names;
pub mod pak;
pub mod pixel_program;
pub mod reference_rig;
pub mod scene;
pub mod scp_cameras;
pub mod scp_lights;
pub mod scp_locators;
pub mod scp_occluders;
pub mod scp_splines;
pub mod scp_tables;
pub mod session;
pub mod skin;
pub mod surfaces;
pub mod texture;

/// Why a read was refused.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Field(#[from] primase_foundations::bytes::FieldError),
    /// The data is not what the format allows, at `offset` in the file or decoded entry being read.
    #[error("at {offset:#x}: {detail}")]
    Invalid { offset: usize, detail: String },
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn invalid(offset: usize, detail: impl ToString) -> Error {
    Error::Invalid {
        offset,
        detail: detail.to_string(),
    }
}

/// An affine 3x4 transform, column by column: the images of local +X, +Y, +Z, then the translation;
/// `world = M * local`. Entries are the stored f32 values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub cols: [[f32; 3]; 4],
}

impl Affine {
    pub const IDENTITY: Affine = Affine {
        cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0., 0., 0.]],
    };

    /// `self` after `inner` (`world = self (inner local)`), computed in f64 from the stored values with each entry
    /// rounded once to f32.
    pub fn compose(&self, inner: &Affine) -> Affine {
        let o = self.cols.map(|c| c.map(f64::from));
        let linear = |v: [f32; 3]| -> [f64; 3] {
            let v = v.map(f64::from);
            std::array::from_fn(|r| o[0][r] * v[0] + o[1][r] * v[1] + o[2][r] * v[2])
        };
        let [x, y, z, t] = inner.cols.map(linear);
        Affine {
            cols: [
                x.map(|v| v as f32),
                y.map(|v| v as f32),
                z.map(|v| v as f32),
                std::array::from_fn(|r| (t[r] + o[3][r]) as f32),
            ],
        }
    }
}
