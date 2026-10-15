//! Static layered-material bake policy. This is not runtime sampler state.
mod sampler;
pub use sampler::BaseMipBilinearWrap;
mod binding;
pub use binding::{Bake, Inputs, Prepared};
