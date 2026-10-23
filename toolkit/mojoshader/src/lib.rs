//! Lifting the GLSL dialect MojoShader writes from Shader Model 2 and 3 programs (its translation of D3D9 bytecode for
//! OpenGL, which engines ship their effects compiled to) to primase's shader representation
//! ([`primase_shader::Program`]).
//!
//! This reads that dialect, not GLSL: no `#version` (GLSL 1.10), `attribute` and `varying` declarations, registers as
//! global `vec4` variables (`ps_r0`, `vs_a0` an `ivec4`), constants as `const` declarations and `#define` aliases of a
//! `uniform vec4 ps_uniforms_vec4[N]` array, one statement per instruction on swizzled registers, `if`/`else`, `rep`
//! loops with constant counts, relative addressing through `vs_a0`, `texture2D`, and the handful of built-in functions
//! the translation emits. Anything else is refused by name ([`Error`]), because it is not what the translation writes.
//!
//! What the program is:
//! - a vertex program reads its attributes (each a `vec4` argument named after the semantic the caller states) and
//!   writes `gl_Position` (the system value [`primase_shader::SystemValue::ClipPosition`]) and its varyings;
//! - a pixel program reads varyings and writes `gl_FragColor` or `gl_FragData[n]` (targets 0 to 3);
//! - constants are the uniform array, a constant buffer `cb0` of `vec4<u32>` rows read as floats (an index past the
//!   array reads zero, as D3D9's relative addressing does); samplers are a texture `t<slot>` and a sampler `s<slot>`.
//! - MojoShader ends every vertex program with three statements that fix the clip space for OpenGL (`posFixup`: flip
//!   y, half-pixel offset, depth 0..1 to -1..1). They are the API's, not the program's: they are dropped, the program
//!   keeps D3D's clip space (the conventions say so), and a note says it did.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod lex;
mod lower;

use primase_shader::{Program, Stage};

/// Why a program is refused: the line of the source and what in it is outside the dialect.
#[derive(Debug, thiserror::Error)]
#[error("MojoShader GLSL at line {line}: {detail}")]
pub struct Error {
    pub line: usize,
    pub detail: String,
}

pub type Result<T> = std::result::Result<T, Error>;

/// The reason of a refusal without the line, to count refusals by.
pub fn refusal_reason(e: &Error) -> String {
    e.detail.clone()
}

/// What the caller knows about the program that its text does not say.
#[derive(Clone, Debug, Default)]
pub struct Hints {
    /// The source's name for the target (`opengl effect v12`), kept in the program's provenance.
    pub target: String,
    /// The program's constants: the layout of the uniform array.
    pub constants: Vec<Constant>,
    /// The samplers the program declares.
    pub samplers: Vec<Sampler>,
    /// The vertex attributes the program declares.
    pub attributes: Vec<Attribute>,
}

/// A named constant at a byte offset of the uniform array, `bytes` long (a whole number of registers), `width` lanes
/// wide when it is a scalar or a vector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Constant {
    pub name: String,
    pub offset: u32,
    pub bytes: u32,
    pub width: u8,
}

/// A sampler: the name the program gives it (`ps_s0`), the slot the texture is bound at and the name of what is bound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sampler {
    pub glsl_name: String,
    pub slot: u32,
    pub name: String,
}

/// A vertex attribute: the name the program gives it (`vs_v0`) and the semantic and index it is bound by (`TEXCOORD`,
/// 1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute {
    pub glsl_name: String,
    pub semantic: String,
    pub index: u32,
}

/// The version of the lifter: bumped when what it produces for the same text changes.
pub const LIFTER_VERSION: u32 = 1;

/// Lift the vertex or pixel program `source`. Only [`Stage::Vertex`] and [`Stage::Pixel`] are programs of this
/// dialect.
pub fn lift(source: &str, stage: Stage, hints: &Hints) -> Result<Program> {
    lower::lift(source, stage, hints)
}
