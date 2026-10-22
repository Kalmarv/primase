//! Registering the crate's passes builds every pass from its example options (a register call panics otherwise).

#[test]
fn every_registered_example_builds() {
    let mut registry = primase_core::pass::PassRegistry::new();
    primase_anvil::register(&mut registry);
}
