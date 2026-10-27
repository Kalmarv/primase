//! A vocabulary for what a shader program exposes: its interface (varyings and system values) and its
//! resources (constant buffers with explicit layouts, textures, samplers, buffers, storage), independent of any
//! graphics API.
//!
//! A reader maps its API's reflection into this once and keeps the API's own names and addresses in provenance, so a
//! consumer that only needs meaning (a binding pass, a validator, a writer) never sees an API's enum numbers. The
//! rule for differences between APIs is the shader design's: conventions are normalized (D3D register packing is
//! unpacked into one varying per label, D3D, std140 and DX9 constant packings all become explicit byte offsets and
//! strides), and the original is kept in provenance; behaviour that differs is not hidden here at all (that is the
//! program body's business). A value a reader cannot map is refused by name, never mapped to a guess.
//!
//! Everything is plain data: owned strings, integers and enums, no handles and no GPU types, and every type
//! serializes, so the vocabulary can be shown, stored and compared.

mod binding;
mod facts;
mod interface;
mod resource;
mod stage;
pub mod surface;

pub use binding::{
    AddressMode, AlphaComparison, AlphaTest, Blend, BlendEquation, BlendFactor, BlendOp, BufferBinding,
    BufferSource, BufferTable, Compare, ConstantBinding, ConstantBufferBinding, ConstantRole, Cull,
    DepthBias, DepthState, Fill, FrameParameter, FrameTexture, InputBinding, InputRole, KeyPart, LaneAffine,
    LaneStream, MatrixLines, NormalEncoding, NormalSpace, ObjectMatrix, ObjectParameter, Occluded,
    OutputBinding, OutputRole, RenderState, SamplerState, ScalarEncoding, SkinPrefix, Stencil, StencilFace,
    StencilOp, TargetState, Unshown, VariantKey, ViewParameter,
};
pub use binding::{Attribute, Filter, Filters, MipFilter};
pub use facts::{InstructionCount, InstructionUnit, ProgramFacts, ProgramResourceFacts};
pub use interface::{
    D3dSignature, DomainFacts, GeometryFacts, GsInput, GsOutput, HitGroup, HitGroupKind, HullFacts,
    Interface, InterpMode, InterpSampling, Interpolation, Label, MeshFacts, MeshTopology, Partitioning,
    RayTracingFacts, ScalarType, StageFacts, Subobject, SystemValue, TessDomain, TessTopology, Value,
    Varying, VaryingProvenance,
};
pub use resource::{
    Access, BufferKind, BufferLayout, Constant, ConstantType, D3dBinding, D3dClass, Majorness, Member,
    Resource, ResourceId, ResourceKind, ResourceProvenance, SampleType, StorageTarget, TextureDim,
};
pub use stage::{ShaderFormat, Stage};
pub use surface::Surface;
