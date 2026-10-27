//! The instance and matrix tables of a vertex program found by running it (`primase_frostbite::passes::buffer_tables`),
//! on the fixture programs of `tests/fixtures/tables_vs.hlsl`: one-word records, two-word records with the matrix in the
//! last word, and the programs that break the layout (rows read where the header is, a third buffer) refused by name.

#![allow(clippy::unwrap_used)]

use iridia_interface::{D3dClass, ResourceId, ResourceKind, ShaderFormat};
use primase_core::shader::Shader;
use primase_frostbite::passes::buffer_tables::{
    InstanceBase, MatrixLayout, SkinInputs, TableRoles, probe_tables, probe_tables_with,
};
use primase_frostbite::passes::clip_position::shader_clip_rows;

const ONE_WORD: &[u8] = include_bytes!("fixtures/tables_vs.dxil");
const TWO_WORDS: &[u8] = include_bytes!("fixtures/tables_vs.record2.dxil");
const NO_HEADER: &[u8] = include_bytes!("fixtures/tables_vs.no_header.dxil");
const THREE_BUFFERS: &[u8] = include_bytes!("fixtures/tables_vs.three_buffers.dxil");
const SKINNED: &[u8] = include_bytes!("fixtures/tables_vs.skinned.dxil");
const SKINNED_WEIGHTED: &[u8] = include_bytes!("fixtures/tables_vs.skinned_weighted.dxil");
const SKINNED_STRIDE_TWO: &[u8] = include_bytes!("fixtures/tables_vs.skinned_stride2.dxil");
const SKINNED_BIT_18: &[u8] = include_bytes!("fixtures/tables_vs.skinned_bit18.dxil");
const SKINNED_SECOND_BLOCK: &[u8] = include_bytes!("fixtures/tables_vs.skinned_second_block.dxil");

fn shader(bytes: &[u8]) -> Shader {
    let program = primase_dxil::lift(bytes).expect("the fixture lifts");
    Shader {
        format: ShaderFormat::Dxil,
        bytecode: bytes.to_vec(),
        interface: program.interface,
    }
}

/// The constant buffer of the shader bound at shader register `register`.
fn constants_at(s: &Shader, register: u32) -> ResourceId {
    s.interface
        .resources
        .iter()
        .find(|r| {
            matches!(r.kind, ResourceKind::ConstantBuffer { .. })
                && r.provenance
                    .d3d
                    .is_some_and(|d| d.class == D3dClass::ConstantBuffer && d.register == register)
        })
        .expect("the fixture has the constant buffer")
        .id
}

/// The buffer of the shader bound at shader register `register`.
fn buffer_at(s: &Shader, register: u32) -> ResourceId {
    s.interface
        .resources
        .iter()
        .find(|r| {
            matches!(r.kind, ResourceKind::Buffer(_))
                && r.provenance
                    .d3d
                    .is_some_and(|d| d.class == D3dClass::ShaderResource && d.register == register)
        })
        .expect("the fixture has the buffer")
        .id
}

fn probed(bytes: &[u8]) -> Result<TableRoles, String> {
    let s = shader(bytes);
    let (_, rows) = shader_clip_rows(&s).expect("the fixture has the camera-relative clip form");
    probe_tables(&s, rows)
}

#[test]
fn the_instance_and_matrix_tables_are_found_by_running_the_program() {
    let s = shader(ONE_WORD);
    assert_eq!(
        probed(ONE_WORD),
        Ok(TableRoles {
            instances: buffer_at(&s, 0),
            stride: 1,
            matrix_start: 0,
            matrices: buffer_at(&s, 67),
            layout: MatrixLayout::Plain,
            base: Some(InstanceBase {
                buffer: constants_at(&s, 0),
                offset: 0,
            }),
        })
    );
    // two words a record, the matrix in the last
    let s = shader(TWO_WORDS);
    assert_eq!(
        probed(TWO_WORDS),
        Ok(TableRoles {
            instances: buffer_at(&s, 0),
            stride: 2,
            matrix_start: 1,
            matrices: buffer_at(&s, 67),
            layout: MatrixLayout::Plain,
            base: Some(InstanceBase {
                buffer: constants_at(&s, 0),
                offset: 0,
            }),
        })
    );
}

