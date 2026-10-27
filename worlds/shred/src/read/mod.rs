//! World assembly and the conversion adapter over `rme-formats`' readers: level geometry, collision, source materials
//! and their texture graph, reference rigs and name joins.
pub mod collision;
pub mod convert;
pub mod level;
pub mod manifest;
pub mod material;
pub mod material_join;
pub mod names;
pub mod reference_rig;
pub mod texture;
