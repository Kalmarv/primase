//! The version gate (`primase_shader::gate`): what the lifter makes of its committed fixtures is hashed under
//! `LIFTER_VERSION` and `PROGRAM_VERSION` in `tests/lifted.hashes`. A change to the output without a bump of the version that
//! names it fails here, and so does a bump without the table of that version. The current table is written to the scratch root
//! (`dxbc-lifted.hashes`, the path is in the failure) when they differ; it replaces the committed one in the commit that bumps the
//! version.
use std::path::Path;

use primase_dxbc::{LIFTER_VERSION, lift};
use primase_shader::gate;

#[test]
fn what_the_lifter_makes_of_its_fixtures_is_the_versions() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut names: Vec<String> = std::fs::read_dir(dir.join("fixtures"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".dxbc"))
        .collect();
    names.sort();
    let outcomes: Vec<_> = names
        .into_iter()
        .map(|name| {
            let bytes = std::fs::read(dir.join("fixtures").join(&name)).unwrap();
            let outcome = lift(&bytes)
                .map_err(|e| e.to_string())
                .and_then(|p| p.text().map_err(|e| format!("not written: {e}")));
            (name, outcome)
        })
        .collect();
    let current = gate::table("primase-dxbc", LIFTER_VERSION, &outcomes);
    let committed = std::fs::read_to_string(dir.join("lifted.hashes")).unwrap_or_default();
    if let Err(what) = gate::check(&committed, &current) {
        let new = primase_testdata::scratch().join("dxbc-lifted.hashes");
        std::fs::write(&new, &current).unwrap();
        panic!("{what}; the current table is {}", new.display());
    }
}
