//! SDB2's stored shader records. Reading bytes is `read::shaders`; choosing permutations and bindings is a pass.

/// A source file with its original line endings, CRC-32 and 128-bit dependency identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub name: String,
    pub crc32: u32,
    pub dependency: [u32; 4],
    pub bytes: Vec<u8>,
}

/// A table key: kind 4 is a compiled effect, 5..7 utility stages, 8 a state-only pass.
/// The low six permutation bits select the template; the remaining bits are that template's defines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub kind: u32,
    pub permutation: u64,
    pub dependencies: [u32; 4],
}

/// A stored D3D9 state and its raw value (enum, bool, bit mask or float bits according to the state).
/// No destination render-state interpretation is made here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    pub kind: u32,
    pub value: u32,
}

/// A named sampler and its named texture parameter, with the packed native address and sampler states.
/// In the shipped PC databases the packed address is `0x00800000 | pixel_sampler_slot`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SamplerBinding {
    pub sampler: String,
    pub texture: String,
    pub address: u32,
    pub states: Vec<State>,
}

/// A compiled pass's header and all metadata following its bytecode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Effect {
    /// Raw SM3 vertex and pixel programs. A state-only record has neither.
    pub vertex: Option<Vec<u8>>,
    pub pixel: Option<Vec<u8>>,
    /// Native per-stage cache identities, not hashes synthesized by primase. Their hash algorithm is not inferred.
    pub vertex_key: u64,
    pub pixel_key: u64,
    /// Assignments made on pass entry.
    pub states: Vec<State>,
    /// States saved on entry and restored on exit. Values here are the stored initial placeholders, not assignments.
    pub restore_states: Vec<State>,
    pub samplers: Vec<SamplerBinding>,
    /// The header's second sampler-binding group, encoded identically. Empty in the shipped PC databases.
    pub additional_samplers: Vec<SamplerBinding>,
    /// Ordered names of effect variables that drive render state. Kept separate from literal assignments.
    pub variables: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Payload {
    /// A retained key with no compiled data. This is not a zero-byte shader.
    Missing,
    Effect(Effect),
    /// Raw utility stage (kinds 5..7). Stage meaning follows the token version, not a guess from its role.
    Program {
        stage: primase_core::shader::Stage,
        bytes: Vec<u8>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub key: Key,
    /// File offset, including zero-length entries' positions.
    pub offset: u32,
    pub bytes: u32,
    pub payload: Payload,
}

/// An uncompressed version-39 SDB2, in source/table order. No quality level is chosen by the reader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Database {
    pub sources: Vec<Source>,
    pub records: Vec<Record>,
    /// The zeroed reserve between the active record table and the first payload, in bytes.
    pub table_padding: usize,
}

/// Named graph derivation from the stored forward Colour pixel program.
pub mod albedo;

mod material;
pub use material::bind_material;

pub(crate) use material::bind_stage;
