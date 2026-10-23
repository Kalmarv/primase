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

mod flow;
mod memory;
mod ops;
mod resources;
mod setup;
mod values;

use std::collections::{BTreeMap, BTreeSet};

use naga::{Handle, LocalVariable};
use primase_shader::build::{Builder, Kind, Val};
use primase_shader::{Conventions, D3dModes, Program, Provenance, ShaderFormat, Stage};

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
    let setup = setup::Setup::new(&chunks, &module, entry)?;
    if part.program_version >> 16 == 1
        && !setup
            .interface
            .outputs
            .iter()
            .any(|v| v.system == Some(primase_shader::SystemValue::ClipPosition))
    {
        return Err(error(
            0,
            "a vertex program that writes no position (it feeds a tessellator, which a Program has no stage for)",
        ));
    }
    let target = format!(
        "{}_{}_{}",
        match part.program_version >> 16 {
            0 => "ps",
            _ => "vs",
        },
        (part.program_version >> 4) & 15,
        part.program_version & 15
    );
    let mut lift = Lift::new(&module, entry, setup)?;
    lift.run()?;
    lift.finish(target)
}

/// A translated LLVM value.
#[derive(Clone, Debug)]
pub(crate) enum Lv {
    Scalar(Val),
    /// A struct result (`ResRet`, a `dx.op` pair): its elements.
    Agg(Vec<Lv>),
    /// A resource handle: its class (0 shader resource, 1 unordered access, 2 constant buffer, 3 sampler) and the
    /// index of the interface resource it names.
    Handle {
        class: u8,
        resource: usize,
    },
    Pointer(memory::Ptr),
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
}

/// The state of one lift.
pub(crate) struct Lift<'m> {
    pub(crate) m: &'m Module,
    pub(crate) f: &'m Function,
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

    fn new(m: &'m Module, f: &'m Function, setup: setup::Setup) -> Result<Self> {
        let mut blocks = vec![Vec::new(); f.blocks];
        for (i, inst) in f.instructions.iter().enumerate() {
            blocks[inst.block].push(i);
        }
        let mut lift = Lift {
            m,
            f,
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
                Op::Return(None) | Op::Branch { .. } | Op::Unreachable | Op::Alloca { .. } => {}
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
            Some(Type::Float) => Ok(Kind::F),
            Some(Type::Integer(1)) => Ok(Kind::B),
            Some(Type::Integer(32)) => Ok(Kind::U),
            Some(Type::Integer(n)) => self.refuse(format!("{n}-bit integers are not lifted")),
            Some(Type::Half) => self.refuse("half-precision floats are not lifted"),
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

    fn finish(self, target: String) -> Result<Program> {
        let stage = self.setup.interface.stage;
        let early = false;
        let module = self.b.finish(
            "main",
            match stage {
                Stage::Vertex => naga::ShaderStage::Vertex,
                _ => naga::ShaderStage::Fragment,
            },
            self.arguments,
            self.result_type,
            early,
        );
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
            notes: self.notes.into_iter().collect(),
        })
    }
}

/// A refusal, for callers that count them by reason: the detail with its numbers removed.
pub fn refusal_reason(e: &Error) -> String {
    e.detail.chars().filter(|c| !c.is_ascii_digit()).collect()
}
