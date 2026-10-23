//! What a lift needs before it writes any code: the interface (the signature elements and resources the program
//! uses), how the entry point carries it, and the program's own instruction index.

use std::collections::{BTreeMap, BTreeSet};

use primase_shader::{EntryIo, Interface, Resource, ResourceId, ResourceKind, Stage, SystemValue, entry_io};

use crate::module::{Constant, Function, Module, Op, Value, ValueId};
use crate::reflection::{self, Element, Range};
use crate::{Chunk, Error, Result, error};

/// The constant integer a value is, if it is one.
pub(super) fn const_int(m: &Module, f: &Function, v: ValueId) -> Option<i64> {
    let value = if v < f.first {
        m.values.get(v)?
    } else {
        f.locals.get(v - f.first)?
    };
    match value {
        Value::Constant(Constant::Integer(i)) => Some(*i),
        Value::Constant(Constant::Null) => Some(0),
        _ => None,
    }
}

/// The range a `CreateHandleFromBinding` names: its class, lower bound and register space (the binding constant
/// `{ lower, upper, space, class }`), and the offset into the range (the call's index is the register itself).
pub(super) fn binding_of(m: &Module, f: &Function, bind: ValueId) -> Option<(u8, u32, u32)> {
    let value = if bind < f.first {
        m.values.get(bind)?
    } else {
        f.locals.get(bind - f.first)?
    };
    // a zero binding (register 0 of space 0, a shader resource) is stored as the null constant
    let field = |k: usize| match value {
        Value::Constant(Constant::Null) => Some(0),
        Value::Constant(Constant::Aggregate(items)) => const_int(m, f, *items.get(k)?),
        _ => None,
    };
    Some((
        u8::try_from(field(3)?).ok()?,
        u32::try_from(field(0)?).ok()?,
        u32::try_from(field(2)?).ok()?,
    ))
}

/// The `dx.op` opcode a call makes: the callee is a `dx.op.*` declaration and the first argument a constant.
pub(super) fn dx_opcode(m: &Module, f: &Function, function: ValueId, args: &[ValueId]) -> Option<u64> {
    if !m.name(function)?.starts_with("dx.op.") {
        return None;
    }
    u64::try_from(const_int(m, f, *args.first()?)?).ok()
}

/// The resource ranges of the program, in the interface's order, and which of them the program uses.
pub(crate) struct Setup {
    pub interface: Interface,
    pub io: EntryIo,
    pub input_elements: Vec<Element>,
    pub output_elements: Vec<Element>,
    /// The kept resources' ranges, parallel to `interface.resources`.
    pub ranges: Vec<Range>,
    pub notes: BTreeSet<String>,
}

