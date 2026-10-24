//! Which constants of its constant buffers a program reads.
//!
//! [`WordsRead`] is the answer from the value graph ([`crate::flow::Values`]): the words its outputs, discards and
//! writes are made of, by buffer. It is word-granular however the program loads (a lifted DXIL program loads a whole
//! register and takes lanes afterwards, so a load says nothing of which lanes count). [`Program::constant_reads`] is it
//! for a program.
//!
//! [`ConstantReads`] is the older answer from the loads of the compacted module, row and lane, by the bind point the
//! caller derives from the lifter's binding: still used by the worlds that take it, until each moves to [`WordsRead`].
//! A program's buffers declare far more than it uses: an engine's per-frame buffers hold some hundred vectors and a
//! program reads a handful. A lifter writes one load per row it touches and then takes lanes of it, and also takes lanes
//! the program never uses; naga's compaction drops the expressions no statement uses, so what is left is what the program
//! computes with. A row loaded and none of its lanes taken is used as a whole vector. A buffer indexed by a value that is
//! not a literal (a bone or object-matrix palette) is read at any row.
//!
//! A buffer is named by the pair its bytecode binds it at: DXIL's (space, register), which it packs as
//! `space << 16 | register` into the module binding, DXBC's (0, register), so the caller says how a module binding
//! becomes that pair.
use std::collections::{BTreeMap, BTreeSet};

use naga::Expression;

use crate::flow::{At, Node, NodeId, Values};
use crate::{Interface, Program, ResourceId, ResourceKind};

/// The words of each constant buffer a program's results are made of: every word some output lane, discard condition
/// or write ([`Values::writes`]) of the value graph reaches back to, at the word's place in the buffer (row `r`, lane
/// `l` is word `4r + l`). A buffer read at an index computed at run time may be read at any word. A word a computation
/// reads and nothing uses is not read. The reads are what the program may use, not what it must: a word multiplied by
/// zero at run time still counts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WordsRead {
    /// The words read at places the program states, by buffer.
    words: BTreeMap<ResourceId, BTreeSet<u32>>,
    /// The buffers read at an index computed at run time.
    indexed: BTreeSet<ResourceId>,
    /// The constant buffers by their D3D bind point (space, register).
    bound: BTreeMap<(u32, u32), ResourceId>,
}

impl WordsRead {
    /// The words `values` reads of the constant buffers of `interface` (the program's).
    pub fn of_values(values: &Values, interface: &Interface) -> WordsRead {
        let buffers: BTreeSet<ResourceId> = interface
            .resources
            .iter()
            .filter(|r| matches!(r.kind, ResourceKind::ConstantBuffer { .. }))
            .map(|r| r.id)
            .collect();
        let mut out = WordsRead {
            bound: interface
                .resources
                .iter()
                .filter(|r| buffers.contains(&r.id))
                .filter_map(|r| r.provenance.d3d.map(|d| ((d.space, d.register), r.id)))
                .collect(),
            ..WordsRead::default()
        };
        let mut seen = BTreeSet::new();
        let mut stack: Vec<NodeId> = values
            .outputs()
            .values()
            .copied()
            .chain(values.discards().iter().flatten().copied())
            .chain(values.writes().iter().flatten().copied())
            .collect();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            if let Node::Global {
                resource: Some(r),
                at,
            } = values.node(id)
                && buffers.contains(r)
            {
                match at {
                    At::Component(word) => {
                        out.words.entry(*r).or_default().insert(*word);
                    }
                    At::Indexed { .. } => {
                        out.indexed.insert(*r);
                    }
                }
            }
            stack.extend(values.operands(id));
        }
        out
    }

    /// The words of `buffer` read at places the program states, in order (none when it reads none).
    pub fn words(&self, buffer: ResourceId) -> impl Iterator<Item = u32> + '_ {
        self.words.get(&buffer).into_iter().flatten().copied()
    }

    /// The rows (registers of four lanes) of `buffer` with a word read at a place the program states, in order.
    pub fn rows(&self, buffer: ResourceId) -> Vec<u32> {
        let mut rows: Vec<u32> = self.words(buffer).map(|w| w / 4).collect();
        rows.dedup();
        rows
    }

    /// The lanes read of row `row` of `buffer`, in order.
    pub fn lanes(&self, buffer: ResourceId, row: u32) -> Vec<u32> {
        self.words
            .get(&buffer)
            .into_iter()
            .flat_map(|words| words.range(row.saturating_mul(4)..row.saturating_mul(4).saturating_add(4)))
            .map(|w| w % 4)
            .collect()
    }

    /// Whether `buffer` is read at an index computed at run time: any of its words may be read.
    pub fn indexed(&self, buffer: ResourceId) -> bool {
        self.indexed.contains(&buffer)
    }

    /// The constant buffer at the D3D bind point (`space`, `register`), if the program declares one there.
    pub fn buffer_at(&self, space: u32, register: u32) -> Option<ResourceId> {
        self.bound.get(&(space, register)).copied()
    }

    /// [`WordsRead::reads`] of the constant buffer at the D3D bind point (`space`, `register`): false where the program
    /// declares none there.
    pub fn reads_at(&self, space: u32, register: u32, offset: u32, bytes: u32) -> bool {
        self.buffer_at(space, register)
            .is_some_and(|buffer| self.reads(buffer, offset, bytes))
    }

    /// Whether the program reads any of the `bytes` bytes at `offset` of `buffer`.
    pub fn reads(&self, buffer: ResourceId, offset: u32, bytes: u32) -> bool {
        if self.indexed(buffer) {
            return true;
        }
        let (first, last) = (offset / 4, offset.saturating_add(bytes.max(1) - 1) / 4);
        self.words
            .get(&buffer)
            .is_some_and(|words| words.range(first..=last).next().is_some())
    }
}

