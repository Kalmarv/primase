//! Lifting a Shader Model 6 pixel or vertex program (DXIL) to primase's shader representation
//! ([`primase_shader::Program`]), with the same conventions as the DXBC lift (`primase-dxbc`): varyings unpacked from
//! D3D registers, resources as module globals (constant buffer `cb<n>` a uniform `array<vec4<u32>, rows>` in group 0,
//! textures and buffer views group 1, samplers group 2, typed texture views group 3, binding = register), D3D's
//! results written out as operations ([`primase_shader::D3dModes`]).
//!
//! What differs is the source. A DXIL function is LLVM's: values in static single assignment form over basic blocks.
//! Basic blocks are structured by [`primase_shader::build::structure`] (a reducible graph; irreducible flow is refused
//! by name); a phi is a local the edges assign; a value used outside the block that defines it is a local too, the
//! rest stay expressions. `dx.op` calls are the intrinsics (inputs and outputs, math, bit operations, constant
//! buffers, samples, loads, derivatives, discard), and `alloca`, `getelementptr`, `load` and `store` the local arrays
//! and constant tables.

mod compute;
mod flow;
mod memory;
mod ops;
mod rayquery;
mod resources;
mod setup;
mod stages;
mod values;

use std::collections::{BTreeMap, BTreeSet};

use naga::{Handle, LocalVariable};
use primase_shader::build::{Builder, Kind, Val};
use primase_shader::{Conventions, D3dModes, Program, Provenance, ShaderFormat, Stage, StageFacts};

use crate::module::{Function, Module, Op, Type, TypeId, ValueId};
use crate::{Error, Result, error};

/// The version of the lifter: bumped when what it produces for the same bytecode changes.
pub const LIFTER_VERSION: u32 = 1;

/// Lift a DXIL container's entry function. A pixel or vertex program is the scope; every construct the lifter does not
/// carry (a wide integer, a resource array, an irreducible loop, an operation it has no meaning for) is an `Err` naming
/// the bit offset of the instruction and the reason.
pub fn lift(container: &[u8]) -> Result<Program> {
    let chunks = crate::chunks(container)?;
    let module = crate::reflection::module(&chunks, b"DXIL")?.ok_or_else(|| error(0, "no DXIL part"))?;
    let part = crate::program(
        chunks
            .iter()
            .find(|c| &c.kind == b"DXIL")
            .ok_or_else(|| error(0, "no DXIL part"))?
            .data,
    )?;
    let entry_value = crate::reflection::entry(&module)?;
    let entry = module
        .functions
        .iter()
        .find(|f| f.value == entry_value)
        .ok_or_else(|| error(0, "the entry point has no body"))?;
    // a hull program is two functions: the control-point phase (the entry) and the patch-constant function
    let patch_constant = if part.program_version >> 16 == 3 {
        let state = crate::reflection::hull_state(&module)?;
        Some(
            module
                .functions
                .iter()
                .find(|f| f.value == state.patch_constant)
                .ok_or_else(|| error(0, "the patch-constant function has no body"))?,
        )
    } else {
        None
    };
    let mut functions = vec![entry];
    functions.extend(patch_constant);
    let setup = setup::Setup::new(&chunks, &module, &functions)?;
    let target = format!(
        "{}_{}_{}",
        match part.program_version >> 16 {
            0 => "ps",
            1 => "vs",
            3 => "hs",
            4 => "ds",
            _ => "cs",
        },
        (part.program_version >> 4) & 15,
        part.program_version & 15
    );
    let mut lift = Lift::new(&module, entry, functions, setup)?;
    if lift.setup.interface.stage == Stage::Compute {
        lift.b.workgroup_size = crate::reflection::num_threads(&module)?
            .ok_or_else(|| error(0, "a compute program that states no thread group size"))?;
    }
    match lift.setup.sigs.clone() {
        None => {
            lift.run()?;
            lift.finish(target)
        }
        Some(sigs) => {
            for sig in &sigs {
                let f = if sig.name == "patch_constant" {
                    patch_constant
                        .ok_or_else(|| error(0, "a patch-constant function in a program that has none"))?
                } else {
                    entry
                };
                lift.run_function(f, sig)?;
            }
            lift.finish_functions(target)
        }
    }
}