impl Setup {
    pub(super) fn new(chunks: &[Chunk<'_>], m: &Module, entry: &Function) -> Result<Self> {
        let named = reflection::module(chunks, b"STAT")?;
        let mut interface = reflection::reflection(chunks, m, named.as_ref())?;
        if !matches!(interface.stage, Stage::Pixel | Stage::Vertex) {
            return Err(error(0, "only pixel and vertex programs are lifted"));
        }
        let (input_elements, output_elements) = reflection::elements(m)?;
        let program_ranges: Vec<Range> = reflection::ranges(m)?.into_iter().map(|r| r.0).collect();
        if program_ranges.len() != interface.resources.len() {
            return Err(error(
                0,
                "the reflection module's resources differ from the program's",
            ));
        }
        // which cells of the signatures and which ranges the instructions use
        let mut cells_in: BTreeSet<(u32, u8)> = BTreeSet::new();
        let mut cells_out: BTreeSet<(u32, u8)> = BTreeSet::new();
        let mut system_out: BTreeSet<u8> = BTreeSet::new();
        let mut used_ranges: BTreeSet<(u8, u32)> = BTreeSet::new();
        let mut used_bindings: BTreeSet<(u8, u32, u32)> = BTreeSet::new();
        for inst in &entry.instructions {
            let Op::Call { function, args } = &inst.op else {
                continue;
            };
            let Some(code) = dx_opcode(m, entry, *function, args) else {
                continue;
            };
            let konst = |n: usize| args.get(n).and_then(|&a| const_int(m, entry, a));
            match code {
                4 | 5 => {
                    let output = code == 5;
                    let list = if output { &output_elements } else { &input_elements };
                    let Some(id) = konst(1) else {
                        return Err(error(inst.at, "a signature element named by a computed id"));
                    };
                    let e = list
                        .iter()
                        .find(|e| i64::from(e.id) == id)
                        .ok_or_else(|| error(inst.at, format!("signature element {id}")))?;
                    if !e.placed {
                        if output {
                            system_out.insert(e.system_value);
                            continue;
                        }
                        return Err(error(
                            inst.at,
                            format!("input element {} is not placed in a register", e.name),
                        ));
                    }
                    let (rows, columns) = match (konst(2), konst(3)) {
                        (Some(r), Some(c)) => (r..r + 1, c..c + 1),
                        (Some(r), None) => (r..r + 1, 0..i64::from(e.columns)),
                        (None, Some(c)) => (0..i64::from(e.rows), c..c + 1),
                        (None, None) => (0..i64::from(e.rows), 0..i64::from(e.columns)),
                    };
                    for r in rows {
                        for c in columns.clone() {
                            let cell = (e.row + r as u32, e.column + c as u8);
                            if output {
                                cells_out.insert(cell);
                            } else {
                                cells_in.insert(cell);
                            }
                        }
                    }
                }
                57 => {
                    if let (Some(class), Some(id)) = (konst(1), konst(2)) {
                        used_ranges.insert((class as u8, id as u32));
                    } else {
                        return Err(error(inst.at, "a handle of a computed range"));
                    }
                }
                217 => {
                    let Some((class, lower, space)) = binding_of(m, entry, args[1]) else {
                        return Err(error(inst.at, "a handle of a binding that is not constant"));
                    };
                    used_bindings.insert((class, lower, space));
                }
                _ => {}
            }
        }
        // the signature varyings that cover a used cell, and the system outputs the program writes
        let keeps = |v: &primase_shader::Varying, cells: &BTreeSet<(u32, u8)>, system: &BTreeSet<u8>| {
            match v.provenance.d3d {
                Some(d3d) if d3d.register != u32::MAX => {
                    (0..4u8).any(|lane| d3d.mask >> lane & 1 != 0 && cells.contains(&(d3d.register, lane)))
                }
                _ => match v.system {
                    // DXIL semantic kinds: 17 depth, 18 depth less-equal, 19 depth greater-equal, 14 coverage
                    Some(SystemValue::Depth) => system.contains(&17),
                    Some(SystemValue::DepthLessEqual) => system.contains(&18),
                    Some(SystemValue::DepthGreaterEqual) => system.contains(&19),
                    Some(SystemValue::Coverage) => system.contains(&14),
                    _ => false,
                },
            }
        };
        let none = BTreeSet::new();
        interface.inputs.retain(|v| keeps(v, &cells_in, &none));
        interface.outputs.retain(|v| keeps(v, &cells_out, &system_out));
        // the resources the program creates handles for, numbered anew
        let mut resources = Vec::new();
        let mut ranges = Vec::new();
        for (original, range) in interface.resources.drain(..).zip(program_ranges) {
            if !used_ranges.contains(&(range.class, range.id))
                && !used_bindings.contains(&(range.class, range.lower, range.space))
            {
                continue;
            }
            if original.is_array()
                && !matches!(
                    original.kind,
                    ResourceKind::Texture { .. } | ResourceKind::Sampler { .. }
                )
            {
                return Err(error(
                    0,
                    format!(
                        "{}: arrays of resources other than textures and samplers (a range of {}) are not lifted",
                        original.provenance.name, original.count
                    ),
                ));
            }
            resources.push(Resource {
                id: ResourceId(resources.len() as u32),
                ..original
            });
            ranges.push(range);
        }
        interface.resources = resources;
        let io = entry_io(&interface).map_err(|detail| error(0, detail))?;
        Ok(Setup {
            interface,
            io,
            input_elements,
            output_elements,
            ranges,
            notes: BTreeSet::new(),
        })
    }

    /// The input varying whose register and lanes hold this cell, and the lane's position in its value.
    pub(super) fn input_cell(&self, register: u32, lane: u8) -> Option<(usize, u8)> {
        cell(&self.interface.inputs, register, lane)
    }
    pub(super) fn output_cell(&self, register: u32, lane: u8) -> Option<(usize, u8)> {
        cell(&self.interface.outputs, register, lane)
    }

    /// The kept resource of a binding's class, lower bound and register space.
    pub(super) fn resource_by_binding(&self, class: u8, lower: u32, space: u32) -> Option<usize> {
        self.ranges
            .iter()
            .position(|r| r.class == class && r.lower == lower && r.space == space)
    }

    /// The kept resource of a handle's class and range id.
    pub(super) fn resource(&self, class: u8, id: u32) -> Option<usize> {
        self.ranges.iter().position(|r| r.class == class && r.id == id)
    }
    pub(super) fn kind(&self, resource: usize) -> &ResourceKind {
        &self.interface.resources[resource].kind
    }
}

fn cell(list: &[primase_shader::Varying], register: u32, lane: u8) -> Option<(usize, u8)> {
    list.iter().enumerate().find_map(|(i, v)| {
        let d3d = v.provenance.d3d?;
        (d3d.register == register && d3d.mask >> lane & 1 != 0)
            .then(|| (i, (d3d.mask & ((1u8 << lane) - 1)).count_ones() as u8))
    })
}

pub(super) fn _unused(_: BTreeMap<u8, u8>, _: Error) {}
