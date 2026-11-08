//! Derivations from SDB2's stored shader records (`anvil_formats::shaunwhite::shaders`); choosing permutations and
//! bindings is a pass.

/// Named graph derivation from the stored forward Colour pixel program.
pub mod albedo;

/// Named base-UV graph derivation from the matching stored generic VS.
pub mod base_uv;

/// [`albedo`] over the IR (`iridia_ir::Program`, `iridia-d3d9-ir`, `iridia-analysis`): the same cut, matched on the IR's value graph.
pub mod albedo_ir;

/// [`base_uv`] over the IR.
pub mod base_uv_ir;

mod material;
pub use material::bind_material;

pub(crate) use material::bind_stage;
