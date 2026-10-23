//! Shader Model 6 (DXIL) programs: the container is the DXBC format (`primase-dxcontainer`); the LLVM bitcode in its `DXIL`
//! part is read by a bounded bitstream reader of our own (`bitstream`, `module`), the reflection (`reflection`, `rdat`), the
//! lifter to `primase_shader::Program` (`lift`, every stage a DXIL container holds) and the older bounded pixel-program
//! evaluator implementing [`primase_shader::PixelShader`] (`Shader`). Sampling belongs to the caller of the evaluator.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]
pub mod bitstream;
mod evaluate;
mod lift;
pub mod module;
pub mod rdat;
pub mod reflection;
pub use evaluate::{Census, Shader};
pub use lift::{LIFTER_VERSION, lift};
pub use reflection::interface;

/// A DXIL program or container refused or unreadable: the bit it is at (in the bitcode, or eight times a byte of the
/// container) and why ([`primase_shader::Refusal`]).
pub type Error = primase_shader::Refusal;
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn error(bit: usize, detail: impl Into<String>) -> Error {
    Error::new("DXIL", primase_shader::Unit::Bit, bit, detail)
}

/// The `DXIL` part of a DXBC container: the program header and its LLVM bitcode.
#[derive(Clone, Copy, Debug)]
pub struct Part<'a> {
    /// Shader kind in the high 16 bits (0 pixel), model major and minor in the low byte (`0x60` is 6.0).
    pub program_version: u32,
    /// DXIL version (`0x100` is 1.0).
    pub dxil_version: u32,
    pub bitcode: &'a [u8],
}

/// The parts of a DXBC container, an error of the container as one of the program (the bit it is at).
pub(crate) fn parts(container: &[u8]) -> Result<Vec<primase_dxcontainer::Chunk<'_>>> {
    primase_dxcontainer::chunks(container).map_err(|e| error(e.offset.saturating_mul(8), e.detail))
}

/// A program part's header (`DXIL` and the `STAT` reflection part share it) and its bitcode.
pub fn program(data: &[u8]) -> Result<Part<'_>> {
    let view = primase_bytes::View::new(data);
    let word = |at: usize| -> Result<u32> {
        view.array::<4>(at)
            .map(u32::from_le_bytes)
            .map_err(|e| error(at * 8, e.to_string()))
    };
    let program_version = word(0)?;
    let dwords = word(4)? as usize;
    if dwords.checked_mul(4) != Some(data.len()) {
        return Err(error(32, "program size disagrees with the part"));
    }
    if view.slice(8, 4).map_err(|e| error(64, e.to_string()))? != b"DXIL" {
        return Err(error(64, "program header magic"));
    }
    let dxil_version = word(12)?;
    // The bitcode offset counts from the magic.
    let offset = word(16)? as usize;
    let size = word(20)? as usize;
    let bitcode = offset
        .checked_add(8)
        .and_then(|start| view.slice(start, size).ok())
        .ok_or_else(|| error(128, "bitcode outside the part"))?;
    Ok(Part {
        program_version,
        dxil_version,
        bitcode,
    })
}

/// Find and check a container's `DXIL` part.
pub fn part(container: &[u8]) -> Result<Part<'_>> {
    let chunks = parts(container)?;
    let data = chunks
        .iter()
        .find(|c| &c.kind == b"DXIL")
        .ok_or_else(|| error(0, "no DXIL part"))?
        .data;
    program(data)
}

/// The shader kind (0 pixel, 1 vertex, 2 geometry, 3 hull, 4 domain, 5 compute) of a container and the `dx.op`
/// operations called across all the module's function bodies (a hull shader's patch-constant function is a second
/// one), by name: for surveying programs a stage's lifter does not exist for yet.
pub fn operations(container: &[u8]) -> Result<(u32, std::collections::BTreeMap<String, usize>)> {
    use module::Op;
    let chunks = parts(container)?;
    let part = program(
        chunks
            .iter()
            .find(|c| &c.kind == b"DXIL")
            .ok_or_else(|| error(0, "no DXIL part"))?
            .data,
    )?;
    let module = reflection::module(&chunks, b"DXIL")?.ok_or_else(|| error(0, "no DXIL part"))?;
    let mut out = std::collections::BTreeMap::new();
    for f in &module.functions {
        for inst in &f.instructions {
            let Op::Call { function, .. } = &inst.op else {
                continue;
            };
            if let Some(name) = module.name(*function).filter(|n| n.starts_with("dx.op.")) {
                let code = match &inst.op {
                    Op::Call { args, .. } => args.first().and_then(|&a| {
                        let value = if a < f.first {
                            module.values.get(a)
                        } else {
                            f.locals.get(a - f.first)
                        };
                        match value {
                            Some(module::Value::Constant(module::Constant::Integer(i))) => Some(*i),
                            _ => None,
                        }
                    }),
                    _ => None,
                };
                let key = match code {
                    Some(code) => format!("{name} (op {code})"),
                    None => name.to_owned(),
                };
                *out.entry(key).or_default() += 1;
            }
        }
    }
    Ok((part.program_version >> 16, out))
}
