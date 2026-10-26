//! Shaun White Skateboarding's lowerings: its stored data to core meanings.

mod collision;
mod grind;
mod layers;
mod lighting;
mod material;
mod select;
mod water;
pub use collision::{LOWER_SHAUNWHITE_COLLISION, LowerShaunwhiteCollision};
pub use grind::{LOWER_SHAUNWHITE_GRINDS, LowerShaunwhiteGrinds, RAIL_MAX_UP, RAIL_MIN_UP, rail_edge};
pub use layers::{LOWER_SHAUNWHITE_LAYERS, LowerShaunwhiteLayers};
pub use lighting::{LOWER_SHAUNWHITE_LIGHTING, LightColour, LowerShaunwhiteLighting, SpotAxis, SunVector};
pub use material::{LOWER_SHAUNWHITE_MATERIAL, LowerShaunwhiteMaterial};
pub use select::{Class, Keep, ROWS, Row, SELECT_SHAUNWHITE_CONTENT, SelectShaunwhiteContent, Subject, Test};
pub use water::{LOWER_SHAUNWHITE_WATER, LowerShaunwhiteWater, WATER_SURFACES};

use primase_core::pass::PassRegistry;

/// Every pass of this crate, by name.
pub fn register(reg: &mut PassRegistry) {
    reg.add(&LOWER_SHAUNWHITE_MATERIAL);
    reg.add(&SELECT_SHAUNWHITE_CONTENT);
    reg.add(&LOWER_SHAUNWHITE_COLLISION);
    reg.add(&LOWER_SHAUNWHITE_WATER);
    reg.add(&LOWER_SHAUNWHITE_GRINDS);
    reg.add(&LOWER_SHAUNWHITE_LIGHTING);
    reg.add(&LOWER_SHAUNWHITE_LAYERS);
}
