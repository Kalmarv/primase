//! What a program is and where it runs: its encoding and its pipeline stage.
use serde::Serialize;

/// How a shader program's bytes are encoded, for every game and target (the way [`crate::TexFormat`] is for texture
/// bytes). Only the encoding lives here; reading one is the job of the crate that knows it (`iridia-dxbc`,
/// `iridia-dxil`), and the program's meaning is lifted from it on demand.
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
    /// iridia).
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
