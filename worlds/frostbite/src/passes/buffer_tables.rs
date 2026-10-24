//! The instance and matrix tables of a vertex program, read by running it ([`probe_tables`]).
//!
//! An instanced Frostbite vertex program finds its world position in two buffers, not in constants. The instance table
//! holds one record of a few words per instance (the program reads word `matrix_start` of record `base + instance`), the
//! record's word is the index of an element of the matrix table, and the matrix table's elements are four words each: that
//! element is a header, and three rows of four words follow it. The programs differ in the record's size and word, the
//! buffers' registers and how many other things they read, and are written as long expressions with bit operations
//! ([`super::clip_position`] reads the clip position's form from the value graph; these tables are not matched by shape
//! that way but by running the program).
//!
//! The probe runs the program on the CPU executor with every constant zero but the rows of its clip position (identity)
//! and no buffer bound, so every read of a buffer is answered by a backend that logs it. The first buffer a program reads is
//! its instance table; the second is its matrix table, whose first element read is taken as the header, zero, with the
//! rows of the identity transform in the three elements after it. When the program, at instance 0 and at instance 3, gives
//! the clip position of the position it was given (a vertex at the identity transform, its world position taken from the
//! table), the layout holds for it, and the two instances' first read addresses give the record's size. Which word of the
//! record names the header is read from a third run in which each word of the record names a different element: the
//! header element the program then reads is the one its word names.
//! The instance number is added to a word of a constant buffer before the record is read (the draw's first record). That
//! word is found in the value graph (the constants the index of an instance table read is made of) and proved by a run: set to
//! 3, the first record read moves by three records ([`InstanceBase`]).
//! That a zero header means the rows follow it is what the programs gave; the header's bits (a flag with a 16-bit offset in
//! bits 1 to 16, a flag at bit 18) move the rows by what the programs read from them and are not otherwise documented.
//!
//! What was checked, six programs of one install (instance tables of one, two and three words, at registers 0 and 2;
//! matrix tables at register 67): the read addresses follow the record word as above with a zero header and move by the
//! offset, one element for the flag at bit 18, and both added when the flags are set; the identity rows give the position
//! back at instances 0 and 3 exactly. A program that does not do this (another header layout, a third buffer, a record
//! word that is not the last, rows not directly after a zero header) fails the probe and its buffers stay unknown.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::shader::{
    BufferKind, Interface, ResourceId, ResourceKind, ScalarType, Shader, ShaderFormat, Stage, SystemValue,
};
use primase_shader::WordsRead;
use primase_shader::flow::{At, Node, Values};
use primase_shader_exec::probe::{BufferReads, ProbeRun, VertexProbe};

use super::clip_position::{ClipRows, pass_buffer};

/// The element of the matrix table the instance records name in the probe.
const RECORD: u32 = 40;
/// The position the probe gives the program (the first three floats of every float input) and the clip position it expects
/// back at the identity transform.
const POSITION: [f32; 4] = [0.3, 0.4, 0.5, 0.6];
const EXPECTED: [f32; 4] = [0.3, 0.4, 0.5, 1.0];

/// The tables a program reads its matrices through, as ids of the shader's own interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableRoles {
    /// The instance table.
    pub instances: ResourceId,
    /// Words of a record of it.
    pub stride: u8,
    /// The word of a record that names the matrix's header element.
    pub matrix_start: u8,
    /// The matrix table: headers and three rows each.
    pub matrices: ResourceId,
    /// The word the instance number is added to before a record is read, when the program has one.
    pub base: Option<InstanceBase>,
}

/// The constant word that holds the index of the draw's first record in the instance table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstanceBase {
    /// The constant buffer, as an id of the shader's own interface.
    pub buffer: ResourceId,
    /// The word's byte offset in it.
    pub offset: u32,
}

/// What a probe gives the instance base word to see that the first record read moves by that many records.
const BASE_PROBE: u32 = 3;

/// A backend that answers every buffer read and logs which buffers were read first.
struct Tables {
    first: Option<ResourceId>,
    second: Option<ResourceId>,
    /// The first word read of the first buffer.
    first_word: Option<u32>,
    /// Whether a third buffer was read.
    more: bool,
    /// `None`: every word of the instance table says the element [`RECORD`]. `Some(stride)`: the word `w` says
    /// `RECORD + 8 * (w % stride)`, so the header element the matrix table is read at names the word of the record
    /// the program takes.
    marked: Option<u32>,
    /// The element of the matrix table read first: the header, with the rows of the identity transform after it.
    header: Option<u32>,
}

