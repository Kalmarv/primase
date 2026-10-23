//! Static layered-material bake policy. This is not runtime sampler state.
mod sampler;
pub use sampler::BaseMipBilinearWrap;
mod binding;
pub(crate) use binding::OutputRgb;
pub(crate) use binding::sampled_period;
pub use binding::{Bake, FrozenFrame, Inputs, Prepared};
pub mod grouping;
pub mod period;
pub mod resolve;
