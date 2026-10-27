#![allow(clippy::unwrap_used)]
//! The DXIL reader on the control-flow and memory fixtures (dxc 1.7, default optimisation and -Od): a switch is read as
//! a switch, a local array as an alloca with getelementptr, loads and stores; and every committed fixture reads.
use dxil_reader::module::{Module, Op};

fn module(name: &str) -> Module {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let bytes = std::fs::read(path).unwrap();
    let chunks = dxcontainer::chunks(&bytes).unwrap();
    let dxil = chunks.iter().find(|c| &c.kind == b"DXIL").unwrap();
    let part = dxil_reader::program(dxil.data).unwrap();
    let blocks = dxil_reader::bitstream::read(part.bitcode, *b"BC\xc0\xde").unwrap();
    Module::read(&blocks).unwrap()
}

fn count(m: &Module, pred: impl Fn(&Op, usize) -> bool) -> usize {
    m.functions
        .iter()
        .flat_map(|f| f.instructions.iter())
        .filter(|i| pred(&i.op, i.block))
        .count()
}

#[test]
fn a_switch_is_a_switch_with_its_cases() {
    for name in ["switch.dxil", "switch.od.dxil"] {
        let m = module(name);
        assert_eq!(count(&m, |op, _| matches!(op, Op::Switch { .. })), 2, "{name}");
        let widest = m
            .functions
            .iter()
            .flat_map(|f| &f.instructions)
            .filter_map(|i| match &i.op {
                Op::Switch { cases, .. } => Some(cases.len()),
                _ => None,
            })
            .max();
        assert_eq!(
            widest,
            Some(4),
            "{name}: the first switch has the labels 0, 1, 2 and 5"
        );
    }
}

#[test]
fn a_local_array_is_memory() {
    for name in ["arrays.dxil", "arrays.od.dxil"] {
        let m = module(name);
        assert_eq!(count(&m, |op, _| matches!(op, Op::Alloca { .. })), 1, "{name}");
        assert!(count(&m, |op, _| matches!(op, Op::Store { .. })) >= 4, "{name}");
        assert!(count(&m, |op, _| matches!(op, Op::Gep { .. })) >= 4, "{name}");
        // the constant table is a global with an initial value
        let tables = m
            .values
            .iter()
            .filter(|v| {
                matches!(
                    v,
                    dxil_reader::module::Value::Global {
                        init: Some(_),
                        constant: true,
                        ..
                    }
                )
            })
            .count();
        assert!(tables >= 1, "{name}: {tables} constant tables");
    }
}

#[test]
fn every_fixture_reads() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut n = 0;
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "dxil") {
            module(&p.file_name().unwrap().to_string_lossy());
            n += 1;
        }
    }
    assert!(n >= 17, "{n} fixtures");
}