/// A translated LLVM value.
#[derive(Clone, Debug)]
pub(crate) enum Lv {
    Scalar(Val),
    /// A struct result (`ResRet`, a `dx.op` pair): its elements.
    Agg(Vec<Lv>),
    /// A resource handle: its class (0 shader resource, 1 unordered access, 2 constant buffer, 3 sampler), the index
    /// of the interface resource it names and, for a resource array, the element (from the array's first) the
    /// program picked.
    Handle {
        class: u8,
        resource: usize,
        element: Option<Val>,
    },
    Pointer(memory::Ptr),
    /// An inline ray tracing query (see [`rayquery`]).
    RayQuery(Handle<LocalVariable>, u32),
}

/// Where a value lives once defined: as the expressions of its block, or in locals when another block reads it.
#[derive(Clone, Debug)]
enum Slot {
    Value(Lv),
    Stored(Stored),
}

#[derive(Clone, Debug)]
enum Stored {
    Scalar {
        local: Handle<LocalVariable>,
        kind: Kind,
        width: u8,
    },
    Agg(Vec<Stored>),
    /// Not stored: handles and the like, which are the same wherever they are read.
    Plain(Lv),
    /// A handle to an element of a resource array: the element index is a value, kept in a local.
    Handle {
        class: u8,
        resource: usize,
        element: Box<Stored>,
    },
}

/// The state of one lift.
pub(crate) struct Lift<'m> {
    pub(crate) m: &'m Module,
    pub(crate) f: &'m Function,
    /// Every function whose instructions the lift reads: the entry's, and a hull program's patch-constant function.
    functions: Vec<&'m Function>,
    /// The function of a program of a stage with no entry point being lifted.
    sig: Option<primase_shader::FunctionSig>,
    pub(crate) b: Builder,
    pub(super) setup: setup::Setup,
    /// The slot of each local value (arguments and instruction results), by `v - f.first`.
    slots: Vec<Option<Slot>>,
    /// Whether a value is read outside the block that defines it.
    cross: Vec<bool>,
    /// The instructions of each basic block, by index into `f.instructions`.
    blocks: Vec<Vec<usize>>,
    /// Output registers, four raw lanes each.
    outputs: BTreeMap<u32, Handle<LocalVariable>>,
    depth: Option<Handle<LocalVariable>>,
    coverage: Option<Handle<LocalVariable>>,
    resources: resources::Resources,
    memory: memory::Memory,
    arguments: Vec<naga::FunctionArgument>,
    /// The expression of each entry argument, made once.
    argument_exprs: BTreeMap<usize, naga::Handle<naga::Expression>>,
    result_type: Option<Handle<naga::Type>>,
    notes: BTreeSet<String>,
    /// The bit offset of the instruction being lifted, for refusals.
    at: usize,
}

impl<'m> Lift<'m> {
    pub(crate) fn refuse<T>(&self, detail: impl Into<String>) -> Result<T> {
        Err(error(self.at, detail))
    }

    fn new(
        m: &'m Module,
        f: &'m Function,
        functions: Vec<&'m Function>,
        setup: setup::Setup,
    ) -> Result<Self> {
        let mut blocks = vec![Vec::new(); f.blocks];
        for (i, inst) in f.instructions.iter().enumerate() {
            blocks[inst.block].push(i);
        }
        let mut lift = Lift {
            m,
            f,
            functions,
            sig: None,
            b: Builder::new(),
            notes: setup.notes.clone(),
            setup,
            slots: vec![None; f.locals.len()],
            cross: vec![false; f.locals.len()],
            blocks,
            outputs: BTreeMap::new(),
            depth: None,
            coverage: None,
            resources: resources::Resources::default(),
            memory: memory::Memory::default(),
            arguments: Vec::new(),
            argument_exprs: BTreeMap::new(),
            result_type: None,
            at: 0,
        };
        lift.analyse_uses();
        Ok(lift)
    }

