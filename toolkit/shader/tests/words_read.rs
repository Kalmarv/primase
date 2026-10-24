//! The constant-buffer words a program reads ([`primase_shader::WordsRead`]), from the value graph: word by word
//! however the program loads, and through writes the graph does not follow.
use std::collections::BTreeSet;

use primase_shader::flow::Values;
use primase_shader::{Program, ResourceId, ResourceKind};

/// The constant buffer at D3D register `register`.
fn buffer(program: &Program, register: u32) -> ResourceId {
    program
        .interface
        .resources
        .iter()
        .find(|r| {
            matches!(r.kind, ResourceKind::ConstantBuffer { .. })
                && r.provenance.d3d.is_some_and(|d| d.register == register)
        })
        .expect("the buffer")
        .id
}

#[test]
fn a_program_that_loads_whole_registers_reads_only_the_lanes_it_uses() {
    // the buffer at b4 holds 245 rows, the four clip rows (words 980-995), 16 rows and the camera's three lanes
    // (1060-1062); the program loads the camera's whole register, and the lane beside it (1063) is not read
    let program = primase_dxil::lift(include_bytes!(
        "../../../worlds/frostbite/tests/fixtures/tables_vs.dxil"
    ))
    .expect("dxc's program lifts");
    let reads = program.constant_reads().expect("the value graph");
    let pass = buffer(&program, 4);
    let expected: Vec<u32> = (980..996).chain(1060..1063).collect();
    assert_eq!(reads.words(pass).collect::<Vec<_>>(), expected);
    assert!(!reads.indexed(pass));
    assert!(reads.reads(pass, 1062 * 4, 4));
    assert!(!reads.reads(pass, 1063 * 4, 4));
    assert!(
        reads.reads(pass, 1063 * 4 - 2, 4),
        "a range that straddles a word read"
    );
    assert!(!reads.reads(pass, 996 * 4, 64 * 4), "the 16 rows between");
    let set = buffer(&program, 0);
    assert_eq!(reads.words(set).collect::<Vec<_>>(), [0]);
    // as rows of four lanes: the clip rows whole, the camera's row without its w
    assert_eq!(reads.rows(pass), [245, 246, 247, 248, 265]);
    assert_eq!(reads.lanes(pass, 265), [0, 1, 2]);
    assert_eq!(reads.lanes(pass, 246), [0, 1, 2, 3]);
    assert!(reads.lanes(pass, 250).is_empty());
    // the same by D3D bind point (space 0)
    assert_eq!(reads.buffer_at(0, 4), Some(pass));
    assert!(reads.reads_at(0, 4, 1062 * 4, 4));
    assert!(!reads.reads_at(0, 4, 1063 * 4, 4));
    assert!(!reads.reads_at(1, 4, 1062 * 4, 4), "no buffer at space 1");
}

#[test]
fn a_constant_that_reaches_only_a_storage_write_is_read() {
    // `scale` (b0, word 0) is used only in the value `raw.Store` writes
    let program = primase_dxil::lift(include_bytes!("../../dxil/tests/fixtures/compute_basic.dxil"))
        .expect("dxc's program lifts");
    let values = Values::of_program(&program).expect("the value graph");
    assert!(values.outputs().is_empty());
    assert!(!values.writes().is_empty());
    let reads = program.constant_reads().expect("the value graph");
    assert_eq!(reads.words(buffer(&program, 0)).collect::<Vec<_>>(), [0]);
}

#[test]
fn a_resource_the_program_never_names_is_unread() {
    // the program samples base_map; a second texture is declared beside it, as a lifter declares what the bytecode
    // lists, and nothing reads it
    let mut program =
        primase_dxil::lift(include_bytes!("fixtures/flow_value.dxil")).expect("dxc's program lifts");
    assert_eq!(program.unread_resources().expect("valid"), BTreeSet::new());
    let texture = program
        .interface
        .resources
        .iter()
        .find(|r| matches!(r.kind, ResourceKind::Texture { .. }))
        .expect("base_map")
        .clone();
    let mut global = program.module.global_variables[program.global(texture.id).expect("its global")].clone();
    let binding = global.binding.as_mut().expect("a binding");
    binding.binding += 1;
    global.name = Some("declared".into());
    let declared = program
        .module
        .global_variables
        .append(global, naga::Span::UNDEFINED);
    let id = ResourceId(
        program
            .interface
            .resources
            .iter()
            .map(|r| r.id.0)
            .max()
            .unwrap_or(0)
            + 1,
    );
    let mut resource = texture;
    resource.id = id;
    if let Some(d3d) = resource.provenance.d3d.as_mut() {
        d3d.register += 1;
    }
    program.interface.resources.push(resource);
    program.bindings.push((id, declared));
    assert_eq!(program.unread_resources().expect("valid"), BTreeSet::from([id]));
}

#[test]
fn a_dxbc_program_reads_the_lanes_it_computes_with() {
    // fxc's pixel program of one buffer at b0 (`f` row 0, `u` row 1, `i` row 2) that uses every lane of rows 0 and 1
    // and the x and y lanes of row 2
    let program = primase_dxbc::lift(include_bytes!("../../dxbc/tests/fixtures/modes.dxbc"))
        .expect("fxc's program lifts");
    let reads = program.constant_reads().expect("the value graph");
    let b0 = buffer(&program, 0);
    assert_eq!(reads.rows(b0), [0, 1, 2]);
    assert_eq!(reads.lanes(b0, 0), [0, 1, 2, 3]);
    assert_eq!(reads.lanes(b0, 1), [0, 1, 2, 3]);
    assert_eq!(reads.lanes(b0, 2), [0, 1]);
}
