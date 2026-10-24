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
//! That a zero header means the rows follow it is what the programs gave; the header's bits (a flag with a 16-bit offset in
//! bits 1 to 16, a flag at bit 18) move the rows by what the programs read from them and are not otherwise documented.
//!
//! What was checked, six programs of one install (instance tables of one, two and three words, at registers 0 and 2;
//! matrix tables at register 67): the read addresses follow the record word as above with a zero header and move by the
//! offset, one element for the flag at bit 18, and both added when the flags are set; the identity rows give the position
//! back at instances 0 and 3 exactly. A program that does not do this (another header layout, a third buffer, a record
//! word that is not the last, rows not directly after a zero header) fails the probe and its buffers stay unknown.

use primase_core::shader::{
    BufferKind, ResourceId, ResourceKind, ScalarType, Shader, ShaderFormat, Stage, SystemValue,
};
use primase_shader_exec::{Backend, Element, Executable, ImageSample, Resources, Runner};

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
}

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

impl Backend for Tables {
    fn sample(&mut self, _request: ImageSample<Element>) -> Result<[f32; 4], String> {
        Ok([0.5; 4])
    }
    fn read_buffer(&mut self, view: ResourceId, word: u32, count: u8) -> Result<[u32; 4], String> {
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
    fn buffer_words(&mut self, _view: ResourceId) -> Result<u32, String> {
        Ok(1 << 20)
    }
}

/// One run of the program at `instance`: the clip position and what the backend saw.
struct Run {
    clip: [f32; 4],
    backend: Tables,
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
    let iface = &program.interface;
    let position = iface
        .outputs
        .iter()
        .position(|o| o.system == Some(SystemValue::ClipPosition))
        .ok_or("the program writes no clip position")?;
    let pass = pass_buffer(iface).ok_or("the lifted program has no constant buffer at register 4")?;
    let exe =
        Executable::new(&program).map_err(|e| format!("the executor does not take the program: {e}"))?;
    let run = |instance: u32, marked: Option<u32>| -> Result<Run, String> {
        let mut resources = Resources::default();
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
                resources.constants.insert(r.id, rows);
            }
        }
        let bound = exe
            .bind_resources(&resources)
            .map_err(|e| format!("the constants do not bind: {e}"))?;
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
        let mut backend = Tables {
            first: None,
            second: None,
            first_word: None,
            more: false,
            marked,
            header: None,
        };
        let mut runner = Runner::<1>::new(&exe, &bound, &mut backend, None);
        let [out] = runner
            .run([&args])
            .map_err(|e| format!("the program does not run: {e}"))?;
        let clip = out.values[position].map(f32::from_bits);
        Ok(Run { clip, backend })
    };
    let (a, b) = (run(0, None)?, run(3, None)?);
    for r in [&a, &b] {
        if !is_position(&r.clip) {
            return Err(format!(
                "with a zero header and identity rows its clip position is {:?}, not the position given",
                r.clip
            ));
        }
        if r.backend.more {
            return Err("the program reads more than two buffers".into());
        }
    }
    let (Some(instances), Some(matrices)) = (a.backend.first, a.backend.second) else {
        return Err("the program reads one buffer, not an instance table and a matrix table".into());
    };
    if (b.backend.first, b.backend.second) != (Some(instances), Some(matrices)) {
        return Err("the instances read other buffers".into());
    }
    let (Some(at0), Some(at3)) = (a.backend.first_word, b.backend.first_word) else {
        return Err("the program reads no instance record".into());
    };
    let step = at3.checked_sub(at0).filter(|s| s % 3 == 0 && *s > 0);
    let Some(stride) = step.map(|s| s / 3).and_then(|s| u8::try_from(s).ok()) else {
        return Err(format!(
            "the records of instances 0 and 3 start at words {at0} and {at3}, not a whole record apart"
        ));
    };
    // which word of the record the program takes: each word names another header element, and the one read says which
    let c = run(0, Some(u32::from(stride)))?;
    let Some(header) = c.backend.header else {
        return Err("the program reads no matrix header".into());
    };
    let named = header.wrapping_sub(RECORD);
    if named % 8 != 0 || named / 8 >= u32::from(stride) {
        return Err(format!(
            "the header element {header} is not one a record word names"
        ));
    }
    if !is_position(&c.clip) {
        return Err(format!(
            "with the record word it takes its clip position is {:?}, not the position given",
            c.clip
        ));
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
    Ok(TableRoles {
        instances: own(instances, u32::from(stride) * 4)?,
        stride,
        matrix_start,
        matrices: own(matrices, 16)?,
    })
}
