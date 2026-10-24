//! How a program holds a matrix constant ([`primase_shader::matrix::lines`]), on the lifters' fixtures: a domain program
//! that applies a `column_major` matrix as `mul(M, v)` (its lines are the columns of the matrix it applies) as fxc and dxc
//! compile it, optimized and not, and a vertex program that applies one as `mul(v, M)` (the lines are the rows of the
//! matrix it applies to column vectors, the transpose) as dxc compiles it for two shader models and unoptimized. The
//! two lifters write a dot product as the same node, so the row form is not fxc-specific. The domain program is not walked
//! by the value graph (it has no entry point a WebGPU module can hold), so the column form is held on fxc's vertex
//! programs of FromSoftware's fixtures, which also hold two palettes chosen by an index, one blended across joints, and
//! a cloth program's blend chosen by a flag, as dxc and fxc compile it.
use primase_shader::flow::Values;
use primase_shader::matrix::{Unshown, Words, lines};
use primase_shader::{Majorness, Program, ResourceKind};

/// The arrangement `lines` reads for the constant `name`, by its place in the program's own layout, or, for a palette,
/// by the indexed reads of its buffer.
fn lines_of(program: &Program, name: &str) -> Result<Majorness, Unshown> {
    read(program, name, false)
}

fn palette_lines_of(program: &Program, name: &str) -> Result<Majorness, Unshown> {
    read(program, name, true)
}

fn read(program: &Program, name: &str, indexed: bool) -> Result<Majorness, Unshown> {
    let values = Values::of_program(program).expect("the value graph");
    for r in &program.interface.resources {
        let ResourceKind::ConstantBuffer { layout } = &r.kind else {
            continue;
        };
        if let Some(c) = layout.constants.iter().find(|c| c.name == name) {
            let size = c.ty.size().expect("a sized constant");
            let words = if indexed {
                Words::Indexed
            } else {
                Words::At(c.offset / 4..(c.offset + size).div_ceil(4))
            };
            return lines(&values, r.id, &words);
        }
    }
    panic!("no constant {name}")
}

fn dxbc(bytes: &[u8]) -> Program {
    primase_dxbc::lift(bytes).expect("fxc's program lifts")
}

fn dxil(bytes: &[u8]) -> Program {
    primase_dxil::lift(bytes).expect("dxc's program lifts")
}

const SINGLE_UV_VS: &[u8] = include_bytes!("../../../worlds/fromsoft/tests/fixtures/single_uv_vs.dxbc");
const SKIN_VS: &[u8] = include_bytes!("../../../worlds/fromsoft/tests/fixtures/layered_skin_vs.dxbc");

/// `mul(M, v)` of a `column_major` matrix scales each line by a lane of the vector: its lines are columns.
#[test]
fn a_matrix_applied_to_a_column_vector_is_read_by_columns() {
    let single = dxbc(SINGLE_UV_VS);
    assert_eq!(lines_of(&single, "VC_MatrixViewProj"), Ok(Majorness::Column));
    let skin = dxbc(SKIN_VS);
    for name in ["VC_MatrixViewProj", "FC_MatrixView"] {
        assert_eq!(lines_of(&skin, name), Ok(Majorness::Column), "{name}");
    }
}

/// A palette chosen by an index is read by the line of its element the index adds: `mul(v, M[i])` of a `column_major`
/// `float4x3` dots the vector with each of the element's lines, and a blend of the joints' matrices line by line
/// (`M[a] * wa + M[b] * wb`) that the program then dots the position with is read by rows too, the blend not being a
/// sum across lines.
#[test]
fn a_palette_is_read_by_the_lines_of_its_elements() {
    let skin = dxbc(SKIN_VS);
    for name in ["VC_aClothCancelObjMatrix", "VC_aObjMatrix"] {
        assert_eq!(palette_lines_of(&skin, name), Ok(Majorness::Row), "{name}");
    }
}

/// A cloth program that blends its joints' matrices word by word and then chooses each blended word by a flag against
/// another buffer's matrix (Elden Ring's, as dxc and fxc compile its shape) is read through the choice: the position is
/// dotted with each blended line, a row.
#[test]
fn a_palette_blended_then_chosen_by_a_flag_is_read_by_rows() {
    for (fixture, program) in [
        (
            "dxc",
            dxil(include_bytes!(
                "../../../worlds/fromsoft/tests/fixtures/layered_cloth_blend_vs.dxil"
            )),
        ),
        (
            "fxc",
            dxbc(include_bytes!(
                "../../../worlds/fromsoft/tests/fixtures/layered_cloth_blend_vs.dxbc"
            )),
        ),
    ] {
        assert_eq!(
            palette_lines_of(&program, "VC_aObjMatrix"),
            Ok(Majorness::Row),
            "{fixture}"
        );
    }
}

