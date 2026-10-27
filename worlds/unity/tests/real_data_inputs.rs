//! Every real-data query this crate's sources write is declared in tests/inputs/mod.rs, and the lists of its
//! inputs agree with it.
mod inputs;

#[test]
fn real_data_inputs_are_declared() {
    primase_testdata::check_declared(&inputs::REGISTRY, env!("CARGO_MANIFEST_DIR"));
}
