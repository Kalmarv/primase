//! The instance and matrix tables of a vertex program found by running it (`primase_frostbite::passes::buffer_tables`),
//! on the fixture programs of `tests/fixtures/tables_vs.hlsl`: one-word records, two-word records with the matrix in the
//! last word, and the programs that break the layout (rows read where the header is, a third buffer) refused by name.

#![allow(clippy::unwrap_used)]

use primase_core::shader::{D3dClass, ResourceId, ResourceKind, Shader, ShaderFormat};
use primase_frostbite::passes::buffer_tables::{InstanceBase, TableRoles, probe_tables};
use primase_frostbite::passes::clip_position::shader_clip_rows;

const ONE_WORD: &[u8] = include_bytes!("fixtures/tables_vs.dxil");
const TWO_WORDS: &[u8] = include_bytes!("fixtures/tables_vs.record2.dxil");
const NO_HEADER: &[u8] = include_bytes!("fixtures/tables_vs.no_header.dxil");
const THREE_BUFFERS: &[u8] = include_bytes!("fixtures/tables_vs.three_buffers.dxil");

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
            base: Some(InstanceBase {
                buffer: constants_at(&s, 0),
                offset: 0,
            }),
        })
    );
}

#[test]
fn a_program_that_does_not_read_its_tables_that_way_is_refused_by_name() {
    // the rows read where the header is: the identity rows are one element later than it looks for them
    let err = probed(NO_HEADER).unwrap_err();
    assert!(err.contains("not the position given"), "{err}");
    // a third buffer
    let err = probed(THREE_BUFFERS).unwrap_err();
    assert!(
        err.contains("more than two buffers") || err.contains("not the position given"),
        "{err}"
    );
}
