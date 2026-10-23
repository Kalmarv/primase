//! Static layered-material bake policy. This is not runtime sampler state.
mod sampler;
pub use sampler::BaseMipBilinearWrap;
mod binding;
pub(crate) use binding::OutputRgb;
pub(crate) use binding::sampled_period;
pub use binding::{Bake, FrozenFrame, Inputs, Prepared, SECOND_UV_APPROXIMATION, TEXTURE_SCROLL_POLICY};
pub mod grouping;
pub mod period;
pub mod resolve;
