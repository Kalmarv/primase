//! Derivations from SDB2's stored shader records (`anvil_formats::shaunwhite::shaders`); choosing permutations and
//! bindings is a pass.

/// Named graph derivation from the stored forward Colour pixel program.
pub mod albedo;

/// Named base-UV graph derivation from the matching stored generic VS.
pub mod base_uv;

mod material;
pub use material::bind_material;

pub(crate) use material::bind_stage;
