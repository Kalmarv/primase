#![allow(clippy::unwrap_used)]
//! Lifting the fixtures (dxc 1.7, default optimisation and -Od): each lifts, validates and writes WGSL.
use primase_dxil::lift;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

#[test]
fn every_fixture_lifts() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".dxil"))
        .collect();
    names.sort();
    let mut failures = Vec::new();
    for name in &names {
        match lift(&fixture(name)) {
            Err(e) => failures.push(format!("{name}: lift: {e}")),
            Ok(p) => {
                if let Err(problems) = p.validate() {
                    failures.push(format!(
                        "{name}: validate: {}",
                        problems
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join("; ")
                    ));
                } else if let Err(e) = primase_shader::emit::wgsl(&p) {
                    failures.push(format!("{name}: wgsl: {e}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