/// Whether `clip` is the position the probe gave, to within rounding; a NaN is not.
fn is_position(clip: &[f32; 4]) -> bool {
    clip.iter()
        .zip(EXPECTED)
        .all(|(got, want)| (got - want).abs() <= 1e-4)
}

/// The word `word` of the matrix table when its header is at element `header`: a zero header, then the rows of the
/// identity transform.
fn matrix_word(header: u32, word: u32) -> u32 {
    let (element, lane) = (word / 4, word % 4);
    if element > header && element <= header + 3 && lane == element - header - 1 {
        1.0f32.to_bits()
    } else {
        0
    }
}

impl BufferReads for Tables {
    fn read(&mut self, view: ResourceId, word: u32, count: u8) -> Result<[u32; 4], String> {
        match (self.first, self.second) {
            (None, _) => {
                self.first = Some(view);
                self.first_word = Some(word);
            }
            (Some(first), None) if first != view => self.second = Some(view),
            (Some(first), Some(second)) if first != view && second != view => self.more = true,
            _ => {}
        }
        let mut out = [0u32; 4];
        for (k, slot) in out.iter_mut().enumerate().take(usize::from(count)) {
            let w = word + k as u32;
            *slot = if Some(view) == self.first {
                match self.marked {
                    Some(stride) => RECORD + 8 * (w % stride),
                    None => RECORD,
                }
            } else {
                let header = *self.header.get_or_insert(w / 4);
                matrix_word(header, w)
            };
        }
        Ok(out)
    }
}

