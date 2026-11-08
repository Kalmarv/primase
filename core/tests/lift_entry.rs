//! Every DXBC or DXIL container primase lifts onto the IR goes through `primase_core::shader::lift_dxbc` / `lift_dxil` (and their
//! `_with` forms), which prepare the program (`iridia_passes::prepare`) as the IR's analyses and executors expect. A raw lift can hold
//! a control flow graph they refuse, so a source file that calls the iridia lifters directly is a site that may skip it. A caller that
//! really needs the raw lift is listed here with its reason.

use std::path::{Path, PathBuf};

/// Files that call the iridia lifters themselves, each with why.
const RAW_LIFT_CALLERS: &[(&str, &str)] = &[
    ("core/src/shader/lift.rs", "the entry points themselves"),
    ("core/tests/lift_entry.rs", "this test names the calls it looks for"),
];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name != "target" && name != ".git" {
                sources(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_source_lifts_a_container_without_preparing_it() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for dir in ["core", "worlds", "formats", "toolkit"] {
        sources(&root.join(dir), &mut files);
    }
    let mut offenders = Vec::new();
    for file in files {
        let relative = file
            .strip_prefix(&root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace(char::from(92), "/");
        // gta and shaunwhite still lift with the older programs; they move to the IR in their own slices and then to the entry points
        if relative.starts_with("worlds/gta/") || relative.starts_with("worlds/shaunwhite/") {
            continue;
        }
        if RAW_LIFT_CALLERS.iter().any(|(path, _)| *path == relative) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for pattern in ["iridia_dxbc::ir::lift", "iridia_dxil::ir::lift"] {
            if text.contains(pattern) {
                offenders.push(format!("{relative}: {pattern}"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "lift with primase_core::shader::lift_dxbc / lift_dxil, or list the file in RAW_LIFT_CALLERS with why:\n{}",
        offenders.join("\n")
    );
}
