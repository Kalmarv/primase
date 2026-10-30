//! D3D pixels exact against bcdec; independent C# and Pillow values within one channel level.
mod inputs;

use inputs::{Game, OracleSet, Tool, oracle, tool};
use primase_core::TexFormat;
use primase_testdata::gate::{GateReport, Tolerance};
use primase_texcodec::{Bcdec, Decoder, TexDecoder};
use std::{
    collections::BTreeMap,
    io::{BufReader, Read, Write},
    process::{Command, Stdio},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn query(input: &mut impl Write, output: &mut impl Read, path: &std::path::Path) -> Result<Vec<u8>> {
    writeln!(input, "{}", path.display())?;
    input.flush()?;
    let mut header = [0; 9];
    output.read_exact(&mut header)?;
    let n = u64::from_le_bytes(header[1..].try_into()?) as usize;
    if n > 1024 * 1024 * 1024 {
        return Err("oracle size limit".into());
    }
    let mut bytes = vec![0; n];
    output.read_exact(&mut bytes)?;
    if header[0] != 0 {
        return Err(String::from_utf8_lossy(&bytes).to_string().into());
    }
    Ok(bytes)
}
/// The area textures the C# reference (BCnEncoder.Net 2.2.1) cannot decode over `oracles.soulsformats.sekiro/tpf`,
/// by its reason, measured on that corpus: m25's Galaxy and MilkyWay maps.
const PINNED_REFERENCE_REFUSALS: &[(&str, usize)] = &[("Unsupported compression format: Unknown", 2)];

#[test]
fn area_pixels() -> Result<()> {
    let Some(root) = oracle(OracleSet::SoulsFormats, Game::Sekiro, "tpf") else {
        return Ok(());
    };
    // the worker's folder: DdsWorker.dll with the libraries it loads beside it
    let Some(csharp) = oracle(OracleSet::Scripts, Game::Sekiro, "dds") else {
        return Ok(());
    };
    let Some(dotnet) = tool(Tool::Dotnet) else {
        return Ok(());
    };
    let Some(python) = tool(Tool::PythonPillow) else {
        return Ok(());
    };
    let mut cs = Command::new(dotnet)
        .arg(csharp.join("DdsWorker.dll"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut csin = cs.stdin.take().ok_or("stdin")?;
    let mut csout = BufReader::new(cs.stdout.take().ok_or("stdout")?);
    let mut py = Command::new(python)
        .args([
            "-I",
            "-c",
            include_str!("../../../toolkit/texcodec/tests/oracles/worker.py"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut pyin = py.stdin.take().ok_or("stdin")?;
    let mut pyout = BufReader::new(py.stdout.take().ok_or("stdout")?);
    let mut hashes = BTreeMap::new();
    let mut path_count = 0;
    // textures the C# reference cannot decode, by its reason (the census writes `rgba_error` in place of `rgba`)
    let mut unreferenced: BTreeMap<String, usize> = BTreeMap::new();
    for line in std::fs::read_to_string(root.join("packages.jsonl"))?.lines() {
        let p: serde_json::Value = serde_json::from_str(line)?;
        if p["area"] != true {
            continue;
        }
        for t in p["textures"].as_array().ok_or("textures")? {
            if t["kind"] == 0 {
                path_count += 1;
                let Some(rgba) = t["rgba"].as_str() else {
                    let why = t["rgba_error"].as_str().ok_or("neither rgba nor rgba_error")?;
                    *unreferenced.entry(why.to_owned()).or_default() += 1;
                    continue;
                };
                hashes.insert(t["sha256"].as_str().ok_or("hash")?.to_owned(), rgba.to_owned());
            }
        }
    }
    let mut pillow = GateReport::new("TPF area D3D vs Pillow", Tolerance::Abs(1.));
    let mut bcdec = GateReport::new("TPF area D3D arithmetic vs bcdec", Tolerance::Exact);
    let mut csharp = GateReport::new("TPF area D3D vs C#", Tolerance::Abs(1.));
    let mut hist = BTreeMap::<String, [u64; 256]>::new();
    let mut by_format = BTreeMap::<String, GateReport>::new();
    let mut failures = Vec::new();
    for (hash, want) in &hashes {
        let path = root.join("area-dds").join(format!("{hash}.dds"));
        let result = (|| -> Result<()> {
            let dds = std::fs::read(&path)?;
            let t = primase_dds::read(&dds)?;
            let mut reference = query(&mut csin, &mut csout, &path)?;
            use sha2::{Digest, Sha256};
            let digest: String = Sha256::digest(&reference)
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect();
            assert_eq!(&digest, want, "C# reference identity");
            let mut pyref = query(&mut pyin, &mut pyout, &path)?;
            if t.format == TexFormat::Bc4Unorm {
                for pixels in [&mut reference, &mut pyref] {
                    for px in pixels.as_chunks_mut::<4>().0 {
                        px[1] = 0;
                        px[2] = 0;
                    }
                }
            }
            let independent = Bcdec.decode_rgba8(t.format, t.width, t.height, &t.levels[0])?;
            let rgba = Decoder::default().decode_rgba8(t.format, t.width, t.height, &t.levels[0])?;
            bcdec.compare_bytes(hash, &rgba, &independent);
            csharp.compare_bytes(hash, &rgba, &reference);
            pillow.compare_bytes(hash, &rgba, &pyref);
            for (name, reference) in [("C#", &reference), ("Pillow", &pyref)] {
                let label = format!("{:?} vs {name}", t.format);
                by_format
                    .entry(label.clone())
                    .or_insert_with(|| GateReport::new(&label, Tolerance::Abs(1.)))
                    .compare_bytes(hash, &rgba, reference);
                if rgba.len() == reference.len() {
                    let histogram = hist.entry(label).or_insert([0; 256]);
                    for (a, b) in rgba.iter().zip(reference) {
                        histogram[a.abs_diff(*b) as usize] += 1;
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            failures.push(format!("{hash}: {e}"));
        }
    }
    drop(csin);
    drop(pyin);
    let (cs_exit, py_exit) = (cs.wait()?, py.wait()?);
    assert!(
        cs_exit.success() && py_exit.success(),
        "a reference worker failed (C# {cs_exit}, Pillow {py_exit}); {} textures failed, the first: {:?}",
        failures.len(),
        failures.first()
    );
    println!("area references {path_count}, distinct DDS {}", hashes.len());
    // the reference decoder's refusals, pinned: a new reason, or more of one, fails
    for (why, n) in &unreferenced {
        println!("  not decoded by the C# reference ({n}): {why}");
    }
    let over: Vec<String> = unreferenced
        .iter()
        .filter(|(why, n)| {
            PINNED_REFERENCE_REFUSALS
                .iter()
                .all(|(p, pin)| p != why || *n > pin)
        })
        .map(|(why, n)| format!("{why}: {n}"))
        .collect();
    assert!(over.is_empty(), "C# reference refusals beyond the pins: {over:?}");
    println!("{pillow}\n{bcdec}\n{csharp}");
    for (k, h) in hist {
        println!(
            "{}, channel histogram {:?}",
            by_format[&k],
            h.iter().enumerate().filter(|(_, n)| **n > 0).collect::<Vec<_>>()
        );
    }
    for e in &failures {
        println!("FAIL {e}");
    }
    assert!(failures.is_empty());
    csharp.assert_passed();
    pillow.assert_passed();
    bcdec.assert_passed();
    Ok(())
}
