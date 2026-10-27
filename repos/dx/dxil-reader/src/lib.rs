//! A reader of Shader Model 6 programs (DXIL) as Direct3D stores them in a `DXBC` container: the program part's header
//! ([`program`]), its LLVM bitcode through a bounded bitstream reader ([`bitstream`]) into the module it holds
//! ([`module`]), the entry point's metadata, resource records and signature parts ([`reflection`]), and a library's
//! runtime data ([`rdat`]), each as the records the file stores with DXIL's and D3D's own numbers.
//!
//! It reads and checks bytes and never interprets them for an API: a resource is its record's class, range and shape
//! numbers, a signature element its stored name, index, register and masks. Mapping those into a graphics API's
//! vocabulary is a consumer's choice. Everything it can't read (a truncated record, an abbreviation it does not know, a
//! reference outside its table) is an [`Error`] naming the bit where it happened; it never panics on untrusted input.
//!
//! ```no_run
//! # fn main() -> Result<(), dxil_reader::Error> {
//! let container = std::fs::read("shader.dxil").unwrap();
//! let parts = dxil_reader::parts(&container)?;
//! let module = dxil_reader::module(&parts, b"DXIL")?.expect("a DXIL part");
//! println!("{} functions", module.functions.len());
//! # Ok(()) }
//! ```
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]
pub mod bitstream;
pub mod module;
pub mod rdat;
pub mod reflection;

pub use module::Module;

/// Something the reader can't read: the bit where it is (in the bitcode, or eight times a byte of the container or
/// part being read), and what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// The bit offset, counted from the start of the bitcode, part or container being read.
    pub bit: usize,
    pub detail: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DXIL at bit {}: {}", self.bit, self.detail)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn error(bit: usize, detail: impl Into<String>) -> Error {
    Error {
        bit,
        detail: detail.into(),
    }
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

/// The parts of a DXBC container (its directory), an error of the container at the bit of its byte.
pub fn parts(container: &[u8]) -> Result<Vec<dxcontainer::Chunk<'_>>> {
    dxcontainer::chunks(container).map_err(|e| error(e.offset.saturating_mul(8), e.detail))
}

/// A program part's header (`DXIL` and the `STAT` reflection part share it) and its bitcode.
pub fn program(data: &[u8]) -> Result<Part<'_>> {
    let view = primase_foundations::bytes::View::new(data);
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

/// The module of a container's program part of this kind (`DXIL`, or `STAT` for the reflection module), if it has one.
pub fn module(chunks: &[dxcontainer::Chunk<'_>], kind: &[u8; 4]) -> Result<Option<Module>> {
    let Some(c) = chunks.iter().find(|c| &c.kind == kind) else {
        return Ok(None);
    };
    let part = program(c.data)?;
    let blocks = bitstream::read(part.bitcode, *b"BC\xc0\xde")?;
    Module::read(&blocks).map(Some)
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
    let module = module(&chunks, b"DXIL")?.ok_or_else(|| error(0, "no DXIL part"))?;
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
