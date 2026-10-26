//! Shaun White Skateboarding's lowerings: its stored data to core meanings.

mod material;
pub use material::{LOWER_SHAUNWHITE_MATERIAL, LowerShaunwhiteMaterial};

use primase_core::pass::PassRegistry;

/// Every pass of this crate, by name.
pub fn register(reg: &mut PassRegistry) {
    reg.add(&LOWER_SHAUNWHITE_MATERIAL);
}
