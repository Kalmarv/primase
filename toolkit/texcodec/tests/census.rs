use sha2::{Digest, Sha256};
// Real DDS level-zero gates against bcdec_rs and a persistent Pillow 11 process.
use primase_core::{
    TexFormat,
    gate::{GateReport, Tolerance, real_data},
};
use primase_texcodec::{Bcdec, Decoder, TexDecoder};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
fn walk(path: &Path, inside: bool, out: &mut Vec<PathBuf>) {
    let inside = inside || path.file_name().is_some_and(|x| x == "textures");
    let mut entries: Vec<_> = std::fs::read_dir(path)
        .expect("read corpus directory")
        .map(|x| x.expect("entry").path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, inside, out)
        } else if inside && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dds")) {
            out.push(p)
        }
    }
}
#[test]
fn scene_dds_decoders() {
    for var in ["PRIMASE_SCENES_GTA", "PRIMASE_SCENES_SEKIRO"] {
        let Some(root) = real_data(var) else { continue };
        let mut paths = Vec::new();
        walk(&root, false, &mut paths);
        assert!(!paths.is_empty(), "empty corpus");
        let mut python = Command::new("python")
            .arg("-I")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/oracles/worker.py"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Pillow oracle");
        let mut input = python.stdin.take().expect("stdin");
        let mut output = BufReader::new(python.stdout.take().expect("stdout"));
        let mut bc = GateReport::new(format!("{var} own vs bcdec"), Tolerance::Exact);
        let mut pillow = GateReport::new(format!("{var} D3D vs Pillow 11"), Tolerance::Abs(1.));
        let mut counts = BTreeMap::new();
        let mut formats = BTreeMap::<String, GateReport>::new();
        let mut format_histograms = BTreeMap::<String, [u64; 256]>::new();
        let mut histogram = [0u64; 256];
        let path_count = paths.len();
        let mut contents = std::collections::BTreeSet::<[u8; 32]>::new();
        for p in paths {
            let data = std::fs::read(&p).expect("read DDS");
            if !contents.insert(Sha256::digest(&data).into()) {
                continue;
            }
            let t = primase_dds::read(&data).expect("parse DDS");
            *counts.entry(format!("{:?}", t.format)).or_insert(0u64) += 1;
            let own = Decoder::default()
                .decode_rgba8(t.format, t.width, t.height, &t.levels[0])
                .expect("own");
            let reference = Bcdec
                .decode_rgba8(t.format, t.width, t.height, &t.levels[0])
                .expect("bcdec");
            bc.compare_bytes(p.display().to_string(), &own, &reference);
            writeln!(input, "{}", p.display()).expect("request");
            input.flush().expect("flush");
            let mut header = [0; 9];
            output.read_exact(&mut header).expect("oracle header");
            let size = u64::from_le_bytes(header[1..].try_into().expect("u64")) as usize;
            let mut expected = vec![0; size];
            output.read_exact(&mut expected).expect("oracle body");
            if header[0] != 0 {
                pillow.fail(p.display().to_string(), String::from_utf8_lossy(&expected));
                continue;
            }
            if t.format == TexFormat::Bc4Unorm {
                for px in expected.as_chunks_mut::<4>().0 {
                    px[1] = 0;
                    px[2] = 0;
                }
            }
            pillow.compare_bytes(p.display().to_string(), &own, &expected);
            let label = format!("{:?}", t.format);
            formats
                .entry(label.clone())
                .or_insert_with(|| {
                    GateReport::new(format!("{var} {label} D3D vs Pillow"), Tolerance::Abs(1.))
                })
                .compare_bytes(p.display().to_string(), &own, &expected);
            let format_histogram = format_histograms.entry(label).or_insert([0; 256]);
            for (a, b) in own.iter().zip(&expected) {
                histogram[a.abs_diff(*b) as usize] += 1;
                format_histogram[a.abs_diff(*b) as usize] += 1;
            }
        }
        println!(
            "{var}: {path_count} paths, {} distinct contents (SHA256)",
            contents.len()
        );
        drop(input);
        assert!(output.fill_buf().expect("eof").is_empty());
        assert!(python.wait().expect("oracle exit").success());
        println!(
            "{bc}\n{pillow}\nformats={counts:?}\nbcdec vs Pillow channel histogram={:?}",
            histogram
                .iter()
                .enumerate()
                .filter(|(_, n)| **n > 0)
                .collect::<Vec<_>>()
        );
        bc.assert_passed();
        pillow.assert_passed();
        for (name, gate) in formats {
            println!(
                "{gate}; channel histogram={:?}",
                format_histograms[&name]
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| **n > 0)
                    .collect::<Vec<_>>()
            );
            gate.assert_passed();
        }
    }
}
