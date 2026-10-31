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

use iridia::Program;
use iridia::flow::{At, Node, NodeId, Values};
use iridia_exec::probe::{ProbeRun, VertexProbe, Zeros};
use iridia_interface::{BufferLayout, ConstantType, ResourceId, ResourceKind, ScalarType};
use std::collections::{BTreeMap, BTreeSet};

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

/// Every constant of a buffer's layout, the members of a structure (the instance record, an array of them) by their own
/// names at their own offsets in the first element: name, byte offset, type.
pub(crate) fn flat_constants(layout: &BufferLayout) -> Vec<(String, u32, &ConstantType)> {
    fn descend<'a>(
        name: &str,
        offset: u32,
        ty: &'a ConstantType,
        flat: &mut Vec<(String, u32, &'a ConstantType)>,
    ) {
        match ty.members() {
            [] => flat.push((name.to_owned(), offset, ty)),
            members => {
                for member in members {
                    descend(&member.name, offset + member.offset, &member.ty, flat);
                }
            }
        }
    }
    let mut flat = Vec::new();
    for c in &layout.constants {
        descend(&c.name, c.offset, &c.ty, &mut flat);
    }
    flat
}

/// The bytes from a constant's start that hold a 1 in the identity of its type `ty`: a matrix's diagonal (line k's
/// lane k, whichever way its lines hold it), each of an array of matrices', or each matrix's of an array of `float4`
/// rows three to a matrix (FromSoftware's palettes and object matrices: three rows of four per matrix, as the programs
/// that apply them read them). Any other type is refused: the probe would not know where the identity's words are.
fn identity(ty: &ConstantType) -> Result<Vec<u32>, String> {
    let refused = |ty: &ConstantType| {
        format!(
            "a {ty:?}, not a matrix, an array of them or an array of float4 rows three to a matrix: the probe does not \
             know where its identity is"
        )
    };
    match ty {
        ConstantType::Matrix {
            rows,
            columns,
            stride,
            ..
        } => Ok((0..u32::from(*rows.min(columns)))
            .map(|k| k * stride + 4 * k)
            .collect()),
        ConstantType::Array {
            element,
            count,
            stride,
        } => match element.as_ref() {
            ConstantType::Matrix { .. } => {
                let one = identity(element)?;
                Ok((0..*count)
                    .flat_map(|e| one.iter().map(move |byte| e * stride + byte))
                    .collect())
            }
            ConstantType::Vector(v) if v.width == 4 && v.scalar == ScalarType::Float && count % 3 == 0 => {
                Ok((0..*count).map(|row| row * stride + 4 * (row % 3)).collect())
            }
            _ => Err(refused(ty)),
        },
        _ => Err(refused(ty)),
    }
}

