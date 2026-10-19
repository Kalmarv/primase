//! Every pass the ReSkate target registers prints every setting it was made with: made from its own `options()`, it
//! prints them back the same.

#![allow(clippy::unwrap_used)]

use primase_core::pass::PassRegistry;

#[test]
fn every_reskate_pass_reads_back_what_it_prints() {
    let mut registry = PassRegistry::new();
    primase_reskate::register(&mut registry);
    let names: Vec<&str> = registry.names().collect();
    assert!(names.len() >= 20, "{names:?}");
    for name in names {
        let pass = registry.make_registered(name).unwrap();
        let printed = pass.options();
        let again = registry
            .make(name, &printed)
            .unwrap_or_else(|e| panic!("{name}: its options {printed:?} do not make it again: {e}"));
        assert_eq!(again.options(), printed, "{name}");
    }
}
