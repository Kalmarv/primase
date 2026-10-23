//! Checked, bounded Shader Model 4 and 5 pixel-program evaluation.
//! Sampling belongs to the caller: a scalar interpreter cannot infer derivatives, so the caller states its inputs'.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod decode;
mod evaluate;
mod opcodes;
mod program;
mod reflection;
pub use decode::{Index, decode};
pub use evaluate::{RegisterValue, Trace};
pub use primase_shader::{
    AtomicAdd, Execution, Interface, Load, Lod, PixelShader, Resource, Sample, Sampler, Store, Varying,
};
pub use program::{
    Dimension, InputDeclaration, Instruction, Operand, Program, Register, ResourceDeclaration, ReturnType,
    Selection,
};
pub use reflection::{reflection, signatures};

#[derive(Debug, thiserror::Error)]
#[error("DXBC at word {word}: {detail}")]
pub struct Error {
    pub word: usize,
    pub detail: String,
}
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn error(word: usize, detail: impl Into<String>) -> Error {
    Error {
        word,
        detail: detail.into(),
    }
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
        at: e.word,
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

/// The checksum a container stores at bytes 4..20: MD5's block function over its bytes from 20 on, the last block
/// padded as the compiler does (the bit count first, then the rest, `0x80`, zeros, and at 60 the bit count over 4,
/// or 1; a rest of 56 bytes or more padded in a block of its own first). `None` for fewer than 20 bytes.
pub fn checksum(container: &[u8]) -> Option<[u8; 16]> {
    let data = container.get(20..)?;
    let bits = u32::try_from(data.len()).ok()?.wrapping_mul(8);
    let (blocks, rest) = data.as_chunks::<64>();
    let mut state = primase_hash::MD5_INIT;
    for block in blocks {
        primase_hash::md5_block(&mut state, block);
    }
    let mut last = [0u8; 64];
    if rest.len() < 56 {
        last[4..4 + rest.len()].copy_from_slice(rest);
        last[4 + rest.len()] = 0x80;
    } else {
        let mut first = [0u8; 64];
        first[..rest.len()].copy_from_slice(rest);
        first[rest.len()] = 0x80;
        primase_hash::md5_block(&mut state, &first);
    }
    last[..4].copy_from_slice(&bits.to_le_bytes());
    last[60..].copy_from_slice(&((bits >> 2) | 1).to_le_bytes());
    primase_hash::md5_block(&mut state, &last);
    Some(primase_hash::md5_bytes(state))
}

/// A borrowed container chunk. Offsets and lengths are checked before exposure.
#[derive(Clone, Copy, Debug)]
pub struct Chunk<'a> {
    pub kind: [u8; 4],
    pub data: &'a [u8],
}

/// Read the DXBC directory, rejecting truncated, overlapping and duplicate code chunks.
/// The container digest is not an authenticity check and is not verified here.
pub fn chunks(bytes: &[u8]) -> Result<Vec<Chunk<'_>>> {
    let word = |at: usize| -> Result<usize> {
        let b = bytes
            .get(at..at + 4)
            .ok_or_else(|| error(at / 4, "short container"))?;
        Ok(u32::from_le_bytes(b.try_into().expect("four bytes")) as usize)
    };
    if bytes.get(..4) != Some(b"DXBC") || word(20)? != 1 || word(24)? != bytes.len() {
        return Err(error(0, "invalid container header or declared size"));
    }
    let n = word(28)?;
    if n > bytes.len().saturating_sub(32) / 4 {
        return Err(error(7, "chunk directory exceeds container"));
    }
    let mut ranges = Vec::new();
    let mut result = Vec::new();
    let mut code = false;
    for i in 0..n {
        let start = word(32 + 4 * i)?;
        if start < 32 + 4 * n || start > bytes.len().saturating_sub(8) || start % 4 != 0 {
            return Err(error(8 + i, "invalid chunk offset"));
        }
        let len = word(start + 4)?;
        let end = (start + 8)
            .checked_add(len)
            .filter(|&e| e <= bytes.len())
            .ok_or_else(|| error(start / 4, "chunk exceeds container"))?;
        if ranges.iter().any(|&(a, b)| start < b && a < end) {
            return Err(error(start / 4, "overlapping chunks"));
        }
        ranges.push((start, end));
        let kind: [u8; 4] = bytes[start..start + 4].try_into().expect("checked header");
        if &kind == b"SHEX" || &kind == b"SHDR" {
            if code {
                return Err(error(start / 4, "multiple shader code chunks"));
            }
            code = true;
        }
        result.push(Chunk {
            kind,
            data: &bytes[start + 8..end],
        });
    }
    Ok(result)
}
