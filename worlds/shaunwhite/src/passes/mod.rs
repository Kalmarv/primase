//! Shaun White Skateboarding's lowerings: its stored data to core meanings.

mod material;
mod select;
pub use material::{LOWER_SHAUNWHITE_MATERIAL, LowerShaunwhiteMaterial};
pub use select::{Class, Keep, ROWS, Row, SELECT_SHAUNWHITE_CONTENT, SelectShaunwhiteContent, Subject, Test};

use primase_core::pass::PassRegistry;

/// Every pass of this crate, by name.
pub fn register(reg: &mut PassRegistry) {
    reg.add(&LOWER_SHAUNWHITE_MATERIAL);
    reg.add(&SELECT_SHAUNWHITE_CONTENT);
}
