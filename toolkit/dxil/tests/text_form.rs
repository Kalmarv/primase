//! A program WGSL cannot express keeps a text form: the module as naga's IR dump, under a line saying it is not WGSL and why.

#![allow(clippy::unwrap_used)]

use primase_dxil::lift;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

/// `image_atomic_results.dxil` reads the old value of texture atomics, which WGSL's texture atomics do not return.
#[test]
fn a_program_wgsl_cannot_write_is_dumped_as_ir() {
    let program = lift(&fixture("image_atomic_results.dxil")).unwrap();
    assert!(primase_shader::emit::wgsl(&program).is_err());
    let text = program.text().unwrap();
    let label = text
        .lines()
        .find(|l| l.starts_with("// the module below is naga's IR (Debug), not WGSL"))
        .expect("the label line");
    assert!(label.contains("texture atomic result"), "{label}");
    assert!(text.contains("ImageAtomic"), "the dump holds the statement");
    assert_eq!(text, program.text().unwrap(), "the same program, the same text");
}

#[test]
fn a_program_wgsl_can_write_is_written_as_wgsl() {
    let text = lift(&fixture("pixel_ops.dxil")).unwrap().text().unwrap();
    assert!(!text.contains("not WGSL"), "{text}");
    assert!(text.contains("fn "), "WGSL functions");
}
