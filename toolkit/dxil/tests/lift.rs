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

/// Fixtures whose program computes with a double (a 64-bit float made from or split into two 32-bit words): WGSL has no 64-bit
/// float, so the lifter refuses each by name. The refusal is the outcome asserted, not a skipped file.
const DOUBLE_FIXTURES: [&str; 1] = ["structured_tail.dxil"];

/// Fixtures that are a library of several programs, whose programs are the per-export fixtures beside them.
const WHOLE_LIBRARIES: [&str; 1] = ["ray_subobjects.dxil"];

#[test]
fn every_fixture_lifts() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".dxil"))
        // a whole library (every program of it, with its subobjects) is not one program: the lifter takes a library of
        // one, and the per-export containers of it are lifted instead
        .filter(|n| n != "ray_subobjects.dxil")
        .collect();
    names.sort();
    let mut failures = Vec::new();
    for name in &names {
        // a whole library holds several programs: `lift` refuses it by name, and each program of it is a container of
        // its own (ray_subobjects.<export>.dxil), which this loop lifts
        if WHOLE_LIBRARIES.contains(&name.as_str()) {
            match lift(&fixture(name)) {
                Err(e) if e.to_string().contains("not exactly one entry point") => {}
                other => failures.push(format!(
                    "{name}: expected the refusal of a whole library, got {other:?}"
                )),
            }
            continue;
        }
        if DOUBLE_FIXTURES.contains(&name.as_str()) {
            match lift(&fixture(name)) {
                Err(e) if e.to_string().contains("double-precision floats are not lifted") => {}
                other => failures.push(format!(
                    "{name}: expected the refusal of double-precision floats, got {other:?}"
                )),
            }
            continue;
        }
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
                } else {
                    // a program of a stage with no WGSL stage is written as its functions
                    let written = if iridia::stage_functions(&p.interface).is_ok_and(|f| f.is_some()) {
                        iridia::emit::wgsl_functions(&p)
                    } else {
                        iridia::emit::wgsl(&p)
                    };
                    match written {
                        Ok(_) => {}
                        // WebGPU has no ray tracing pipeline: those programs are refused by name
                        Err(iridia::ProgramError::Unsupported { detail, .. })
                            if detail.contains("ray tracing pipeline") => {}
                        // WGSL's texture atomics return no old value and have no exchange: naga's writer refuses those by name
                        Err(iridia::ProgramError::Unsupported { detail, .. })
                            if detail.contains("texture atomic result")
                                || detail.contains("texture atomic exchange") => {}
                        Err(e) => failures.push(format!("{name}: wgsl: {e}")),
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
