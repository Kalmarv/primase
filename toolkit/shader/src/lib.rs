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

pub mod d3d;
mod evaluate;
pub use evaluate::{
    Constants, Evaluation, Field, Grid, Image, Input, Invocation, Lane, Layout, Raw, Texture, evaluate,
    evaluate_raw,
};
mod periodic;
pub use periodic::evaluate_periodic;

/// How a sample request picks its level of detail.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lod {
    /// The implicit level (`sample`), whose footprint a scalar evaluation leaves to the sampler.
    Implicit,
    /// The implicit level plus a bias the program states (`sample_b`, as given: the sampler clamps it to D3D's
    /// -16..15.99).
    Bias(f32),
    /// A level the program states (`sample_l`).
    Level(f32),
    /// The UV's screen derivatives: those the program states (`sample_d`), or a quad's own for an implicit-LOD
    /// sample (d/dx, then d/dy).
    Gradients([[f32; 2]; 2]),
    /// A quad's own UV derivatives plus the bias the program states (`sample_b` in a quad).
    GradientsBias([[f32; 2]; 2], f32),
}

/// A sample request of a 2D texture (or a 2D array's layer).
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    /// Texture bind slot (`t<n>`).
    pub texture: u32,
    /// Sampler bind slot (`s<n>`).
    pub sampler: u32,
    pub uv: [f32; 2],
    /// A 2D array's layer coordinate, as the program gives it (the sampler rounds it); `None` for a 2D texture.
    pub layer: Option<f32>,
    pub lod: Lod,
}

/// A load request (`ld`): one texel by its integer address, without a sampler.
#[derive(Clone, Copy, Debug)]
pub struct Load {
    /// Texture bind slot (`t<n>`).
    pub texture: u32,
    /// x, y, and a 2D array's layer or a 3D texture's z (0 for a 2D texture); a buffer's element at x (y and z 0).
    pub texel: [i32; 3],
    /// The mip level (0 for a buffer).
    pub mip: i32,
}

/// A store request (`store_uav_typed`, `store_structured`).
#[derive(Clone, Copy, Debug)]
pub struct Store {
    /// Unordered-access view bind slot (`u<n>`).
    pub view: u32,
    /// A typed view's x, y, and z or a 2D array's layer (0 where the view has fewer); a structured view's element and
    /// byte offset (then 0).
    pub address: [u32; 3],
    /// The lanes as the program writes them (bits), only those `mask` marks.
    pub value: [u32; 4],
    pub mask: u8,
}

/// An atomic add (`imm_atomic_iadd`) to the 32-bit word of an unordered-access view at `address`: a structured view's
/// element and byte offset in its first two lanes, a typed view's x, y (and z) otherwise (0 where the view has fewer).
#[derive(Clone, Copy, Debug)]
pub struct AtomicAdd {
    /// Unordered-access view bind slot (`u<n>`).
    pub view: u32,
    pub address: [u32; 3],
    pub value: u32,
}

/// Answers sample, load, store and structured-buffer requests; an error stops the evaluation. A closure answers samples
/// only.
pub trait Sampler {
    fn sample(&mut self, request: Sample) -> std::result::Result<[f32; 4], String>;
    /// The texel's four lanes as its view returns them: f32 bits for a float view, the integers for a uint or sint
    /// one.
    fn load(&mut self, request: Load) -> std::result::Result<[u32; 4], String> {
        Err(format!("t{}: no loads given", request.texture))
    }
    /// A store to an unordered-access view ([`Store`]): the lanes `mask` marks of `value`, at the address the program
    /// gives (a structured view's lanes the consecutive words from the byte offset). Its effect is the caller's: it
    /// reaches none of this evaluation's outputs.
    fn store(&mut self, request: Store) -> std::result::Result<(), String> {
        Err(format!("u{}: no stores given", request.view))
    }
    /// An atomic add ([`AtomicAdd`]); its result the word as it was before. The add itself is the caller's effect.
    fn atomic_add(&mut self, request: AtomicAdd) -> std::result::Result<u32, String> {
        Err(format!("u{}: no atomics given", request.view))
    }
    /// The 32-bit word at byte `offset` of element `element` of the structured buffer at `t<slot>`.
    fn structured(&mut self, slot: u32, element: u32, offset: u32) -> std::result::Result<u32, String> {
        let _ = (element, offset);
        Err(format!("t{slot}: no structured buffer given"))
    }
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
    /// Whether the program discarded the pixel (`discard`): its outputs are then those written before.
    pub discarded: bool,
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
impl Type {
    /// 32-bit words the type occupies in a constant buffer under D3D packing: each matrix row (row-major) or column
    /// (column-major), array element and struct starts a four-word register, and the last one takes only its own
    /// words. None for object types (class 4 and above, except structs) and for overflow.
    pub fn packed_words(&self) -> Option<u32> {
        let (rows, columns) = (u32::from(self.rows.max(1)), u32::from(self.columns.max(1)));
        let one = match self.class {
            0 | 1 => rows.checked_mul(columns)?,
            2 => (rows - 1).checked_mul(4)?.checked_add(columns)?,
            3 => (columns - 1).checked_mul(4)?.checked_add(rows)?,
            5 => self.members.iter().try_fold(0u32, |end, m| {
                Some(end.max(m.offset.div_ceil(4).checked_add(m.ty.packed_words()?)?))
            })?,
            _ => return None,
        };
        match u32::from(self.elements) {
            0 => Some(one),
            n => (n - 1)
                .checked_mul(one.div_ceil(4).checked_mul(4)?)?
                .checked_add(one),
        }
    }
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
    /// Evaluate once with the inputs' screen derivatives: per input register, its d/dx then d/dy lanes (f32 bits),
    /// for programs that read derivatives. A bytecode that reads none evaluates as [`PixelShader::execute`].
    fn execute_with_derivatives(
        &self,
        inputs: &[[u32; 4]],
        derivatives: &[[[u32; 4]; 2]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> std::result::Result<Execution, Error> {
        let _ = derivatives;
        self.execute(inputs, buffers, sampler)
    }
    /// Evaluate a 2 x 2 quad together (lanes 0, 1 the top row, 2, 3 the bottom), its derivatives and implicit
    /// levels of detail the quad's own. A bytecode without quad evaluation refuses.
    fn execute_quad(
        &self,
        inputs: [&[[u32; 4]]; 4],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> std::result::Result<[Execution; 4], Error> {
        let _ = (inputs, buffers, sampler);
        Err(Error {
            bytecode: "this bytecode",
            at: 0,
            detail: "no quad evaluation".to_owned(),
        })
    }
}
