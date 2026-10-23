//! Static layered-material bake policy. This is not runtime sampler state.
mod sampler;
pub use sampler::BaseMipBilinearWrap;
mod binding;
pub(crate) use binding::OutputRgb;
pub(crate) use binding::sampled_period;
pub use binding::{
    ALPHA_TEST_POLICY, BIND_POSE_POLICY, Bake, FrozenFrame, INSTANCE_DATA_POLICY, Inputs, Prepared, Programs,
    SECOND_UV_APPROXIMATION, TEXTURE_SCROLL_POLICY,
};
pub use vertex::COLOUR1_POLICY;
pub mod grouping;
pub mod period;
pub mod resolve;
mod vertex;
