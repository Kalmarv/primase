#![allow(clippy::unwrap_used)]
//! Containers a damaged file could hold, made from the committed fixtures by changing one byte (found by mutating every fixture
//! and shrinking each panic of the lifter to the one byte that causes it): each is refused by name, never a panic. The oracle
//! is the format's own rules (a value, a block or a callee the function has; the arguments an operation takes).
use primase_dxil::lift;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn refusal(bytes: &[u8]) -> String {
    match lift(bytes) {
        Ok(_) => panic!("the program was lifted"),
        Err(e) => e.detail,
    }
}

/// The fixture with the byte at `at`, which is `was`, made `now`.
fn changed(name: &str, at: usize, was: u8, now: u8) -> Vec<u8> {
    let mut bytes = fixture(name);
    assert_eq!(bytes[at], was, "{name} is not the fixture the case was made from");
    bytes[at] = now;
    bytes
}

#[test]
fn an_unchanged_fixture_still_lifts() {
    lift(&fixture("arrays.66.dxil")).unwrap();
}

#[test]
fn a_value_the_function_does_not_have_is_refused() {
    // an operand whose relative id wraps past the function's values
    let bytes = changed("arrays.66.dxil", 4492, 0x06, 0x7a);
    assert!(refusal(&bytes).contains("of 84 values"), "{}", refusal(&bytes));
}

#[test]
fn an_instruction_after_the_last_terminator_is_refused() {
    let bytes = changed("buffer_arrays.66.dxil", 4184, 0x00, 0x01);
    assert!(refusal(&bytes).contains("an instruction after the last terminator"));
}

#[test]
fn a_dx_op_called_with_the_wrong_number_of_arguments_is_refused() {
    // the opcode of a call changed to one that takes more (a binary operation read as ternary), fewer, and a matrix query
    for (name, at, was, now, expected) in [
        (
            "waves.od.dxil",
            3767,
            0x7a,
            0x34,
            "dx.op 40 called with 2 arguments, where it takes 3",
        ),
        (
            "ray_subobjects.closest_hit.dxil",
            3434,
            0x56,
            0x12,
            "dx.op 10 called with 1 arguments, where it takes 2",
        ),
        (
            "ray_query_grid.dxil",
            4304,
            0xe8,
            0xbe,
            "dx.op 188 called with 2 arguments, where it takes 4",
        ),
        (
            "lane_2d.dxil",
            3057,
            0x3d,
            0x1a,
            "dx.op 42 called with 2 arguments, where it takes 3",
        ),
        (
            "image_array_store.od.dxil",
            3093,
            0x0a,
            0x8a,
            "dx.op 98 called with 1 arguments, where it takes 2",
        ),
    ] {
        let bytes = changed(name, at, was, now);
        assert!(refusal(&bytes).contains(expected), "{name}: {}", refusal(&bytes));
    }
}

#[test]
fn a_global_that_is_not_a_global_is_refused() {
    let bytes = changed("ray_tracing.any_hit.od.dxil", 3228, 0x00, 0xf2);
    assert!(refusal(&bytes).contains("a global that is not a global"));
}

#[test]
fn a_graphics_program_without_its_signature_part_is_refused() {
    // the directory entry of the input signature renamed
    let mut bytes = fixture("arrays.66.dxil");
    let at = bytes
        .windows(4)
        .position(|w| w == b"ISG1")
        .expect("the container has an input signature");
    bytes[at] = b'X';
    assert!(refusal(&bytes).contains("without its ISG1 part"));
}