/// `mul(v, M)` of a `column_major` matrix dots the vector with each line: they are the rows of the matrix applied to
/// column vectors (`M` transposed).
#[test]
fn a_matrix_applied_to_a_row_vector_is_read_by_rows() {
    for (fixture, program) in [
        (
            "vertex.dxil",
            dxil(include_bytes!("../../dxil/tests/fixtures/vertex.dxil")),
        ),
        (
            "vertex.66.dxil",
            dxil(include_bytes!("../../dxil/tests/fixtures/vertex.66.dxil")),
        ),
        (
            "vertex.od.dxil",
            dxil(include_bytes!("../../dxil/tests/fixtures/vertex.od.dxil")),
        ),
    ] {
        assert_eq!(
            lines_of(&program, "world_view_projection"),
            Ok(Majorness::Row),
            "{fixture}"
        );
    }
}

/// A constant the program reads but never multiplies by another value shows no arrangement, and is refused with that.
#[test]
fn a_constant_never_applied_shows_no_arrangement() {
    let program = dxil(include_bytes!("../../dxil/tests/fixtures/vertex.dxil"));
    assert_eq!(lines_of(&program, "tint"), Err(Unshown::NotApplied));
}

/// `state_lines` states each matrix role of a stage from the program, finding its buffer by the D3D binding: the clip
/// matrix by columns, the joints' palette (its buffer's one array) through its indexed reads by rows, and a matrix the
/// program reads but never applies keeps its role with the reason it is unshown. Other roles are left alone.
#[test]
fn a_stage_states_the_lines_of_its_matrices() {
    use primase_core::shader::{ConstantBinding, ConstantRole, MatrixLines, ObjectParameter, ViewParameter};
    use primase_shader::matrix::state_lines;
    let skin = dxbc(SKIN_VS);
    let graph = skin.graph().expect("the value graph");
    let constant = |name: &str, role: ConstantRole| {
        let (resource, c) = skin
            .interface
            .resources
            .iter()
            .find_map(|r| match &r.kind {
                ResourceKind::ConstantBuffer { layout } => layout
                    .constants
                    .iter()
                    .find(|c| c.name == name)
                    .map(|c| (r.id, c)),
                _ => None,
            })
            .expect("the constant");
        ConstantBinding {
            resource,
            offset: c.offset,
            size: c.ty.size().expect("a sized constant"),
            name: Some(name.into()),
            role,
            lines: None,
        }
    };
    let camera = ConstantRole::View(ViewParameter::CameraPosition);
    let mut constants = vec![
        constant(
            "VC_MatrixViewProj",
            ConstantRole::View(ViewParameter::ViewProjection),
        ),
        constant(
            "VC_aObjMatrix",
            ConstantRole::Object(ObjectParameter::BonePalette),
        ),
        // read, but only added and scaled lane by lane: not applied as a matrix
        constant("SC_CameraPos", ConstantRole::View(ViewParameter::InverseView)),
        constant("FC_ShadowColor", camera.clone()),
    ];
    assert_eq!(state_lines(Ok(&graph), &skin.interface, &mut constants), 1);
    let lines: Vec<_> = constants.iter().map(|c| c.lines.clone()).collect();
    assert_eq!(lines[0], Some(MatrixLines::Columns));
    assert_eq!(lines[1], Some(MatrixLines::Rows));
    assert!(
        matches!(&lines[2], Some(MatrixLines::Unshown { why }) if why.contains("sums no product")),
        "{:?}",
        lines[2]
    );
    assert_eq!(constants[2].role, ConstantRole::View(ViewParameter::InverseView));
    assert_eq!((&constants[3].role, &lines[3]), (&camera, &None));
    // a graph that could not be built leaves every matrix unshown, with why
    let mut again = vec![constant(
        "VC_MatrixViewProj",
        ConstantRole::View(ViewParameter::ViewProjection),
    )];
    assert_eq!(state_lines(Err("a call"), &skin.interface, &mut again), 1);
    assert!(matches!(&again[0].lines, Some(MatrixLines::Unshown { why }) if why.contains("a call")));
}