    /// Mark the values read outside the block that defines them. A phi's value is always a local; a phi's incoming
    /// value is read at the end of its predecessor.
    fn analyse_uses(&mut self) {
        let f = self.f;
        let mut def_block: BTreeMap<ValueId, usize> = BTreeMap::new();
        for inst in &f.instructions {
            if let Some((v, _)) = inst.result {
                def_block.insert(v, inst.block);
            }
        }
        let mark = |v: ValueId, at: usize, cross: &mut Vec<bool>| {
            if let Some(&d) = def_block.get(&v)
                && d != at
            {
                cross[v - f.first] = true;
            }
        };
        let mut cross = vec![false; f.locals.len()];
        for inst in &f.instructions {
            let at = inst.block;
            match &inst.op {
                Op::Binary { lhs, rhs, .. } | Op::Compare { lhs, rhs, .. } => {
                    mark(*lhs, at, &mut cross);
                    mark(*rhs, at, &mut cross);
                }
                Op::Cast { value, .. } => mark(*value, at, &mut cross),
                Op::Select {
                    condition,
                    then,
                    otherwise,
                } => {
                    for v in [condition, then, otherwise] {
                        mark(*v, at, &mut cross);
                    }
                }
                Op::ExtractValue { aggregate, .. } => mark(*aggregate, at, &mut cross),
                Op::InsertValue { aggregate, value, .. } => {
                    mark(*aggregate, at, &mut cross);
                    mark(*value, at, &mut cross);
                }
                Op::Call { args, .. } => {
                    for a in args {
                        mark(*a, at, &mut cross);
                    }
                }
                Op::Phi { incoming } => {
                    if let Some((v, _)) = inst.result {
                        cross[v - f.first] = true;
                    }
                    for (v, pred) in incoming {
                        mark(*v, *pred, &mut cross);
                    }
                }
                Op::Return(Some(v)) => mark(*v, at, &mut cross),
                Op::CondBranch { condition, .. } => mark(*condition, at, &mut cross),
                Op::Switch { value, .. } => mark(*value, at, &mut cross),
                Op::Load { pointer } => mark(*pointer, at, &mut cross),
                Op::Store { pointer, value } => {
                    mark(*pointer, at, &mut cross);
                    mark(*value, at, &mut cross);
                }
                Op::Gep { base, indices } => {
                    mark(*base, at, &mut cross);
                    for i in indices {
                        mark(*i, at, &mut cross);
                    }
                }
                Op::AtomicRmw { pointer, value, .. } => {
                    mark(*pointer, at, &mut cross);
                    mark(*value, at, &mut cross);
                }
                Op::CmpXchg {
                    pointer,
                    compare,
                    value,
                } => {
                    for v in [pointer, compare, value] {
                        mark(*v, at, &mut cross);
                    }
                }
                Op::Return(None) | Op::Branch { .. } | Op::Unreachable | Op::Alloca { .. } | Op::Fence => {}
            }
        }
        // a pointer is symbolic and is read wherever it is used: its index values are read there too
        let mut changed = true;
        while changed {
            changed = false;
            for inst in &f.instructions {
                if let (Op::Gep { indices, .. }, Some((p, _))) = (&inst.op, inst.result)
                    && cross[p - f.first]
                {
                    for i in indices {
                        if *i >= f.first && !cross[*i - f.first] {
                            cross[*i - f.first] = true;
                            changed = true;
                        }
                    }
                }
            }
        }
        self.cross = cross;
    }

    pub(crate) fn kind_of_type(&self, ty: TypeId) -> Result<Kind> {
        match self.m.types.get(ty) {
            // a half is computed as a float and rounded to a half where it is made (see `Lift::quantized`)
            Some(Type::Float | Type::Half) => Ok(Kind::F),
            Some(Type::Integer(1)) => Ok(Kind::B),
            // a 16-bit integer is carried as the 32-bit unsigned integer of its bits, zero-extended
            Some(Type::Integer(16 | 32)) => Ok(Kind::U),
            Some(Type::Integer(n)) => self.refuse(format!("{n}-bit integers are not lifted")),
            Some(Type::Double) => self.refuse("double-precision floats are not lifted"),
            other => self.refuse(format!("a value of type {other:?}")),
        }
    }

    fn run(&mut self) -> Result<()> {
        let io = self.setup.io.clone();
        let declared = self.b.declare_entry(&io).map_err(|detail| error(0, detail))?;
        self.arguments = declared.arguments;
        self.result_type = declared.result;
        let u4 = self.b.ty_of(Kind::U, 4);
        for v in &self.setup.interface.outputs {
            if let Some(d3d) = v.provenance.d3d
                && d3d.register != u32::MAX
                && !self.outputs.contains_key(&d3d.register)
            {
                let local = self.b.local(&format!("o{}", d3d.register), u4);
                self.outputs.insert(d3d.register, local);
            }
            match v.system {
                Some(
                    primase_shader::SystemValue::Depth
                    | primase_shader::SystemValue::DepthGreaterEqual
                    | primase_shader::SystemValue::DepthLessEqual,
                ) if self.depth.is_none() => self.depth = Some(self.b.local("oDepth", u4)),
                Some(primase_shader::SystemValue::Coverage) if self.coverage.is_none() => {
                    self.coverage = Some(self.b.local("oMask", u4));
                }
                _ => {}
            }
        }
        self.declare_resources()?;
        self.declare_memory()?;
        self.define_phis()?;
        self.structure_and_lower()
    }

