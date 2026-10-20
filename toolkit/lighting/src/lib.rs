//! Lighting approximations on core lights: ambient light as a grid of fill lights ([`fill`], pass
//! `fill-ambient-lights`), kept off the world's surfaces by a surface index ([`surfaces`]).
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod fill;
pub mod surfaces;

pub use fill::{FillAmbientLights, FillCells, FillGrid, FillPoint};
pub use surfaces::{SurfaceGrid, Triangle, placed_triangles};

/// Register this crate's passes: `fill-ambient-lights`, whose grid is the program's choice (every option required).
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    reg.register_requiring(
        "fill-ambient-lights",
        |o| Ok(Box::new(FillAmbientLights::new(FillGrid::from_options(o)?))),
        &[
            ("anchor", "0,0,0"),
            ("spacing", "15"),
            ("bounds", "0,0,0,15,15,15"),
            ("cells", "meeting"),
            ("clearance", "3.75"),
            ("reach", "22.5"),
        ],
    );
}
