//! Forza Horizon 6's own selections and lowerings, each a registered pass.
pub mod collision;
pub mod drawn;
pub mod main_view;
pub mod materials;
pub mod planar_uv;

pub use collision::{COLLIDE_FORZATECH_DRAWN, CollideForzaDrawn, SeeThrough};
pub use drawn::{SELECT_FORZATECH_DRAWN, SelectForzaDrawn};
pub use main_view::{SELECT_FORZATECH_MAIN_VIEW, SelectForzaMainView};
pub use materials::{LOWER_FORZATECH_MATERIALS, LowerForzaMaterials};
pub use planar_uv::{LOWER_FORZATECH_PLANAR_UV, LowerForzaPlanarUv};
