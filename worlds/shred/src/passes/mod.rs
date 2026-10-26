//! Source approximations, selected by the program and reported where they are made.
pub mod fog;
pub mod materials;
pub use fog::{LOWER_SHRED_FOG, LowerShredFog};
pub use materials::{Alpha, LOWER_SHRED_MATERIALS, LowerShredMaterials, Normals};