/// Whether the clip position of the vertex program `program` is of a world with the camera at its origin, by running it
/// with the matrix, the palettes and the camera at the values of the module documentation ([`VertexProbe`]).
pub fn camera_form(program: &Program) -> Result<CameraForm, String> {
    let probe = VertexProbe::new(program.clone())?;
    let iface = &probe.program().interface;
    let inputs: Vec<[u32; 4]> = iface
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
    let run = |camera: [f32; 3]| -> Result<ProbeRun<Zeros>, String> {
        let mut constants = BTreeMap::new();
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
            for (name, offset, ty) in flat_constants(layout) {
                match name.as_str() {
                    // the identity, each matrix of it
                    "VC_MatrixViewProj" | "VC_aObjMatrix" | "VC_aClothCancelObjMatrix" | "mWorld" => {
                        for byte in identity(ty).map_err(|why| format!("{name} is {why}"))? {
                            set(offset + byte, 1.0);
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
            constants.insert(r.id, rows);
        }
        probe.run(constants, &inputs, Zeros)
    };
    let near = |got: [f32; 4], want: [f32; 4]| got.iter().zip(want).all(|(g, w)| (g - w).abs() <= 1e-4);
    let (x, y, z) = (POSITION[0], POSITION[1], POSITION[2]);
    let at_origin = run([0.0; 3])?;
    if !near(at_origin.clip, [x, y, z, 1.0]) {
        return Err(at_origin.textures.reason(format!(
            "at the identity matrix and palettes its clip position is {:?}, not the position given",
            at_origin.clip
        )));
    }
    let moved = run(CAMERA)?;
    if near(moved.clip, at_origin.clip) {
        Ok(CameraForm::Plain)
    } else if near(moved.clip, [x - CAMERA[0], y - CAMERA[1], z - CAMERA[2], 1.0]) {
        Ok(CameraForm::Relative)
    } else {
        Err(moved.textures.reason(format!(
            "with the camera moved its clip position is {:?}, neither the same nor the position minus the camera",
            moved.clip
        )))
    }
}

/// Whether a read of the constant buffer `buffer` that the results of `program` are made of ([`Values::roots`]: its
/// outputs, discards and writes) is indexed by a vertex stream (a joint index), not by the instance alone: the buffer
/// is a palette of the vertex, not the object's matrices. `Err` when the value graph cannot be built.
pub fn palette_indexed_by_vertex(program: &Program, buffer: ResourceId) -> Result<bool, String> {
    let values = Values::of_program(program).map_err(|e| format!("the value graph: {e}"))?;
    let by_vertex = |index: &[NodeId]| {
        values.reached_from(index.iter().copied()).into_iter().any(|id| {
            matches!(values.node(id), Node::Input { input, .. } if program.interface.inputs[*input].system.is_none())
        })
    };
    Ok(values.reached_from(values.roots()).into_iter().any(|id| {
        matches!(
            values.node(id),
            Node::Global { resource: Some(resource), at: At::Indexed { index, .. } }
                if *resource == buffer && by_vertex(index)
        )
    }))
}

/// The inputs of `shader` its results are made of ([`Values::roots`]), by label (the name in capitals and the index):
/// an input declared and never read is not in it. `Err` when the graph cannot be built.
pub fn inputs_read(program: &Program) -> Result<BTreeSet<(String, u32)>, String> {
    let values = Values::of_program(program).map_err(|e| format!("the value graph: {e}"))?;
    Ok(values
        .reached_from(values.roots())
        .into_iter()
        .filter_map(|id| match values.node(id) {
            Node::Input { input, .. } => {
                let label = &program.interface.inputs[*input].label;
                Some((label.name.to_ascii_uppercase(), label.index))
            }
            _ => None,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use iridia_interface::{Majorness, Value};

    #[test]
    fn nested_constant_members_keep_their_absolute_offsets() {
        use iridia_interface::{Constant, Member};
        let layout = BufferLayout {
            size: 144,
            constants: vec![Constant {
                name: "VC_SpeedTreeWind".into(),
                offset: 16,
                ty: ConstantType::Array {
                    count: 2,
                    stride: 64,
                    element: Box::new(ConstantType::Struct {
                        members: vec![Member {
                            name: "u_sGlobal".into(),
                            offset: 32,
                            ty: ConstantType::Struct {
                                members: vec![Member {
                                    name: "m_fSkip".into(),
                                    offset: 4,
                                    ty: ConstantType::Scalar(ScalarType::Float),
                                }],
                            },
                        }],
                    }),
                },
                default: None,
            }],
        };
        assert_eq!(
            flat_constants(&layout),
            vec![("m_fSkip".to_owned(), 52, &ConstantType::Scalar(ScalarType::Float))],
            "the first array element's nested leaf, including every enclosing offset"
        );
    }

    fn matrix(rows: u8, columns: u8, major: Majorness) -> ConstantType {
        ConstantType::Matrix {
            scalar: ScalarType::Float,
            rows,
            columns,
            major,
            stride: 16,
        }
    }

    /// The identity is the diagonal of each matrix, line k's lane k, however its lines hold it: a `row_major float4x3`
    /// is four lines, its last (the translation) all zero, where a `column_major` one is three; a palette of `float4`
    /// rows is three rows to a matrix; anything else is refused by type.
    #[test]
    fn the_identity_is_placed_by_the_constants_type() {
        assert_eq!(
            identity(&matrix(4, 4, Majorness::Column)),
            Ok(vec![0, 20, 40, 60])
        );
        assert_eq!(identity(&matrix(4, 3, Majorness::Row)), Ok(vec![0, 20, 40]));
        assert_eq!(identity(&matrix(4, 3, Majorness::Column)), Ok(vec![0, 20, 40]));
        let palette = ConstantType::Array {
            element: Box::new(matrix(4, 3, Majorness::Row)),
            count: 2,
            stride: 64,
        };
        assert_eq!(identity(&palette), Ok(vec![0, 20, 40, 64, 84, 104]));
        let rows = |count, width| ConstantType::Array {
            element: Box::new(ConstantType::Vector(Value {
                scalar: ScalarType::Float,
                width,
            })),
            count,
            stride: 16,
        };
        assert_eq!(identity(&rows(6, 4)), Ok(vec![0, 20, 40, 48, 68, 88]));
        assert!(identity(&rows(4, 4)).is_err(), "not three rows to a matrix");
        assert!(identity(&rows(6, 3)).is_err(), "not rows of four");
        assert!(identity(&ConstantType::Scalar(ScalarType::Float)).is_err());
    }
}
