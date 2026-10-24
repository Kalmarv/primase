//! What a vertex program does with the camera and with its palettes, read by running and walking it.
//!
//! A FromSoft vertex program places a vertex with the engine's view-projection matrix, `VC_MatrixViewProj`, and the
//! engine's names say nothing about the world that matrix expects: the programs of Elden Ring's census take either a world
//! with the camera at its origin (the program subtracts `SC_CameraPos` from the world position first: 15,483 of the 15,707
//! programs whose clip position has the form of a matrix times a world position) or the world itself (the other 224). So
//! the role of the matrix is read from the program: [`camera_form`] runs it with the matrix, its palettes and the camera
//! set to values that make the clip position equal the world position it was given, moves the camera, and sees whether
//! the clip position moves with it. A program the run does not decide (it does not give the position back at the identity,
//! or moves in another way) has its matrix left unknown, with the reason.
//!
//! The constants are placed by the names of the program's own reflection (`VC_MatrixViewProj`, `VC_aObjMatrix`,
//! `VC_aClothCancelObjMatrix`, `mWorld`, `SC_CameraPos`): what the run shows is what they do, not what they are called.
//!
//! [`palette_indexed_by_vertex`] reads in the value graph whether a constant buffer is indexed by a vertex stream, as a
//! joint palette is, or only by the instance, as the object matrices of an unskinned draw are.

use primase_core::shader::{
    BufferLayout, ResourceId, ResourceKind, ScalarType, Shader, ShaderFormat, Stage, SystemValue,
};
use primase_shader::Program;
use primase_shader::flow::{At, Node, NodeId, Values};
use primase_shader_exec::{
    Backend, Element, Executable, ImageLoad, ImageQuery, ImageSample, Resources, Runner,
};
use std::collections::BTreeSet;

/// The position the probe gives the program, and the camera it moves to.
const POSITION: [f32; 3] = [0.3, 0.4, 0.5];
const CAMERA: [f32; 3] = [0.5, 0.25, 0.125];

/// What the program does with the camera.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraForm {
    /// The program subtracts the camera position from the world position before the matrix: the matrix is the
    /// view-projection of a world with the camera at its origin.
    Relative,
    /// The program multiplies the world position as it is.
    Plain,
}

/// A backend that answers every read with zero: no instance table, no texture (a texture is one texel).
struct Zeros;

impl Backend for Zeros {
    fn sample(&mut self, _request: ImageSample<Element>) -> Result<[f32; 4], String> {
        Ok([0.5; 4])
    }
    fn read_buffer(&mut self, _view: ResourceId, _word: u32, _count: u8) -> Result<[u32; 4], String> {
        Ok([0; 4])
    }
    // a texture the program loads from or asks the size of (a vertex texture of its own): one texel of zeros
    fn load(&mut self, _request: ImageLoad<Element>) -> Result<[u32; 4], String> {
        Ok([0; 4])
    }
    fn query(&mut self, _request: ImageQuery<Element>) -> Result<[u32; 4], String> {
        Ok([1; 4])
    }
    fn buffer_words(&mut self, _view: ResourceId) -> Result<u32, String> {
        Ok(1 << 20)
    }
}

/// The program of a shader, lifted from its bytecode.
fn lift(shader: &Shader) -> Result<Program, String> {
    match shader.format {
        ShaderFormat::Dxbc => primase_dxbc::lift(&shader.bytecode).map_err(|e| e.to_string()),
        ShaderFormat::Dxil => primase_dxil::lift(&shader.bytecode).map_err(|e| e.to_string()),
        other => Err(format!("a {other:?} program")),
    }
}

/// Every constant of a buffer's layout, the members of a structure (the instance record, an array of them) by their own
/// names at their own offsets in the first element: name, byte offset, size.
pub(crate) fn flat_constants(layout: &BufferLayout) -> Vec<(String, u32, Option<u32>)> {
    let mut flat = Vec::new();
    for c in &layout.constants {
        match c.ty.members() {
            [] => flat.push((c.name.clone(), c.offset, c.ty.size())),
            members => flat.extend(
                members
                    .iter()
                    .map(|m| (m.name.clone(), c.offset + m.offset, m.ty.size())),
            ),
        }
    }
    flat
}

