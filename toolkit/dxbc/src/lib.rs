//! Shader Model 4 and 5 programs: the container and token reader (`decode`, `chunks`, `reflection`), the lifter to
//! `primase_shader::Program` (`lift`, any stage), and the older bounded pixel-program evaluator (`Shader`,
//! `Program::execute`). Sampling belongs to the caller of the evaluator: a scalar interpreter cannot infer derivatives, so
//! the caller states its inputs'.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod arity;
pub mod decode;
mod evaluate;
mod lift;
mod opcodes;
mod program;
mod reflection;
pub use decode::{Index, decode};
pub use evaluate::{RegisterValue, Trace};
pub use lift::{LIFTER_VERSION, lift};
pub use primase_shader::{
    AtomicAdd, Execution, Interface, Load, Lod, PixelShader, Resource, Sample, Sampler, Store, Varying,
};
pub use program::{
    Dimension, InputDeclaration, Instruction, Operand, Program, Register, ResourceDeclaration, ReturnType,
    Selection,
};
pub use reflection::{
    SignatureElement, patch_signature_elements, reflection, signature_elements, signatures,
};

/// A DXBC program or container refused or unreadable: the word it is at and why ([`primase_shader::Refusal`]).
pub type Error = primase_shader::Refusal;
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn error(word: usize, detail: impl Into<String>) -> Error {
    Error::new("DXBC", primase_shader::Unit::Word, word, detail)
}

/// A parsed program with its container's interface: the [`PixelShader`] a bytecode-independent caller evaluates.
#[derive(Debug)]
pub struct Shader {
    pub program: Program,
    pub interface: Interface,
}
impl Shader {
    /// Parse a DXBC container's program and reflection.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        Self::with(Program::parse(bytes)?, reflection(bytes)?)
    }
    /// Parse a container whose RDEF was stripped: the program and its signatures ([`signatures`]); the caller
    /// builds the constant layout from its own records.
    pub fn parse_stripped(bytes: &[u8]) -> Result<Self> {
        Self::with(Program::parse(bytes)?, signatures(bytes)?)
    }
    /// The program with its container's interface, whose inputs take the interpolation the program declares: that of
    /// the declarations at an input's register that cover any of its lanes (a declaration covers only the lanes the
    /// program reads). An input the program never declares has none.
    fn with(program: Program, mut interface: Interface) -> Result<Self> {
        for input in &mut interface.inputs {
            let Some(d3d) = input.provenance.d3d else {
                continue;
            };
            let mut declared = program
                .input_declarations()
                .iter()
                .filter(|d| d.register == d3d.register && d.mask & d3d.mask != 0)
                .map(|d| d.interpolation);
            input.interp = declared.next();
            if declared.any(|other| Some(other) != input.interp) {
                return Err(error(
                    0,
                    format!(
                        "input {}{} is declared with different interpolations",
                        input.label.name, input.label.index
                    ),
                ));
            }
        }
        Ok(Self { program, interface })
    }
}
fn shader_error(e: Error) -> primase_shader::Error {
    primase_shader::Error {
        bytecode: "DXBC",
        at: e.at,
        detail: e.detail,
    }
}
impl PixelShader for Shader {
    fn interface(&self) -> &Interface {
        &self.interface
    }
    fn input_count(&self) -> usize {
        self.program.input_count()
    }
    fn constant_buffers(&self) -> &[(u32, u32)] {
        self.program.constant_buffers()
    }
    fn textures(&self) -> &[u32] {
        self.program.textures()
    }
    fn samplers(&self) -> &[u32] {
        self.program.samplers()
    }
    fn execute(
        &self,
        inputs: &[[u32; 4]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> std::result::Result<Execution, primase_shader::Error> {
        self.program
            .execute(inputs, buffers, sampler)
            .map_err(shader_error)
    }
    fn execute_with_derivatives(
        &self,
        inputs: &[[u32; 4]],
        derivatives: &[[[u32; 4]; 2]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> std::result::Result<Execution, primase_shader::Error> {
        self.program
            .execute_with_derivatives(inputs, derivatives, buffers, sampler)
            .map_err(shader_error)
    }
    fn execute_quad(
        &self,
        inputs: [&[[u32; 4]]; 4],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> std::result::Result<[Execution; 4], primase_shader::Error> {
        self.program
            .execute_quad(inputs, buffers, sampler)
            .map_err(shader_error)
    }
}

/// The parts of a DXBC container, an error of the container as one of the program (the word it is at).
pub(crate) fn parts(bytes: &[u8]) -> Result<Vec<primase_dxcontainer::Chunk<'_>>> {
    primase_dxcontainer::chunks(bytes).map_err(|e| error(e.offset / 4, e.detail))
}
