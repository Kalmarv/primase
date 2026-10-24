//! Which constants of its constant buffers a program reads.
//!
//! A program's buffers declare far more than it uses: an engine's per-frame buffers hold some hundred vectors and a
//! program reads a handful, so a consumer binds only what it reads. [`WordsRead`] is the answer from the value graph
//! ([`crate::flow::Values`]): the words its outputs, discards and writes are made of, by buffer. It is word-granular
//! however the program loads (a lifted DXIL program loads a whole register and takes lanes afterwards, so a load says
//! nothing of which lanes count), and a buffer indexed by a value the program computes (a bone or object-matrix
//! palette) is read at any word. [`Program::constant_reads`] is it for a program. A buffer is named by its resource, or
//! by the D3D bind point (space, register) its provenance states.
use std::collections::{BTreeMap, BTreeSet};

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
    /// The words `values` reads of the constant buffers of `interface` (the program's): those of the nodes its results
    /// are made of ([`Values::roots`]).
    pub fn of_values(values: &Values, interface: &Interface) -> WordsRead {
        WordsRead::of_nodes(values, interface, &values.reached_from(values.roots()))
    }

    /// The constant-buffer words among `nodes` of `values` (a set [`Values::reached_from`] gives: what one value, such
    /// as an index, is made of).
    pub fn of_nodes(values: &Values, interface: &Interface, nodes: &BTreeSet<NodeId>) -> WordsRead {
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
        for &id in nodes {
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
        }
        out
    }

    /// The constant buffers with a word read, in order.
    pub fn buffers(&self) -> impl Iterator<Item = ResourceId> + '_ {
        self.words.keys().copied()
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
