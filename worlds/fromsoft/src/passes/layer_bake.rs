//! Static layered-material bake policy. This is not runtime sampler state.
mod sampler;
pub use sampler::{FILTERING, base_texture, sample, srgb, stored, wrapped_texture};
mod binding;
pub(crate) use binding::OutputRgb;
pub use binding::{
    ALPHA_TEST_POLICY, BIND_POSE_POLICY, Bake, FrozenFrame, INSTANCE_DATA_POLICY, Inputs, Prepared, Programs,
    SECOND_UV_APPROXIMATION, TEXTURE_SCROLL_POLICY,
};
pub(crate) use binding::{executor, program_executor, sampled_period};
pub use vertex::COLOUR1_POLICY;
pub mod grouping;
pub mod period;
pub mod resolve;
mod state;
pub use state::{AlphaState, Encoded, PipelineState, PixelState, SCHEMA, SlotTexture, TARGETS, VertexState};
mod vertex;
