//! The shader program: what every bytecode lifts into, in primase's own vocabulary, and what is done with it short of running it.
//!
//! A [`Program`] is a `naga::Module` with the [`Interface`] a caller binds inputs, constants and textures by and what the module
//! cannot say; [`Program::validate`] checks it, [`emit`] writes it as WGSL, [`build::Builder`] is the typed builder the lifters
//! (`iridia-dxbc`, `iridia-dxil`, `iridia-mojo`) share, [`gate`] is their version gate and [`flow`] says what a
//! program's outputs are made of, [`matrix`] how it holds a matrix constant, [`output_uses`] what its discards and blend
//! show an unroled output lane is used for, [`output_stores`] what its entry point stores into each output lane and
//! how a stored value is made, [`surface`] the reading of a program's outputs as a surface packed for a GPU composite,
//! with the WGSL that decodes it. The vocabulary is
//! [`iridia_interface`], re-exported here: varyings by label, resources with explicit constant layouts, D3D's
//! registers kept in provenance. The [`d3d`] module is where D3D's enum numbers and
//! packing rules are mapped into it, for the readers of every bytecode.
//!
//! Running a program is `iridia-exec`, which depends on this crate.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod bound;
pub mod build;
pub mod d3d;
pub mod emit;
pub mod flow;
pub mod gate;
mod limits;
pub mod matrix;
pub mod output_stores;
pub mod output_uses;
mod program;
mod reads;
pub mod rt;
pub mod surface;
pub use iridia_interface::{
    Access, BufferKind, BufferLayout, Constant, ConstantType, D3dBinding, D3dClass, D3dSignature,
    DomainFacts, GeometryFacts, GsInput, GsOutput, HitGroup, HitGroupKind, HullFacts, InstructionCount,
    InstructionUnit, Interface, InterpMode, InterpSampling, Interpolation, Label, Majorness, Member,
    MeshFacts, MeshTopology, Partitioning, RayTracingFacts, Resource, ResourceId, ResourceKind,
    ResourceProvenance, SampleType, ScalarType, ShaderFormat, Stage, StageFacts, StorageTarget, Subobject,
    SystemValue, TessDomain, TessTopology, TextureDim, Value, Varying, VaryingProvenance,
};
mod refusal;
pub use bound::BoundSizes;
pub use limits::{WEBGPU_BASELINE, compute_limit_notes, interface_notes};
pub use program::{
    ClipDepth, Conventions, D3dModes, EntryIo, Error as ProgramError, FnItem, FunctionSig, IoItem, IoType,
    ItemList, Origin, Problem, Program, Provenance, VERSION as PROGRAM_VERSION, entry_io, mesh_vertex_io,
    stage_functions,
};
pub use reads::{ProgramGraph, WordsRead};
pub use refusal::{Refusal, Unit, source_hash};
