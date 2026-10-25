//! Lighting approximations on core lights: ambient light as a grid of fill lights ([`fill`], pass
//! `fill-ambient-lights`), kept off the world's surfaces by a surface index ([`surfaces`]); emissive surfaces as point
//! lights ([`emissive`], pass `emissive-lights`); how bright emissive surfaces draw ([`glow`], pass `scale-emission`).
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod emissive;
pub mod fill;
pub mod glow;
pub mod surfaces;

pub use emissive::{Cone, EMISSIVE_LIGHTS, EmissiveLights, EmissivePolicy, Spots};
pub use fill::{FILL_AMBIENT_LIGHTS, FillAmbientLights, FillCells, FillGrid, FillPoint};
pub use glow::{SCALE_EMISSION, ScaleEmission};
pub use surfaces::{SurfaceGrid, Triangle, placed_triangles};

/// Register this crate's passes by their declarations: `fill-ambient-lights` ([`FILL_AMBIENT_LIGHTS`]) and
/// `emissive-lights` ([`EMISSIVE_LIGHTS`]) and `scale-emission` ([`SCALE_EMISSION`]), each with every option required.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    reg.add(&FILL_AMBIENT_LIGHTS);
    reg.add(&EMISSIVE_LIGHTS);
    reg.add(&SCALE_EMISSION);
}
