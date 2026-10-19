//! D3D pixels exact against bcdec; independent C# and Pillow values within one channel level.
use primase_core::{
    TexFormat,
    gate::{GateReport, Tolerance},
};
use primase_testdata::real_data;
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
#[test]
fn area_pixels() -> Result<()> {
    let Some(root) = real_data("PRIMASE_FROMSOFT_TPF_ORACLE") else {
        return Ok(());
    };
    let Some(csharp) = real_data("PRIMASE_FROMSOFT_DDS_ORACLE") else {
        return Ok(());
    };
    let mut cs = Command::new("dotnet")
        .arg(csharp)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut csin = cs.stdin.take().ok_or("stdin")?;
    let mut csout = BufReader::new(cs.stdout.take().ok_or("stdout")?);
    let mut py = Command::new("python")
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
    for line in std::fs::read_to_string(root.join("packages.jsonl"))?.lines() {
        let p: serde_json::Value = serde_json::from_str(line)?;
        if p["area"] != true {
            continue;
        }
        for t in p["textures"].as_array().ok_or("textures")? {
            if t["kind"] == 0 {
                path_count += 1;
                hashes.insert(
                    t["sha256"].as_str().ok_or("hash")?.to_owned(),
                    t["rgba"].as_str().ok_or("reference decode failed")?.to_owned(),
                );
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
    assert!(cs.wait()?.success());
    assert!(py.wait()?.success());
    println!("area references {path_count}, distinct DDS {}", hashes.len());
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
