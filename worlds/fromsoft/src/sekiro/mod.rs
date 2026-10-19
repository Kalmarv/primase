//! Sekiro area reading: each placed model as its FLVER as stored, its materials as resolved source records, the
//! texture files and layered-shader programs they bind, the placed map collision files and object collision bodies as
//! stored, lights, hang edges and spawn points, with typed source identities ([`source`]). The read selects source
//! records (the region's placements, lights and spawn points; the parts' collision files) and converts frames; it
//! does not lower: geometry, materials, collision, lights and extras become core data through the passes in
//! [`passes`], [`collision`] and [`objects`], which a program chooses and orders. What the read leaves out or cannot
//! read is in its [`Report::notices`].
mod components;
pub mod passes;
mod read;
pub use components::source;
pub use passes::collision;
pub use passes::extras;
pub use passes::object_collision as objects;
pub use passes::{blockers, layers, lights, normals, pbr, settings};
pub use read::area::{Options, Report, StateClasses, read_area, world_bounds};
pub use read::collision::Report as CollisionReport;
pub use read::extras::{Report as ExtrasReport, light_record};
pub use read::objects::Report as ObjectCollisionReport;
pub use read::{convert, states};
