//! primase's own vocabulary for what a shader program exposes: its interface (varyings and system values) and its
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
mod program;
mod resource;
mod select_variant;
pub mod surface;

use serde::Serialize;

pub use binding::{
    AddressMode, AlphaComparison, AlphaTest, Blend, BlendEquation, BlendFactor, BlendOp, BufferBinding,
    BufferSource, BufferTable, Compare, ConstantBinding, ConstantBufferBinding, ConstantRole, Cull,
    DepthBias, DepthState, Fill, FrameParameter, FrameTexture, InputBinding, InputRole, KeyPart, LaneAffine,
    LaneStream, MatrixLines, NormalEncoding, NormalSpace, ObjectMatrix, ObjectParameter, Occluded,
    OutputBinding, OutputRole, RenderState, SamplerState, ScalarEncoding, ShaderBinding, SkinPrefix,
    StageBinding, Stencil, StencilFace, StencilOp, TargetState, TextureBinding, TextureSource, Unshown,
    VariantKey, ViewParameter,
};
pub use facts::{InstructionCount, InstructionUnit, MaterialPrograms, ProgramFacts, ProgramResourceFacts};
pub use interface::{
    D3dSignature, DomainFacts, GeometryFacts, GsInput, GsOutput, HitGroup, HitGroupKind, HullFacts,
    Interface, InterpMode, InterpSampling, Interpolation, Label, MeshFacts, MeshTopology, Partitioning,
    RayTracingFacts, ScalarType, StageFacts, Subobject, SystemValue, TessDomain, TessTopology, Value,
    Varying, VaryingProvenance,
};
pub use program::Shader;
pub use resource::{
    Access, BufferKind, BufferLayout, Constant, ConstantType, D3dBinding, D3dClass, Majorness, Member,
    Resource, ResourceId, ResourceKind, ResourceProvenance, SampleType, StorageTarget, TextureDim,
};
pub use select_variant::{SELECT_VARIANT, SelectVariant};
pub use surface::Surface;

/// How a shader program's bytes are encoded, for every game and target (the way [`crate::TexFormat`] is for texture
/// bytes). Only the encoding lives here; reading one is the job of the crate that knows it (`primase-dxbc`,
/// `primase-dxil`), and the program's meaning is lifted from it on demand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ShaderFormat {
    /// A DXBC container holding Shader Model 4 or 5 byte code.
    Dxbc,
    /// A DXBC container holding Shader Model 6 (DXIL, LLVM bitcode).
    Dxil,
    /// A SPIR-V module.
    SpirV,
    /// GLSL source text.
    Glsl,
    /// Raw Direct3D 9 shader tokens, including the version token and END (no DXBC container).
    D3d9,
}

/// The pipeline stage a program runs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Stage {
    Vertex,
    Pixel,
    /// A compute program: no varyings, system inputs naming the invocation, a thread group size.
    Compute,
    /// A hull program: the control-point phase and the patch-constant function of a tessellated patch (see
    /// [`StageFacts::Hull`]).
    Hull,
    /// A domain program: a vertex of a tessellated patch from its domain location (see [`StageFacts::Domain`]).
    Domain,
    /// A geometry program.
    Geometry,
    /// A mesh program: a thread group that writes the vertices and primitives of one meshlet (see
    /// [`StageFacts::Mesh`]); its outputs are the per-vertex varyings of the interface.
    Mesh,
    /// A ray generation program: the start of a ray tracing dispatch, one invocation per dispatched ray position.
    RayGeneration,
    /// An intersection program: tests a ray against a procedural primitive and reports hits.
    Intersection,
    /// An any-hit program: decides whether a candidate hit counts.
    AnyHit,
    /// A closest-hit program: runs for the nearest accepted hit of a traced ray.
    ClosestHit,
    /// A miss program: runs when a traced ray hits nothing.
    Miss,
    /// A callable program: run by another ray tracing program (`CallShader`).
    Callable,
}

impl Stage {
    /// Whether this is one of the ray tracing pipeline's stages: a program of it is functions that read the ray's
    /// system values and carry a payload, not an entry point (see [`crate::shader`]'s `stage_functions` in
    /// primase-shader).
    pub fn is_ray_tracing(self) -> bool {
        matches!(
            self,
            Self::RayGeneration
                | Self::Intersection
                | Self::AnyHit
                | Self::ClosestHit
                | Self::Miss
                | Self::Callable
        )
    }
}
