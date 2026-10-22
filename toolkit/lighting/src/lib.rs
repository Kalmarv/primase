//! Lighting approximations on core lights: ambient light as a grid of fill lights ([`fill`], pass
//! `fill-ambient-lights`), kept off the world's surfaces by a surface index ([`surfaces`]); emissive surfaces as point
//! lights ([`emissive`], pass `emissive-lights`).
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod emissive;
pub mod fill;
pub mod surfaces;

pub use emissive::{EmissiveLights, EmissivePolicy};
pub use fill::{FillAmbientLights, FillCells, FillGrid, FillPoint};
pub use surfaces::{SurfaceGrid, Triangle, placed_triangles};

/// Register this crate's passes: `fill-ambient-lights`, whose grid is the program's choice (every option required).
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    reg.register_requiring(
        "fill-ambient-lights",
        |o| Ok(Box::new(FillAmbientLights::new(FillGrid::from_options(o)?))),
        &[
            ("anchor", "0;0;0"),
            ("spacing", "15"),
            ("bounds", "0;0;0;15;15;15"),
            ("cells", "meeting"),
            ("clearance", "3.75"),
            ("reach", "22.5"),
        ],
    );
    // what the emission becomes, how a string is thinned and what is too many are the program's: every option required
    reg.register_requiring(
        "emissive-lights",
        emissive::emissive_lights,
        &[
            ("weld", "0.001"),
            ("big", "2.5"),
            ("string", "8"),
            ("per_string", "6"),
            ("strip", "8"),
            ("nits_per_strength", "558.5"),
            ("area_factor", "1"),
            ("min_luminance", "0.05"),
            ("e_min", "2"),
            ("range_min", "3"),
            ("range_max", "30"),
            ("max_total", "3500"),
            ("shadows", "off"),
            ("animated", "none"),
        ],
    );
}
