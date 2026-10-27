//! Every disc submesh joins independently stored bank IDs. The ID-zero rule is
//! explicitly inferred from the exhaustive nonzero namespace join, not a native oracle.
mod inputs;

use inputs::{Game, install};
use rme_formats::{
    library::{BankEvidence, MaterialLibrary},
    pak::Pak,
    scene,
};
use std::path::{Path, PathBuf};

fn archives(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let p = entry.expect("directory entry").path();
        if p.is_dir() {
            archives(&p, out);
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pak")) {
            out.push(p);
        }
    }
}

#[test]
fn every_submesh_bank_and_zero_namespace_are_counted() {
    let Some(root) = install(Game::Shred) else { return };
    let usrdir = root.join("PS3_GAME/USRDIR");
    let banks = Pak::open(usrdir.join("database/materials.pak")).unwrap();
    let library = MaterialLibrary::new(&banks, &[]).unwrap();
    let mut paths = Vec::new();
    archives(&usrdir, &mut paths);
    paths.sort();
    let (mut quartets, mut refs, mut inferred) = (0, 0, 0);
    let mut by_bank = [0; 3];
    let mut zeros = [0; 3];
    for path in paths {
        let pak = Pak::open(&path).unwrap();
        for (name, entries) in scene::quartets(&pak).unwrap() {
            quartets += 1;
            let scene = scene::decode_quartet(&pak, entries).unwrap();
            let namespace = name.split(|&b| b == b'\\' || b == b'/').next().unwrap();
            let expected = match namespace {
                b"levels" => ("environments", 0),
                b"characters" => ("characters", 1),
                b"actors" => ("actors", 2),
                other => panic!("unknown quartet namespace {other:?}"),
            };
            for model in scene.models {
                for sub in model.submeshes {
                    let resolution = library.resolve(sub.material, Some(namespace));
                    let selected = resolution.selected.unwrap_or_else(|| {
                        panic!("{} {:?} id{} unresolved", path.display(), name, sub.material)
                    });
                    assert_eq!(selected.bank, expected.0);
                    refs += 1;
                    by_bank[expected.1] += 1;
                    if sub.material == 0 {
                        assert_eq!(resolution.candidates.len(), 3);
                        assert_eq!(resolution.evidence, Some(BankEvidence::InferredQuartetNamespace));
                        inferred += 1;
                        zeros[expected.1] += 1;
                    } else {
                        assert_eq!(resolution.candidates.len(), 1);
                        assert_eq!(resolution.evidence, Some(BankEvidence::UniqueId));
                    }
                }
            }
        }
    }
    eprintln!(
        "SHRED bank join: {quartets} quartets / {refs} references; {inferred} inferred zero refs; env/char/actor={by_bank:?}; zeros={zeros:?}"
    );
    assert_eq!((quartets, refs, inferred), (657, 35114, 645));
    assert_eq!(by_bank, [27183, 1526, 6405]);
    assert_eq!(zeros, [514, 5, 126]);
}
