//! The shader pack's census: every container of `ShaderDataDX11.bin` (in the folder `PRIMASE_ANVIL_DIR` names),
//! checked by its own checksum and the pack by its trailing MD5 ([`containers`]); each container's stage counted; each
//! pixel program parsed by primase-dxbc (its program, then the program with the container's signatures), the failures
//! counted by their message (numbers taken out), most first: what the evaluator lacks, by how many pixel shaders it
//! stops. A count, not an oracle. Run by hand: `cargo test -p primase-anvil --test shaders -- --ignored --nocapture`.

use std::collections::BTreeMap;

use primase_anvil::read::shader_pack::containers;
use primase_testdata::real_data;

/// A message with its digits taken out, so one feature counts once.
fn kind(message: &str) -> String {
    message
        .chars()
        .map(|c| if c.is_ascii_digit() { '#' } else { c })
        .collect()
}

#[test]
#[ignore = "reads the game's 18 MB shader pack; run by hand"]
fn every_container_of_the_shader_pack() {
    let Some(dir) = real_data("PRIMASE_ANVIL_DIR") else {
        return;
    };
    let pack = std::fs::read(dir.join("ShaderDataDX11.bin")).expect("pack");
    let all = containers(&pack).expect("every digest");
    println!(
        "containers: {} (every checksum and the pack's MD5 checked)",
        all.len()
    );
    let mut stages: BTreeMap<String, u64> = BTreeMap::new();
    let mut program: BTreeMap<String, u64> = BTreeMap::new();
    let mut shader: BTreeMap<String, u64> = BTreeMap::new();
    for c in &all {
        let code = primase_dxbc::chunks(c)
            .expect("directory")
            .into_iter()
            .find(|k| &k.kind == b"SHEX" || &k.kind == b"SHDR")
            .and_then(|k| {
                k.data
                    .get(..4)
                    .map(|w| u32::from_le_bytes(w.try_into().expect("4")))
            });
        let stage = match code.map(|v| v >> 16) {
            Some(0) => "pixel",
            Some(1) => "vertex",
            Some(2) => "geometry",
            Some(3) => "hull",
            Some(4) => "domain",
            Some(5) => "compute",
            _ => "unknown",
        };
        *stages
            .entry(format!(
                "{stage} {}.{}",
                code.map_or(0, |v| (v >> 4) & 15),
                code.map_or(0, |v| v & 15)
            ))
            .or_default() += 1;
        if stage != "pixel" {
            continue;
        }
        let p = match primase_dxbc::Program::parse(c) {
            Ok(_) => "parsed".to_owned(),
            Err(e) => kind(&e.detail),
        };
        *program.entry(p).or_default() += 1;
        let s = match primase_dxbc::Shader::parse_stripped(c) {
            Ok(_) => "parsed".to_owned(),
            Err(e) => kind(&e.detail),
        };
        *shader.entry(s).or_default() += 1;
    }
    for (title, t) in [
        ("stages", &stages),
        ("pixel programs", &program),
        ("pixel shaders with signatures", &shader),
    ] {
        println!("{title}:");
        let mut rows: Vec<_> = t.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1));
        for (k, n) in rows {
            println!("  {n:>6}  {k}");
        }
    }
}
