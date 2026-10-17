//! One interface for evaluating a pixel shader on the CPU, whatever its bytecode: [`PixelShader`] with the
//! reflection a caller binds inputs, constants and textures by, and the [`Sampler`] callback that owns texture
//! sampling. `primase-dxbc` (Shader Model 5) and `primase-dxil` (Shader Model 6) implement it; a bake written
//! against it runs either.
//!
//! Registers are four 32-bit lanes. Inputs and outputs are indexed by signature register, constant buffers are
//! rows of four lanes by bind slot, and textures are identified by bind slot (`t<n>`). Sampling is the caller's:
//! a scalar evaluation has no derivatives, so the callback states its footprint, addressing, filtering and
//! transfer policy.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

/// An implicit-LOD sample request of a 2D texture.
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    /// Texture bind slot (`t<n>`).
    pub texture: u32,
    /// Sampler bind slot (`s<n>`).
    pub sampler: u32,
    pub uv: [f32; 2],
}
/// Answers sample requests; an error stops the evaluation.
pub trait Sampler {
    fn sample(&mut self, request: Sample) -> std::result::Result<[f32; 4], String>;
}
impl<F: FnMut(Sample) -> std::result::Result<[f32; 4], String>> Sampler for F {
    fn sample(&mut self, request: Sample) -> std::result::Result<[f32; 4], String> {
        self(request)
    }
}

/// What one evaluation wrote.
#[derive(Debug)]
pub struct Execution {
    /// One entry per output register.
    pub outputs: Vec<[u32; 4]>,
    /// Only lanes marked in this mask were written by the shader.
    pub output_masks: Vec<u8>,
    pub instructions: usize,
    pub samples: usize,
}

/// Resource, constant-buffer and signature metadata, as D3D reflection states it.
#[derive(Clone, Debug)]
pub struct Reflection {
    pub buffers: Vec<Buffer>,
    pub resources: Vec<Resource>,
    pub inputs: Vec<Semantic>,
    pub outputs: Vec<Semantic>,
}
/// A bound resource. `kind` follows D3D_SHADER_INPUT_TYPE (0 constant buffer, 2 texture, 3 sampler) and
/// `dimension` D3D_SRV_DIMENSION (4 a 2D texture).
#[derive(Clone, Debug)]
pub struct Resource {
    pub name: String,
    pub kind: u32,
    pub return_type: u32,
    pub dimension: u32,
    pub slot: u32,
    pub count: u32,
}
#[derive(Clone, Debug)]
pub struct Buffer {
    pub name: String,
    /// Bytes.
    pub size: u32,
    pub variables: Vec<Variable>,
}
#[derive(Clone, Debug)]
pub struct Variable {
    pub name: String,
    /// Byte offset in the buffer.
    pub offset: u32,
    pub size: u32,
    pub ty: Type,
    pub default: Option<Vec<u8>>,
}
#[derive(Clone, Debug)]
pub struct Type {
    pub class: u16,
    pub scalar: u16,
    pub rows: u16,
    pub columns: u16,
    /// Zero means not an array, as in D3D reflection.
    pub elements: u16,
    pub members: Vec<Member>,
}
#[derive(Clone, Debug)]
pub struct Member {
    pub name: String,
    /// Byte offset in the enclosing type.
    pub offset: u32,
    pub ty: Type,
}
/// A signature element.
#[derive(Clone, Debug)]
pub struct Semantic {
    pub name: String,
    pub index: u32,
    pub system_value: u32,
    pub component_type: u32,
    pub register: u32,
    pub mask: u8,
    pub used_mask: u8,
}

/// An evaluation or binding error, located in the implementation's own terms (a DXBC word, a DXIL value).
#[derive(Debug, thiserror::Error)]
#[error("{bytecode} at {at}: {detail}")]
pub struct Error {
    pub bytecode: &'static str,
    pub at: usize,
    pub detail: String,
}

/// A checked pixel shader that can be evaluated.
pub trait PixelShader: std::fmt::Debug {
    fn reflection(&self) -> &Reflection;
    /// Input registers an evaluation needs.
    fn input_count(&self) -> usize;
    /// Constant buffers the code reads: bind slot and its size in rows of four lanes.
    fn constant_buffers(&self) -> &[(u32, u32)];
    /// Texture bind slots the code samples.
    fn textures(&self) -> &[u32];
    /// Sampler bind slots the code uses.
    fn samplers(&self) -> &[u32];
    /// Evaluate once. Missing inputs or buffers are errors; no partial output is returned (the sampler may already
    /// have been called).
    fn execute(
        &self,
        inputs: &[[u32; 4]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> std::result::Result<Execution, Error>;
}
