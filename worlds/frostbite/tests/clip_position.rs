//! The camera-relative form of a vertex program's clip position (`primase_frostbite::passes::clip_position`), on the
//! fixture programs of `tests/fixtures/camera_relative_vs.hlsl`: the form is found at the words it lies at, each way the
//! graph departs from it is refused by name, and the roles are cut out of the unknown constants that cover the words.

#![allow(clippy::unwrap_used)]

use iridia_interface::{
    ConstantBinding, ConstantRole, D3dClass, MatrixLines, ResourceId, ResourceKind, ShaderFormat,
    ViewParameter,
};
use primase_core::shader::Shader;
use primase_frostbite::passes::clip_position::{ClipRows, shader_clip_rows, state_clip_roles};

const FORM: &[u8] = include_bytes!("fixtures/camera_relative_vs.dxil");
const NO_CAMERA: &[u8] = include_bytes!("fixtures/camera_relative_vs.no_camera.dxil");
const WORLD_USES_CAMERA: &[u8] = include_bytes!("fixtures/camera_relative_vs.world_uses_camera.dxil");
const OTHER_LAST_ROW: &[u8] = include_bytes!("fixtures/camera_relative_vs.other_last_row.dxil");

fn shader(bytes: &[u8]) -> Shader {
    let program = primase_dxil::lift(bytes).expect("the fixture lifts");
    Shader {
        format: ShaderFormat::Dxil,
        bytecode: bytes.to_vec(),
        interface: program.interface,
    }
}

/// The constant buffer at register 4 of the shader and its size in bytes.
fn pass_buffer(shader: &Shader) -> (ResourceId, u32) {
    shader
        .interface
        .resources
        .iter()
        .find_map(|r| match &r.kind {
            ResourceKind::ConstantBuffer { layout }
                if r.provenance
                    .d3d
                    .is_some_and(|d| d.class == D3dClass::ConstantBuffer && d.register == 4) =>
            {
                Some((r.id, layout.size))
            }
            _ => None,
        })
        .expect("the fixture has the pass buffer")
}

#[test]
fn rows_and_a_camera_are_found_at_the_words_the_program_reads() {
    let s = shader(FORM);
    let (buffer, _) = pass_buffer(&s);
    assert_eq!(
        shader_clip_rows(&s),
        Ok((
            buffer,
            ClipRows {
                rows: 980,
                camera: 1060
            }
        ))
    );
}

#[test]
fn each_departure_from_the_form_is_refused_by_name() {
    for (bytes, why) in [
        (NO_CAMERA, "not a word times (world - camera word)"),
        (WORLD_USES_CAMERA, "the world position is made of the word"),
        (OTHER_LAST_ROW, "reads other rows or another camera than lane 0"),
    ] {
        let err = shader_clip_rows(&shader(bytes)).unwrap_err();
        assert!(err.contains(why), "{why}: {err}");
    }
    // a program that is not DXIL, or not a vertex program, is refused before it is lifted
    let mut not_dxil = shader(FORM);
    not_dxil.format = ShaderFormat::Dxbc;
    assert!(shader_clip_rows(&not_dxil).unwrap_err().contains("not DXIL"));
}

#[test]
fn the_roles_are_cut_out_of_the_unknown_constants_over_the_words() {
    let s = shader(FORM);
    let (buffer, size) = pass_buffer(&s);
    let (_, rows) = shader_clip_rows(&s).unwrap();
    let unknown = |offset: u32, size: u32| ConstantBinding {
        resource: buffer,
        offset,
        size,
        name: None,
        role: ConstantRole::Unknown {
            why: "the engine fills the pass binding's constants (b4): no role resolved".into(),
        },
        lines: None,
    };
    // the matrix's lines are its rows: each lane of the clip position is one line's words times the position
    let role = |offset: u32, size: u32, role: ViewParameter| ConstantBinding {
        resource: buffer,
        offset,
        size,
        name: None,
        lines: (role == ViewParameter::CameraRelativeViewProjection).then_some(MatrixLines::Rows),
        role: ConstantRole::View(role),
    };
    // one unknown constant over the whole buffer: cut into five
    let mut constants = vec![unknown(0, size)];
    assert!(state_clip_roles(&mut constants, buffer, rows));
    assert_eq!(
        constants,
        [
            unknown(0, 3920),
            role(3920, 64, ViewParameter::CameraRelativeViewProjection),
            unknown(3984, 256),
            role(4240, 12, ViewParameter::CameraPosition),
            unknown(4252, size - 4252),
        ]
    );
    // unknown constants that end inside the words are cut at the words and the pieces beside them are the same ones
    let mut constants = vec![unknown(0, 3950), unknown(3950, 300), unknown(4250, size - 4250)];
    assert!(state_clip_roles(&mut constants, buffer, rows));
    assert_eq!(
        constants,
        [
            unknown(0, 3920),
            role(3920, 64, ViewParameter::CameraRelativeViewProjection),
            unknown(3984, 256),
            role(4240, 12, ViewParameter::CameraPosition),
            unknown(4252, size - 4252),
        ]
    );
    // a stated constant over any of the words: nothing is changed
    let mut stated = vec![unknown(0, 4000), unknown(4000, size - 4000)];
    stated[0].role = ConstantRole::Material(vec![0; 1]);
    let before = stated.clone();
    assert!(!state_clip_roles(&mut stated, buffer, rows));
    assert_eq!(stated, before);
    // words the constants do not cover: nothing is changed
    let mut short = vec![unknown(0, 3950)];
    assert!(!state_clip_roles(&mut short, buffer, rows));
    assert_eq!(short, [unknown(0, 3950)]);
}
