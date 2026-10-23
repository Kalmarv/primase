//! Lifting a Shader Model 4 or 5 program (pixel, vertex or compute) to primase's shader representation
//! ([`primase_shader::Program`]).
//!
//! A DXBC register is four untyped 32-bit lanes, so the body keeps every register as a `vec4<u32>` local and each
//! instruction bitcasts its operands to the type the instruction implies; the typed values of the module are the
//! instruction results. Structured flow lifts to structured flow. The conventions are the shader design's:
//!
//! - **Varyings** are unpacked: one entry argument per input signature element, one result member per output element
//!   ([`primase_shader::entry_io`] says which), the D3D register packing undone by the prologue and epilogue and
//!   kept in the interface's provenance.
//! - **Resources** are module globals: constant buffer `cb<n>` as a uniform `array<vec4<u32>, rows>` (group 0, binding
//!   n), textures and buffer views `t<n>` (group 1), samplers `s<n>` (group 2), typed texture views `u<n>` (group 3, a storage
//!   image of four 32-bit lanes). A name and a layout come from the
//!   container's reflection where it has one; a stripped program's resources are named by register.
//! - **Behaviour** that differs from naga's or other APIs' is written out as operations
//!   ([`primase_shader::build`]'s D3D functions), and [`primase_shader::D3dModes`] says so: division by zero,
//!   float to integer conversion, `min`/`max` and `saturate` with NaN, shift counts, and reads out of range of a
//!   dynamically indexed buffer (zero).
//! - **Unknowns** are refused with the instruction word and the reason by name (`store_uav_typed: unordered-access
//!   views are not lifted`); nothing is approximated silently, and what is approximated is a note on the program.

mod body;
mod compute;
mod io;
mod operands;
mod resources;
mod setup;
mod tessellation;

use std::collections::{BTreeMap, BTreeSet};

use naga::{Handle, LocalVariable};
use primase_shader::build::Builder;
use primase_shader::{Conventions, D3dModes, Program, Provenance, ShaderFormat, Stage};

use crate::decode::{Decoded, decode};
use crate::{Result, chunks, error};

/// The version of the lifter: bumped when what it produces for the same bytecode changes, so a cache keyed by the
/// bytecode's hash knows what is stale; gated by `tests/versions.rs` (the lifted fixtures hashed under it, `primase_shader::gate`).
pub const LIFTER_VERSION: u32 = 2;

/// Lift a DXBC container's program. A pixel or vertex program of Shader Model 4 or 5 is the scope; everything else,
/// and every instruction or declaration the lifter does not carry, is an `Err` naming the word and the reason.
pub fn lift(bytes: &[u8]) -> Result<Program> {
    let mut program = lift_program(bytes)?;
    program.provenance.source_hash = Some(primase_shader::source_hash(bytes));
    Ok(program)
}

fn lift_program(bytes: &[u8]) -> Result<Program> {
    let directory = chunks(bytes)?;
    let code = directory
        .iter()
        .find(|c| matches!(&c.kind, b"SHDR" | b"SHEX"))
        .ok_or_else(|| error(0, "missing shader code chunk"))?;
    if code.data.len() % 4 != 0 {
        return Err(error(0, "unaligned code length"));
    }
    let words: Vec<u32> = code
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes(*b))
        .collect();
    if words.len() > 1_048_576 {
        return Err(error(1, "program exceeds word budget"));
    }
    let decoded = decode(&words)?;
    let stage = match decoded.kind {
        crate::decode::ProgramType::Pixel => Stage::Pixel,
        crate::decode::ProgramType::Vertex => Stage::Vertex,
        crate::decode::ProgramType::Compute => Stage::Compute,
        crate::decode::ProgramType::Hull => Stage::Hull,
        crate::decode::ProgramType::Geometry => Stage::Geometry,
        crate::decode::ProgramType::Domain => Stage::Domain,
    };
    let setup = setup::Setup::new(bytes, &decoded, stage)?;
    let mut lift = Lift::new(&decoded, stage, setup)?;
    if let Some(sigs) = lift.setup.sigs.clone() {
        for sig in &sigs {
            lift.run_function(sig)?;
        }
        return lift.finish_functions(decoded.target());
    }
    if stage == Stage::Compute {
        lift.b.workgroup_size = lift
            .setup
            .workgroup
            .ok_or_else(|| error(0, "a compute program that declares no thread group size"))?;
    }
    lift.prologue()?;
    lift.body()?;
    lift.finish(decoded.target())
}

