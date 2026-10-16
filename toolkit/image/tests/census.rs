use sha2::{Digest, Sha256};
// Real scene PNGs against Pillow 11, with deterministic PNG write/read roundtrips.
use primase_core::gate::{GateReport, Tolerance};
use primase_image::{Samples, read_png, write_png};
use primase_testdata::real_data;
use std::{
    collections::BTreeMap,
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
fn walk(path: &Path, inside: bool, out: &mut Vec<PathBuf>) {
    let inside = inside || path.file_name().is_some_and(|x| x == "textures");
    let mut paths: Vec<_> = std::fs::read_dir(path)
        .expect("directory")
        .map(|x| x.expect("entry").path())
        .collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            walk(&p, inside, out)
        } else if inside && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("png")) {
            out.push(p)
        }
    }
}
#[test]
fn scene_pngs() {
    let mut configured = 0;
    let mut checked = 0;
    for var in ["PRIMASE_SCENES_GTA", "PRIMASE_SCENES_SEKIRO", "PRIMASE_MAPS_GTA"] {
        let Some(root) = real_data(var) else { continue };
        let mut paths = Vec::new();
        walk(&root, false, &mut paths);
        configured += 1;
        checked += paths.len();
        if paths.is_empty() {
            println!("{var}: 0 texture PNGs (no pixel gate performed)");
            continue;
        }
        let mut python = Command::new("python")
            .arg("-I")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/oracles/worker.py"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Pillow");
        let mut input = python.stdin.take().expect("stdin");
        let mut output = BufReader::new(python.stdout.take().expect("stdout"));
        let mut gate = GateReport::new(format!("{var} PNG vs Pillow"), Tolerance::Exact);
        let mut roundtrip = GateReport::new(format!("{var} PNG pixels write/read"), Tolerance::Exact);
        let mut metadata = BTreeMap::new();
        let path_count = paths.len();
        let mut contents = std::collections::BTreeSet::<[u8; 32]>::new();
        for p in paths {
            let data = std::fs::read(&p).expect("PNG bytes");
            if !contents.insert(Sha256::digest(&data).into()) {
                continue;
            }
            let png = read_png(&data).expect("decode PNG");
            for kind in png.ignored_ancillary {
                *metadata
                    .entry(String::from_utf8_lossy(&kind).into_owned())
                    .or_insert(0u64) += 1;
            }
            let image = png.image;
            let encoded = write_png(&image).expect("encode");
            let restored = read_png(&encoded).expect("decode encoded").image;
            if image == restored {
                roundtrip.record(p.display().to_string(), 0.);
            } else {
                roundtrip.fail(p.display().to_string(), "pixel roundtrip");
            }
            writeln!(input, "{}", p.display()).expect("request");
            input.flush().expect("flush");
            let mut header = [0; 9];
            output.read_exact(&mut header).expect("reply");
            let size = u64::from_le_bytes(header[1..].try_into().expect("length")) as usize;
            let mut expected = vec![0; size];
            output.read_exact(&mut expected).expect("pixels");
            if header[0] != 0 {
                gate.fail(p.display().to_string(), String::from_utf8_lossy(&expected));
                continue;
            }
            match image.samples {
                Samples::U8(v) => gate.compare_bytes(p.display().to_string(), &v, &expected),
                Samples::U16(_) => gate.fail(
                    p.display().to_string(),
                    "Pillow RGBA8 cannot gate full 16-bit precision; add a 16-bit oracle",
                ),
            }
        }
        println!(
            "{var}: {path_count} paths, {} distinct contents (SHA256)",
            contents.len()
        );
        drop(input);
        assert!(python.wait().expect("oracle exit").success());
        println!("{gate}\n{roundtrip}\nconsumed ancillary chunks={metadata:?}");
        gate.assert_passed();
        roundtrip.assert_passed();
    }
    assert!(
        configured == 0 || checked > 0,
        "configured corpora contain no PNG textures"
    );
}