#[test]
fn a_program_that_does_not_read_its_tables_that_way_is_refused_by_name() {
    // the rows read where the header is: the elements it reads after the header are two, not three rows
    let err = probed(NO_HEADER).unwrap_err();
    assert!(err.contains("not three rows directly after it"), "{err}");
    // a third buffer
    let err = probed(THREE_BUFFERS).unwrap_err();
    assert!(
        err.contains("more than two buffers") || err.contains("not the position given"),
        "{err}"
    );
}

/// The inputs of the skinned fixture the mesh's weights and joint indices feed: TEXCOORD1 and TEXCOORD0.
fn skin_inputs(s: &Shader) -> SkinInputs {
    let at = |index: u32| {
        s.interface
            .inputs
            .iter()
            .position(|i| i.label.name.eq_ignore_ascii_case("TEXCOORD") && i.label.index == index)
            .expect("the skinned fixture has the input")
    };
    SkinInputs {
        weights: at(1),
        joints: at(0),
    }
}

fn probed_skinned(bytes: &[u8]) -> Result<TableRoles, String> {
    let s = shader(bytes);
    let (_, rows) = shader_clip_rows(&s).expect("the fixture has the camera-relative clip form");
    probe_tables_with(&s, rows, Some(skin_inputs(&s)))
}

/// A program that skins, in the shape of the hat's: four joints at four weights, the header followed by an element of
/// translation (added after the weighted sum), then each joint's three rows three elements after it. Run with its weights
/// at the position like every other float input, the blend is four times the position's w and the position does not come
/// back; run with the whole weight on the first joint, whose index is zero, and the identity rows where the elements it
/// reads say they are, the tables are found, and every part of the layout is proved by a run.
#[test]
fn a_program_that_skins_has_its_layout_read_from_what_it_reads_and_proved_by_runs() {
    let s = shader(SKINNED);
    let err = probed(SKINNED).unwrap_err();
    assert!(err.contains("not the position given"), "{err}");
    assert_eq!(
        probed_skinned(SKINNED),
        Ok(TableRoles {
            instances: buffer_at(&s, 0),
            stride: 1,
            matrix_start: 0,
            matrices: buffer_at(&s, 67),
            layout: MatrixLayout::Skinned {
                prefix: 3,
                joint_stride: 3
            },
            base: Some(InstanceBase {
                buffer: constants_at(&s, 0),
                offset: 0,
            }),
        })
    );
}

/// Each part of the skinned layout that a run proves is refused by name by a program that breaks it: a translation
/// weighted with each influence, joints two elements apart (closer than their three rows), a bit 18 that moves nothing.
#[test]
fn a_skinned_program_that_breaks_a_proved_part_is_refused_by_the_run_that_shows_it() {
    let err = probed_skinned(SKINNED_WEIGHTED).unwrap_err();
    assert!(err.contains("not the weighted position plus the vector"), "{err}");
    let err = probed_skinned(SKINNED_STRIDE_TWO).unwrap_err();
    assert!(err.contains("fewer than a joint's three rows"), "{err}");
    let err = probed_skinned(SKINNED_BIT_18).unwrap_err();
    assert!(err.contains("with the header's bit 18 set"), "{err}");
    // a block also read where the offset flag does not put it (the hat's 6848-byte program): refused by the flag's run, which
    // says the extra elements are that second block and that its role is unknown
    let err = probed_skinned(SKINNED_SECOND_BLOCK).unwrap_err();
    assert!(err.contains("with the header's offset flag set to 2"), "{err}");
    assert!(
        err.contains("a second block no output of the program was seen to depend on"),
        "{err}"
    );
}