    /// Lift one function of a program of a stage with no entry point as the module's function `sig`.
    fn run_function(&mut self, f: &'m Function, sig: &primase_shader::FunctionSig) -> Result<()> {
        // the per-function state: values, blocks, output registers, the argument expressions
        self.f = f;
        let mut blocks = vec![Vec::new(); f.blocks];
        for (i, inst) in f.instructions.iter().enumerate() {
            blocks[inst.block].push(i);
        }
        self.blocks = blocks;
        self.slots = vec![None; f.locals.len()];
        self.cross = vec![false; f.locals.len()];
        self.outputs.clear();
        self.depth = None;
        self.coverage = None;
        self.argument_exprs.clear();
        self.analyse_uses();
        self.sig = Some(sig.clone());
        let decl = self.b.declare_function(sig).map_err(|detail| error(0, detail))?;
        // the registers its results are made of
        let u4 = self.b.ty_of(Kind::U, 4);
        for v in self.result_varyings() {
            if let Some(d3d) = v.provenance.d3d
                && d3d.register != u32::MAX
                && !self.outputs.contains_key(&d3d.register)
            {
                let local = self.b.local(&format!("o{}", d3d.register), u4);
                self.outputs.insert(d3d.register, local);
            }
        }
        self.result_type = decl.result;
        if self.resources.globals_made {
            // the globals are the module's: made with the first function
        } else {
            self.declare_resources()?;
            self.declare_memory()?;
            self.resources.globals_made = true;
        }
        self.define_phis()?;
        self.structure_and_lower()?;
        self.b.end_function(sig.name, decl);
        Ok(())
    }

    /// The varyings the current function's result is made of: the outputs, or a hull program's patch constants.
    pub(super) fn result_varyings(&self) -> Vec<primase_shader::Varying> {
        match (&self.sig, &self.setup.interface.facts) {
            (Some(sig), StageFacts::Hull(h)) if sig.name == "patch_constant" => h.patch_constants.clone(),
            _ => self.setup.interface.outputs.clone(),
        }
    }

    fn finish_functions(self, target: String) -> Result<Program> {
        let stage = self.setup.interface.stage;
        let module = self.b.finish_functions();
        let mut limit_notes = primase_shader::compute_limit_notes(&module);
        let interface = self.setup.interface.clone();
        limit_notes.extend(primase_shader::interface_notes(&interface));
        Ok(Program {
            module,
            stage,
            entry: "main".into(),
            interface: self.setup.interface,
            bindings: self.resources.bound,
            conventions: Conventions::D3D,
            modes: D3dModes::ALL,
            provenance: Provenance {
                format: ShaderFormat::Dxil,
                target,
                lifter: "primase-dxil".into(),
                lifter_version: LIFTER_VERSION,
                source_hash: None,
            },
            notes: self.notes.into_iter().chain(limit_notes).collect(),
        })
    }

    fn finish(self, target: String) -> Result<Program> {
        let stage = self.setup.interface.stage;
        let early = false;
        let module = self.b.finish(
            "main",
            match stage {
                Stage::Vertex => naga::ShaderStage::Vertex,
                Stage::Compute => naga::ShaderStage::Compute,
                _ => naga::ShaderStage::Fragment,
            },
            self.arguments,
            self.result_type,
            early,
        );
        let mut limit_notes = primase_shader::compute_limit_notes(&module);
        let interface = self.setup.interface.clone();
        limit_notes.extend(primase_shader::interface_notes(&interface));
        Ok(Program {
            module,
            stage,
            entry: "main".into(),
            interface: self.setup.interface,
            bindings: self.resources.bound,
            conventions: Conventions::D3D,
            modes: D3dModes::ALL,
            provenance: Provenance {
                format: ShaderFormat::Dxil,
                target,
                lifter: "primase-dxil".into(),
                lifter_version: LIFTER_VERSION,
                source_hash: None,
            },
            notes: self.notes.into_iter().chain(limit_notes).collect(),
        })
    }
}

/// A refusal, for callers that count them by reason: the detail with its numbers removed.
pub fn refusal_reason(e: &Error) -> String {
    e.detail.chars().filter(|c| !c.is_ascii_digit()).collect()
}
