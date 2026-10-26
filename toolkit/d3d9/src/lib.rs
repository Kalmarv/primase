//! Raw Direct3D 9 `ps_3_0` and `vs_3_0` tokens, their CTAB reflection, and lifting to the shared shader IR.
//!
//! This is a bounded shader reader: straight-line float arithmetic, declarations, immediate constants,
//! 2D/cube/volume sampling and discard. Vertex programs additionally keep input/output declarations and clip position. Vertex texture fetch, point size,
//! other models, control flow, relative addressing and modifiers outside
//! this subset are refused by name and word offset. No translated text or native compiler is needed at runtime.
//! [`read`] preserves comments and CTAB's type/default metadata; [`lift`] normalizes registers into varyings and
//! a float constant bank (raw words in `cb0`, register `cN` at byte `16*N`). A `def` overrides that bank locally.
//!
//! Token layout: Microsoft's [instruction](https://learn.microsoft.com/windows-hardware/drivers/display/instruction-token),
//! [source](https://learn.microsoft.com/windows-hardware/drivers/display/source-parameter-token) and
//! [destination](https://learn.microsoft.com/windows-hardware/drivers/display/destination-parameter-token) tokens.
//! Vertex register banks are bounded to 16 inputs, 32 temporaries, 12 declared outputs and 256 float constants;
//! [the VS register reference](https://learn.microsoft.com/windows/win32/direct3dhlsl/dx9-graphics-reference-asm-vs-registers-vs-3-0)
//! and [output declarations](https://learn.microsoft.com/windows/win32/direct3dhlsl/dcl-usage-outut-register---vs) define their layout.
//! Arithmetic follows the [ps_3_0 instruction reference](https://learn.microsoft.com/windows/win32/direct3dhlsl/dx9-graphics-reference-asm-ps-instructions-ps-3-0).
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod ctab;
mod lower;
mod sampler;
pub use sampler::sampler_states;
mod tokens;

pub use ctab::{Constant, ConstantTable, Type, TypeMember};
pub use tokens::{Comment, Destination, Input, Instruction, Opcode, Output, Register, Shader, Source, read};

/// A refusal states the word offset in the raw token stream and the unsupported or malformed feature.
pub type Error = primase_shader::Refusal;
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn error(at: usize, detail: impl Into<String>) -> Error {
    Error::new("D3D9", primase_shader::Unit::Word, at, detail)
}

/// Increment when the same tokens lift differently. Hand-worked fixtures gate this version.
pub const LIFTER_VERSION: u32 = 1;

/// Lift one complete `ps_3_0` or `vs_3_0` program; trailing bytes and unknown features are errors.
pub fn lift(bytes: &[u8]) -> Result<primase_shader::Program> {
    lower::lift(&read(bytes)?, bytes)
}

/// A register value immediately after one original instruction. `lanes` order is the projected RGB order.
/// The word offset must identify an instruction, and all three distinct temporary lanes must be initialized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Projection {
    pub after_word: usize,
    pub temporary: u32,
    pub lanes: [u8; 3],
}

/// Pixel-only: lift the original instruction prefix through `cut`, returning its selected lanes as RGB and alpha one.
/// Prior discards remain; subsequent operations/discards are excluded. This is a derived program, whose notes
/// name the cut, whose lifter identity includes the projection, and whose source hash still identifies the complete original bytecode. No meaning is assigned
/// to the chosen value: recognizing a useful cut is the caller's responsibility.
pub fn project(bytes: &[u8], cut: Projection) -> Result<primase_shader::Program> {
    lower::project(&read(bytes)?, bytes, cut, false)
}

/// Pixel-only: lift the complete original program, replacing just the cut's three register lanes with the new float3
/// input `DERIVED_RGB0` (signature register 31). This runs the original downstream operations, including
/// normalization and discards, rather than a rewritten equation. Other register lanes and the prefix remain.
/// An original input v31 is refused. The caller supplies projected RGB in that new input's xyz lanes.
pub fn reencode(bytes: &[u8], cut: Projection) -> Result<primase_shader::Program> {
    lower::project(&read(bytes)?, bytes, cut, true)
}

/// Frame the first raw SM3 vertex or pixel program in a container's remaining bytes.
/// Returns its stage and exact bytes through END; subsequent container metadata is not consumed.
/// This checks token/comment spans, not instruction semantics: an unsupported operation is retained as
/// bytecode for a later lifter to refuse. It never searches for END inside operands or comments.
pub fn frame(bytes: &[u8]) -> Result<(primase_shader::Stage, &[u8])> {
    use primase_shader::Stage;
    let word = |at: usize| -> Result<u32> {
        let b = bytes
            .get(at..at.saturating_add(4))
            .ok_or_else(|| error(at / 4, "truncated raw program token"))?;
        let a: [u8; 4] = b
            .try_into()
            .map_err(|_| error(at / 4, "truncated raw program word"))?;
        Ok(u32::from_le_bytes(a))
    };
    let stage = match word(0)? {
        0xfffe_0300 => Stage::Vertex,
        0xffff_0300 => Stage::Pixel,
        version => {
            return Err(error(
                0,
                format!("raw program version {version:#x}, expected vs_3_0 or ps_3_0"),
            ));
        }
    };
    let mut at = 4usize;
    while at / 4 < 1_048_576 {
        let token = word(at)?;
        at += 4;
        if token == 0xffff {
            return Ok((stage, &bytes[..at]));
        }
        let count = if token & 65535 == 65534 {
            (token >> 16) & 32767
        } else {
            (token >> 24) & 15
        };
        let end = at.saturating_add(count as usize * 4);
        if end > bytes.len() {
            return Err(error(
                at / 4 - 1,
                "raw program operand/comment span exceeds container",
            ));
        }
        at = end;
    }
    Err(error(at / 4, "raw program exceeds 1,048,576 words or lacks END"))
}
