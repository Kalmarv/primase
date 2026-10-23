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
                    let written = if primase_shader::stage_functions(&p.interface).is_ok_and(|f| f.is_some())
                    {
                        primase_shader::emit::wgsl_functions(&p)
                    } else {
                        primase_shader::emit::wgsl(&p)
                    };
                    match written {
                        Ok(_) => {}
                        // naga's WGSL writer does not write ray queries: the lifted program is what is checked
                        Err(primase_shader::ProgramError::Unsupported { detail, .. })
                            if detail.contains("ray queries") => {}
                        // WebGPU has no ray tracing pipeline: those programs are refused by name
                        Err(primase_shader::ProgramError::Unsupported { detail, .. })
                            if detail.contains("ray tracing pipeline") => {}
                        // naga's image atomics return no old value: a program that reads it is refused by name
                        Err(primase_shader::ProgramError::Unsupported { detail, .. })
                            if detail.contains("reads the old value of an image atomic operation") => {}
                        Err(e) => failures.push(format!("{name}: wgsl: {e}")),
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
