#![allow(clippy::unwrap_used)]
//! The committed fixtures (Microsoft's fxc output for the HLSL beside each; `.od` compiled with `/Od`): each one's
//! container, program type and every declaration and instruction of its program read to the end.
use std::path::Path;

#[test]
fn every_fixture_decodes() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut n = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if !path.extension().is_some_and(|e| e == "dxbc") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let name = path.display();
        let directory = dxbc_reader::parts(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let kind =
            dxbc_reader::reflection::program_type(&directory).unwrap_or_else(|e| panic!("{name}: {e}"));
        let words = dxbc_reader::code(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let program = dxbc_reader::decode(&words).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(program.kind, kind, "{name}");
        assert!(!program.instructions.is_empty(), "{name}: no instructions");
        n += 1;
    }
    assert!(n >= 13, "{n} fixtures");
}