/// Whether the clip position of the vertex program `shader` is of a world with the camera at its origin, by running it with
/// the matrix, the palettes and the camera at the values of the module documentation.
pub fn camera_form(shader: &Shader) -> Result<CameraForm, String> {
    if shader.interface.stage != Stage::Vertex {
        return Err("the shader is not a vertex program".into());
    }
    let program = lift(shader).map_err(|e| format!("the program does not lift: {e}"))?;
    let iface = &program.interface;
    let position = iface
        .outputs
        .iter()
        .position(|o| o.system == Some(SystemValue::ClipPosition))
        .ok_or("the program writes no clip position")?;
    let exe =
        Executable::new(&program).map_err(|e| format!("the executor does not take the program: {e}"))?;
    let run = |camera: [f32; 3]| -> Result<[f32; 4], String> {
        let mut resources = Resources::default();
        for r in &iface.resources {
            let ResourceKind::ConstantBuffer { layout } = &r.kind else {
                continue;
            };
            let mut rows = vec![[0u32; 4]; layout.size.div_ceil(16).max(1) as usize];
            let mut set = |byte: u32, value: f32| {
                let word = byte as usize / 4;
                if let Some(row) = rows.get_mut(word / 4) {
                    row[word % 4] = value.to_bits();
                }
            };
            for (name, offset, size) in flat_constants(layout) {
                match name.as_str() {
                    // the identity
                    "VC_MatrixViewProj" => (0..4).for_each(|i| set(offset + 20 * i, 1.0)),
                    // matrices of three rows of four, each the identity
                    "VC_aObjMatrix" | "VC_aClothCancelObjMatrix" | "mWorld" => {
                        for row in 0..size.unwrap_or(0) / 16 {
                            set(offset + 16 * row + 4 * (row % 3), 1.0);
                        }
                    }
                    "SC_CameraPos" => {
                        for (i, c) in camera.into_iter().enumerate() {
                            set(offset + 4 * i as u32, c);
                        }
                        set(offset + 12, 1.0);
                    }
                    _ => {}
                }
            }
            resources.constants.insert(r.id, rows);
        }
        let bound = exe
            .bind_resources(&resources)
            .map_err(|e| format!("the constants do not bind: {e}"))?;
        let args: Vec<[u32; 4]> = iface
            .inputs
            .iter()
            .map(|i| {
                let name = i.label.name.to_ascii_uppercase();
                match (i.system, i.value.scalar, name.as_str()) {
                    (Some(_), _, _) => [0; 4],
                    (None, ScalarType::Float, "POSITION") => [
                        POSITION[0].to_bits(),
                        POSITION[1].to_bits(),
                        POSITION[2].to_bits(),
                        1.0f32.to_bits(),
                    ],
                    // the whole weight on the first joint
                    (None, ScalarType::Float, "BLENDWEIGHT") => [1.0f32.to_bits(), 0, 0, 0],
                    _ => [0; 4],
                }
            })
            .collect();
        let mut backend = Zeros;
        let mut runner = Runner::<1>::new(&exe, &bound, &mut backend, None);
        let [out] = runner
            .run([&args])
            .map_err(|e| format!("the program does not run: {e}"))?;
        Ok(out.values[position].map(f32::from_bits))
    };
    let near = |got: [f32; 4], want: [f32; 4]| got.iter().zip(want).all(|(g, w)| (g - w).abs() <= 1e-4);
    let (x, y, z) = (POSITION[0], POSITION[1], POSITION[2]);
    let at_origin = run([0.0; 3])?;
    if !near(at_origin, [x, y, z, 1.0]) {
        return Err(format!(
            "at the identity matrix and palettes its clip position is {at_origin:?}, not the position given"
        ));
    }
    let moved = run(CAMERA)?;
    if near(moved, at_origin) {
        Ok(CameraForm::Plain)
    } else if near(moved, [x - CAMERA[0], y - CAMERA[1], z - CAMERA[2], 1.0]) {
        Ok(CameraForm::Relative)
    } else {
        Err(format!(
            "with the camera moved its clip position is {moved:?}, neither the same nor the position minus the camera"
        ))
    }
}

/// Whether a read of the constant buffer `buffer` that the outputs of `program` are made of is indexed by a vertex
/// stream (a joint index), not by the instance alone: the buffer is a palette of the vertex, not the object's matrices.
/// `Err` when the value graph cannot be built.
pub fn palette_indexed_by_vertex(program: &Program, buffer: ResourceId) -> Result<bool, String> {
    let values = Values::of_program(program).map_err(|e| format!("the value graph: {e}"))?;
    let mut pending: Vec<NodeId> = values.outputs().values().copied().collect();
    pending.extend(values.discards().iter().flatten().copied());
    let mut seen = BTreeSet::new();
    let mut indexes: Vec<NodeId> = Vec::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        if let Node::Global {
            resource: Some(resource),
            at: At::Indexed { index, .. },
        } = values.node(id)
            && *resource == buffer
        {
            indexes.extend(index);
        }
        pending.extend(values.operands(id));
    }
    let mut seen = BTreeSet::new();
    while let Some(id) = indexes.pop() {
        if !seen.insert(id) {
            continue;
        }
        if let Node::Input { input, .. } = values.node(id)
            && program.interface.inputs[*input].system.is_none()
        {
            return Ok(true);
        }
        indexes.extend(values.operands(id));
    }
    Ok(false)
}

/// The inputs of `shader` its value graph reads, by label (the name in capitals and the index): an input declared and
/// never read is not in it. `Err` when the graph cannot be built.
pub fn inputs_read(shader: &Shader) -> Result<BTreeSet<(String, u32)>, String> {
    let program = lift(shader)?;
    let values = Values::of_program(&program).map_err(|e| format!("the value graph: {e}"))?;
    let mut pending: Vec<NodeId> = values.outputs().values().copied().collect();
    pending.extend(values.discards().iter().flatten().copied());
    let mut seen = BTreeSet::new();
    let mut read = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        if let Node::Input { input, .. } = values.node(id) {
            let label = &program.interface.inputs[*input].label;
            read.insert((label.name.to_ascii_uppercase(), label.index));
        }
        pending.extend(values.operands(id));
    }
    Ok(read)
}

/// The lifted program of `shader` (for callers that read its value graph beside the run).
pub fn lifted(shader: &Shader) -> Result<Program, String> {
    lift(shader)
}
