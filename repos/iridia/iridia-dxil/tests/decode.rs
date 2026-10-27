#![allow(clippy::unwrap_used)]
//! The control-flow fixtures' loops (dxc 1.7, default optimisation and -Od), as `dxil_reader` reads them and as the lift
//! takes them: backward branches in the module, structured loops in the lifted program.
use dxil_reader::module::{Module, Op};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

fn module(bytes: &[u8]) -> Module {
    let chunks = dxil_reader::parts(bytes).unwrap();
    dxil_reader::module(&chunks, b"DXIL").unwrap().unwrap()
}

#[test]
fn loops_are_backward_branches() {
    for name in ["loops.dxil", "loops.od.dxil"] {
        let bytes = fixture(name);
        let m = module(&bytes);
        let back = m
            .functions
            .iter()
            .flat_map(|f| f.instructions.iter())
            .filter(|i| match &i.op {
                Op::Branch { target } => *target <= i.block,
                Op::CondBranch { then, otherwise, .. } => *then <= i.block || *otherwise <= i.block,
                _ => false,
            })
            .count();
        assert!(back >= 3, "{name}: {back} backward branches (three loops)");
        // the lift takes them as structured loops
        let program = iridia_dxil::lift(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        program.validate().unwrap_or_else(|p| panic!("{name}: {p:?}"));
    }
}