impl Program {
    /// The constant-buffer words the program reads ([`WordsRead`]). An error is the value graph's: what it cannot
    /// follow ([`Values::of_program`]); a caller that must not leave out a word the program reads then takes every
    /// word as read.
    pub fn constant_reads(&self) -> Result<WordsRead, String> {
        Values::of_program(self).map(|values| WordsRead::of_values(&values, &self.interface))
    }
}

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
    /// The reads of the entry point of `module`, compacted first: the expressions no statement uses are not reads.
    /// `bind_point` gives the (space, register) a module binding stands for.
    pub fn of_module(
        mut module: naga::Module,
        bind_point: impl Fn(&naga::ResourceBinding) -> (u32, u32),
    ) -> ConstantReads {
        naga::compact::compact(&mut module, naga::compact::KeepUnused::Yes);
        let mut out = ConstantReads::default();
        let Some(entry) = module.entry_points.first() else {
            return out;
        };
        let function = &entry.function;
        let bound = |g: naga::Handle<naga::GlobalVariable>| {
            let b = module.global_variables.try_get(g).ok()?.binding.as_ref()?;
            Some(bind_point(b))
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
            let Some((space, register)) = bound(*g) else {
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

    /// The lanes read of the row at (`space`, `register`), in order.
    pub fn lanes_of(&self, space: u32, register: u32, row: u32) -> Vec<u32> {
        (0..4)
            .filter(|l| {
                self.whole.contains(&(space, register, row))
                    || self.lanes.contains(&(space, register, row, *l))
            })
            .collect()
    }

    /// The rows read of the buffer at (`space`, `register`), in order, and whether it is read at any row (an index the
    /// program computes: every row is then read, and the rows listed are the literal ones).
    pub fn rows_of(&self, space: u32, register: u32) -> (Vec<u32>, bool) {
        let rows: BTreeSet<u32> = self
            .whole
            .iter()
            .filter(|(s, r, _)| (*s, *r) == (space, register))
            .map(|(_, _, row)| *row)
            .chain(
                self.lanes
                    .iter()
                    .filter(|(s, r, _, _)| (*s, *r) == (space, register))
                    .map(|(_, _, row, _)| *row),
            )
            .collect();
        (
            rows.into_iter().collect(),
            self.dynamic.contains(&(space, register)),
        )
    }
}