/// A conditional's `if` or a `loop`, `switch` being lifted.
struct Frame {
    kind: FrameKind,
    condition: Option<Handle<naga::Expression>>,
    accept: Option<naga::Block>,
    selector: Option<Handle<naga::Expression>>,
    cases: Vec<naga::SwitchCase>,
    current: Option<naga::SwitchValue>,
}
#[derive(PartialEq, Eq)]
enum FrameKind {
    If,
    Loop,
    Switch,
}

/// The state of one lift.
struct Lift<'a> {
    b: Builder,
    decoded: &'a Decoded,
    stage: Stage,
    setup: setup::Setup,
    /// `r<n>`, `v<n>`, `o<n>`: four raw lanes each.
    temps: Vec<Handle<LocalVariable>>,
    inputs: BTreeMap<u32, Handle<LocalVariable>>,
    outputs: BTreeMap<u32, Handle<LocalVariable>>,
    /// `x<n>[i]`: the local and its length.
    indexable: BTreeMap<u32, (Handle<LocalVariable>, u32)>,
    /// `vThreadID`, `vThreadGroupID`, `vThreadIDInGroup` and `vThreadIDInGroupFlattened`, by the system value each is.
    ids: BTreeMap<primase_shader::SystemValue, Handle<LocalVariable>>,
    /// A hull, domain or position-less vertex program's function being lifted (see [`primase_shader::stage_functions`]).
    sig: Option<primase_shader::FunctionSig>,
    /// The control points of a hull or domain program: `v[point][register]` and a hull program's `vocp`, as arrays of
    /// registers, `registers` of them to a point.
    control_points: Option<(Handle<LocalVariable>, u32)>,
    output_points: Option<(Handle<LocalVariable>, u32)>,
    /// A domain program's patch constants `vpc[register]`, `vDomain`, and the phase numbers of a hull program.
    patch_in: Option<Handle<LocalVariable>>,
    domain_point: Option<Handle<LocalVariable>>,
    point_id: Option<Handle<LocalVariable>>,
    fork_id: Option<Handle<LocalVariable>>,
    join_id: Option<Handle<LocalVariable>>,
    /// A geometry program's emitted vertices so far (see [`tessellation::GsState`]) and its instance number.
    gs: Option<tessellation::GsState>,
    gs_instance: Option<Handle<LocalVariable>>,
    /// Lifting a phase of a hull program: a `ret` ends the phase, not the function.
    in_phase: bool,
    /// The declared arguments and result of the function being lifted, until it is closed.
    pending_decl: Option<primase_shader::build::FunctionDecl>,
    /// `vPrim`, `vCoverage`, `oDepth` and `oMask`, lane 0.
    primitive_id: Option<Handle<LocalVariable>>,
    input_coverage: Option<Handle<LocalVariable>>,
    depth: Option<Handle<LocalVariable>>,
    coverage: Option<Handle<LocalVariable>>,
    resources: resources::Resources,
    frames: Vec<Frame>,
    notes: BTreeSet<String>,
    /// The word of the instruction being lifted, for refusals.
    word: usize,
    /// A `ret` ended the program at the top level: what follows is unreachable.
    returned: bool,
    arguments: Vec<naga::FunctionArgument>,
    result_type: Option<Handle<naga::Type>>,
}

impl<'a> Lift<'a> {
    fn refuse<T>(&self, detail: impl Into<String>) -> Result<T> {
        Err(error(self.word, detail))
    }

    fn u4(&mut self) -> Handle<naga::Type> {
        self.b.ty_of(primase_shader::build::Kind::U, 4)
    }

    fn finish(mut self, target: String) -> Result<Program> {
        if !self.returned {
            self.epilogue()?;
        }
        self.b.check().map_err(|detail| error(self.word, detail))?;
        let stage = match self.stage {
            Stage::Vertex => naga::ShaderStage::Vertex,
            Stage::Compute => naga::ShaderStage::Compute,
            _ => naga::ShaderStage::Fragment,
        };
        let module = self.b.finish(
            "main",
            stage,
            self.arguments,
            self.result_type,
            self.setup.early_depth_test,
        );
        let mut limit_notes = primase_shader::compute_limit_notes(&module);
        let interface = self.setup.interface.clone();
        limit_notes.extend(primase_shader::interface_notes(&interface));
        Ok(Program {
            module,
            stage: self.stage,
            entry: "main".into(),
            interface: self.setup.interface,
            bindings: self.resources.bound,
            conventions: Conventions::D3D,
            modes: D3dModes::ALL,
            provenance: Provenance {
                format: ShaderFormat::Dxbc,
                target,
                lifter: "primase-dxbc".into(),
                lifter_version: LIFTER_VERSION,
                // the hash of the bytes, which `lift` sets
                source_hash: None,
            },
            notes: self.notes.into_iter().chain(limit_notes).collect(),
        })
    }
}