/// The tables `shader` (a vertex program with the camera-relative clip form `clip`) reads its matrices through, in the
/// shader's own interface, when running it as the module documentation says gives the position back at two instances.
pub fn probe_tables(shader: &Shader, clip: ClipRows) -> Result<TableRoles, String> {
    if shader.interface.stage != Stage::Vertex {
        return Err("the shader is not a vertex program".into());
    }
    if shader.format != ShaderFormat::Dxil {
        return Err("the program is not DXIL".into());
    }
    let program =
        primase_dxil::lift(&shader.bytecode).map_err(|e| format!("the program does not lift: {e}"))?;
    let probe = VertexProbe::new(program)?;
    let program = probe.program();
    let iface = &program.interface;
    let pass = pass_buffer(iface).ok_or("the lifted program has no constant buffer at register 4")?;
    let run = |instance: u32,
               marked: Option<u32>,
               base: Option<(ResourceId, u32)>|
     -> Result<ProbeRun<Tables>, String> {
        let mut constants = BTreeMap::new();
        for r in &iface.resources {
            if let ResourceKind::ConstantBuffer { layout } = &r.kind {
                let mut rows = vec![[0u32; 4]; layout.size.div_ceil(16).max(1) as usize];
                if r.id == pass {
                    // the clip rows are the identity and the camera is at the origin
                    for k in 0..4usize {
                        let at = clip.rows as usize + 4 * k + k;
                        rows[at / 4][at % 4] = 1.0f32.to_bits();
                    }
                }
                if let Some((buffer, word)) = base
                    && r.id == buffer
                {
                    rows[word as usize / 4][word as usize % 4] = BASE_PROBE;
                }
                constants.insert(r.id, rows);
            }
        }
        let args: Vec<[u32; 4]> = iface
            .inputs
            .iter()
            .map(|i| match (i.system, i.value.scalar) {
                (Some(SystemValue::InstanceIndex), _) => [instance, 0, 0, 0],
                (Some(_), _) => [0; 4],
                (None, ScalarType::Float) => POSITION.map(f32::to_bits),
                (None, _) => [0; 4],
            })
            .collect();
        let tables = Tables {
            first: None,
            second: None,
            first_word: None,
            more: false,
            marked,
            header: None,
        };
        probe.run(constants, &args, tables)
    };
    let (a, b) = (run(0, None, None)?, run(3, None, None)?);
    for r in [&a, &b] {
        if !is_position(&r.clip) {
            return Err(r.textures.reason(format!(
                "with a zero header and identity rows its clip position is {:?}, not the position given",
                r.clip
            )));
        }
        if r.buffers.more {
            return Err("the program reads more than two buffers".into());
        }
    }
    let (Some(instances), Some(matrices)) = (a.buffers.first, a.buffers.second) else {
        return Err("the program reads one buffer, not an instance table and a matrix table".into());
    };
    if (b.buffers.first, b.buffers.second) != (Some(instances), Some(matrices)) {
        return Err("the instances read other buffers".into());
    }
    let (Some(at0), Some(at3)) = (a.buffers.first_word, b.buffers.first_word) else {
        return Err("the program reads no instance record".into());
    };
    let step = at3.checked_sub(at0).filter(|s| s % 3 == 0 && *s > 0);
    let Some(stride) = step.map(|s| s / 3).and_then(|s| u8::try_from(s).ok()) else {
        return Err(format!(
            "the records of instances 0 and 3 start at words {at0} and {at3}, not a whole record apart"
        ));
    };
    // which word of the record the program takes: each word names another header element, and the one read says which
    let c = run(0, Some(u32::from(stride)), None)?;
    let Some(header) = c.buffers.header else {
        return Err("the program reads no matrix header".into());
    };
    let named = header.wrapping_sub(RECORD);
    if named % 8 != 0 || named / 8 >= u32::from(stride) {
        return Err(format!(
            "the header element {header} is not one a record word names"
        ));
    }
    if !is_position(&c.clip) {
        return Err(c.textures.reason(format!(
            "with the record word it takes its clip position is {:?}, not the position given",
            c.clip
        )));
    }
    let matrix_start = (named / 8) as u8;
    // the same bindings in the shader's own interface, with the element sizes the roles need
    let own = |id: ResourceId, bytes: u32| -> Result<ResourceId, String> {
        let d3d = iface
            .resource(id)
            .and_then(|r| r.provenance.d3d)
            .ok_or("a buffer without a binding")?;
        let found = shader
            .interface
            .resources
            .iter()
            .find(|r| matches!(r.kind, ResourceKind::Buffer(_)) && r.provenance.d3d == Some(d3d))
            .ok_or("a buffer the shader's interface does not have")?;
        if let ResourceKind::Buffer(BufferKind::Structured { stride }) = found.kind
            && stride != bytes
        {
            return Err(format!(
                "a buffer of elements of {stride} bytes is read as elements of {bytes}"
            ));
        }
        Ok(found.id)
    };
    // the word the instance number is added to: a constant the index of an instance table read is made of, which when it
    // is 3 moves the first record read by three records
    let values = Values::of_program(program).map_err(|e| format!("the value graph: {e}"))?;
    let mut proved = Vec::new();
    for (buffer, word) in index_constants(&values, iface, instances) {
        let moved = run(0, None, Some((buffer, word)))?;
        if is_position(&moved.clip) && moved.buffers.first_word == Some(at0 + BASE_PROBE * u32::from(stride))
        {
            proved.push((buffer, word));
        }
    }
    let base = match proved.as_slice() {
        [(buffer, word)] => {
            let d3d = iface
                .resource(*buffer)
                .and_then(|r| r.provenance.d3d)
                .ok_or("a constant buffer without a binding")?;
            shader
                .interface
                .resources
                .iter()
                .find(|r| {
                    matches!(r.kind, ResourceKind::ConstantBuffer { .. }) && r.provenance.d3d == Some(d3d)
                })
                .map(|r| InstanceBase {
                    buffer: r.id,
                    offset: word * 4,
                })
        }
        _ => None,
    };
    Ok(TableRoles {
        instances: own(instances, u32::from(stride) * 4)?,
        stride,
        matrix_start,
        matrices: own(matrices, 16)?,
        base,
    })
}

/// The constant-buffer words (buffer of the lifted interface, word) that the index of a read of the buffer `table` is made
/// of, over the nodes the program's results reach ([`Values::roots`]).
fn index_constants(values: &Values, iface: &Interface, table: ResourceId) -> BTreeSet<(ResourceId, u32)> {
    let indexes = values
        .reached_from(values.roots())
        .into_iter()
        .filter_map(|id| match values.node(id) {
            Node::Global {
                resource: Some(resource),
                at: At::Indexed { index, .. },
            } if *resource == table => Some(index.clone()),
            _ => None,
        })
        .flatten();
    let words = WordsRead::of_nodes(values, iface, &values.reached_from(indexes));
    words
        .buffers()
        .flat_map(|buffer| words.words(buffer).map(move |word| (buffer, word)))
        .collect()
}
