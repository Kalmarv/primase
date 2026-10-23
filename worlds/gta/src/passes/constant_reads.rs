//! Which constants of its constant buffers a program reads, from the program lifted by [`primase_dxil::lift`].
//!
//! A program's buffers declare far more than it uses: GTA's engine buffers hold some 150 constants and a program reads a
//! handful. The lifter writes one load per row it touches and then takes lanes of it, and also takes lanes the program
//! never uses (the DXIL's `extractvalue`s of the load); naga's compaction drops the expressions no statement uses, so
//! what is left is what the program computes with. A row loaded and none of its lanes taken is used as a whole vector.
//! A buffer indexed by a value that is not a literal (a bone or object-matrix palette) is read at any row.
use std::collections::{BTreeMap, BTreeSet};

use naga::Expression;

/// The constant-buffer words a program reads, by the buffer's bind point (space, register).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConstantReads {
    /// Lanes taken of a loaded row: (space, register, row, lane).
    lanes: BTreeSet<(u32, u32, u32, u32)>,
    /// Rows loaded and used whole: (space, register, row).
    whole: BTreeSet<(u32, u32, u32)>,
    /// Buffers indexed by a value that is not a literal: (space, register).
    dynamic: BTreeSet<(u32, u32)>,
}

impl ConstantReads {
    /// The reads of a DXIL or DXBC container's program, or why it cannot be lifted.
    pub fn of(container: &[u8]) -> Result<ConstantReads, String> {
        let program = primase_dxil::lift(container).map_err(|e| e.to_string())?;
        Ok(ConstantReads::of_module(program.module))
    }

    /// The reads of the entry point of `module`, compacted first: the expressions no statement uses are not reads.
    pub fn of_module(mut module: naga::Module) -> ConstantReads {
        naga::compact::compact(&mut module, naga::compact::KeepUnused::Yes);
        let mut out = ConstantReads::default();
        let Some(entry) = module.entry_points.first() else {
            return out;
        };
        let function = &entry.function;
        let bind_point = |g: naga::Handle<naga::GlobalVariable>| {
            let b = module.global_variables.try_get(g).ok()?.binding?;
            Some((b.binding >> 16, b.binding & 0xffff))
        };
        // the rows loaded, by the load's handle
        let mut loads: BTreeMap<naga::Handle<Expression>, (u32, u32, u32)> = BTreeMap::new();
        for (handle, expression) in function.expressions.iter() {
            let Expression::Load { pointer } = expression else {
                continue;
            };
            let (base, row) = match function.expressions.try_get(*pointer) {
                Ok(Expression::AccessIndex { base, index }) => (*base, Some(*index)),
                Ok(Expression::Access { base, index }) => {
                    let literal = match function.expressions.try_get(*index) {
                        Ok(Expression::Literal(naga::Literal::U32(v))) => Some(*v),
                        Ok(Expression::Literal(naga::Literal::I32(v))) => u32::try_from(*v).ok(),
                        _ => None,
                    };
                    (*base, literal)
                }
                _ => continue,
            };
            let Ok(Expression::GlobalVariable(g)) = function.expressions.try_get(base) else {
                continue;
            };
            let Some((space, register)) = bind_point(*g) else {
                continue;
            };
            match row {
                Some(row) => {
                    loads.insert(handle, (space, register, row));
                }
                None => {
                    out.dynamic.insert((space, register));
                }
            }
        }
        let mut laned = BTreeSet::new();
        for (_, expression) in function.expressions.iter() {
            if let Expression::AccessIndex { base, index } = expression
                && let Some((space, register, row)) = loads.get(base)
            {
                out.lanes.insert((*space, *register, *row, *index));
                laned.insert(*base);
            }
        }
        for (handle, (space, register, row)) in loads {
            if !laned.contains(&handle) {
                out.whole.insert((space, register, row));
            }
        }
        out
    }

    /// Whether the program reads any of the `bytes` bytes at `offset` of the constant buffer at (`space`, `register`).
    pub fn reads(&self, space: u32, register: u32, offset: u32, bytes: u32) -> bool {
        if self.dynamic.contains(&(space, register)) {
            return true;
        }
        let end = offset.saturating_add(bytes.max(1));
        let mut at = offset - offset % 4;
        while at < end {
            let (row, lane) = (at / 16, at % 16 / 4);
            if self.whole.contains(&(space, register, row))
                || self.lanes.contains(&(space, register, row, lane))
            {
                return true;
            }
            at += 4;
        }
        false
    }

    /// The lanes taken of the row at (`space`, `register`): for the tests that hold this analysis to the emitted code.
    pub fn lanes_of(&self, space: u32, register: u32, row: u32) -> Vec<u32> {
        (0..4)
            .filter(|l| {
                self.whole.contains(&(space, register, row))
                    || self.lanes.contains(&(space, register, row, *l))
            })
            .collect()
    }
}
