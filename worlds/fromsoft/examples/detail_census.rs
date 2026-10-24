//! What each of Sekiro's detail-blend G-buffer pixel programs reads, run as the program bake binds it
//! ([`ProgramProbe`]) with its material's words and texture samples given (seeded, the same each run): how it reads its
//! material (named fields or `SAT_MtdParam`), which textures it samples through which samplers, and whether a single-lane
//! input the vertex stage passes (frozen at 0) changes any output when it is 1.
//!
//! `cargo run --release -p primase-fromsoft --example detail_census -- <folder of Sekiro .ppo> [name filter]`: the
//! shader corpus's `programs/sekiro-ps` (`PRIMASE_SHADER_CORPUS_SEKIRO`), the filter `DetailBlend` by default. One line
//! a program, then the totals.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use primase_fromsoft::detail_probe::ProgramProbe;
use primase_shader_exec::ImageSample;
use primase_shader_exec::probe::AnsweredReads;

type BoxError = Box<dyn std::error::Error>;

/// A seeded sequence of words (xorshift32), the same for the same seed.
struct Words(u32);

impl Words {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
    /// A float from 0 to 1.
    fn unit(&mut self) -> f32 {
        (self.next() >> 8) as f32 / (1u32 << 24) as f32
    }
}

fn seed(name: &str) -> u32 {
    name.bytes().fold(2_166_136_261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16_777_619)
    }) | 1
}

/// The sample of texture slot `slot`: four lanes from 0 to 1, the same for the slot wherever it is read.
fn texel(slot: u32) -> [f32; 4] {
    let mut w = Words(seed(&format!("t{slot}")));
    [w.unit(), w.unit(), w.unit(), w.unit()]
}

fn main() -> Result<(), BoxError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (folder, filter) = match args.as_slice() {
        [folder] => (PathBuf::from(folder), "DetailBlend".to_owned()),
        [folder, filter] => (PathBuf::from(folder), filter.clone()),
        _ => return Err("usage: detail_census <folder of Sekiro .ppo> [name filter]".into()),
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&folder)?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    files.retain(|p| {
        p.extension().is_some_and(|e| e == "ppo")
            && p.file_name()
                .is_some_and(|n| n.to_string_lossy().contains(&filter))
    });
    files.sort();
    let mut totals: BTreeMap<String, u64> = BTreeMap::new();
    for path in &files {
        let name = path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let line = match census(&std::fs::read(path)?, &name) {
            Ok((line, keys)) => {
                for k in keys {
                    *totals.entry(k).or_default() += 1;
                }
                line
            }
            Err(e) => {
                *totals.entry(format!("refused: {e}")).or_default() += 1;
                format!("REFUSED {e}")
            }
        };
        println!("{name}\t{line}");
    }
    println!("== {} programs", files.len());
    for (k, n) in &totals {
        println!("{n:6} {k}");
    }
    Ok(())
}

/// One program's line, and the keys it adds to the totals.
fn census(bytes: &[u8], name: &str) -> Result<(String, Vec<String>), BoxError> {
    let mut probe = ProgramProbe::new(bytes)?;
    let mut words = Words(seed(name));
    for m in probe.material.clone() {
        let values: Vec<u32> = (0..m.words).map(|_| words.unit().to_bits()).collect();
        probe.set_words(m.buffer, m.word, &values)?;
    }
    let uv = [0.3, 0.6];
    let mut reads = AnsweredReads::new(|r: &ImageSample<u32>| Ok(texel(r.texture)));
    let (outputs, discarded) = probe.pixel(uv, &mut reads)?;
    // each texture's samplers and reads
    let mut textures: BTreeMap<u32, (BTreeSet<u32>, u32)> = BTreeMap::new();
    for r in &reads.samples {
        let entry = textures.entry(r.texture).or_default();
        entry.0.insert(r.sampler);
        entry.1 += 1;
    }
    let texture_text: Vec<String> = textures
        .iter()
        .map(|(t, (s, n))| {
            format!(
                "t{t}@{}x{n}",
                s.iter().map(|s| format!("s{s}")).collect::<Vec<_>>().join("+")
            )
        })
        .collect();
    // the samplers each texture is read through, as a signature of the program's texture use
    let groups: BTreeMap<String, usize> = textures.values().fold(BTreeMap::new(), |mut m, (s, _)| {
        *m.entry(s.iter().map(|s| format!("s{s}")).collect::<Vec<_>>().join("+"))
            .or_default() += 1;
        m
    });
    let signature = groups
        .iter()
        .map(|(s, n)| format!("{n}x{s}"))
        .collect::<Vec<_>>()
        .join(" ");
    // single-lane inputs frozen at 0: does 1 change an output?
    let mut live = Vec::new();
    for frozen in probe.frozen.clone() {
        let Some(rest) = frozen.strip_prefix("TEXCOORD") else {
            continue;
        };
        let Some(index) = rest.strip_suffix(" 0").and_then(|i| i.parse::<u32>().ok()) else {
            continue;
        };
        probe.set_input("TEXCOORD", index, [1.; 4])?;
        let mut again = AnsweredReads::new(|r: &ImageSample<u32>| Ok(texel(r.texture)));
        let (changed, _) = probe.pixel(uv, &mut again)?;
        probe.set_input("TEXCOORD", index, [0.; 4])?;
        let differ = outputs
            .iter()
            .zip(&changed)
            .any(|(a, b)| a.iter().zip(b).any(|(x, y)| x.to_bits() != y.to_bits()));
        live.push(format!(
            "TEXCOORD{index}.x {}",
            if differ { "live" } else { "unread" }
        ));
    }
    let roles: Vec<String> = probe.outputs.iter().map(|o| format!("{:?}", o.role)).collect();
    let line = format!(
        "{:?} {} material words\t{}\t{}\t{}\tdiscarded={discarded}\troles={}",
        probe.form,
        probe.material.iter().map(|m| m.words).sum::<u32>(),
        texture_text.join(" "),
        signature,
        live.join(", "),
        roles.join("|"),
    );
    let mut keys = vec![
        format!("form {:?}", probe.form),
        format!("texture signature {signature}"),
    ];
    keys.extend(live.into_iter().map(|l| format!("input {l}")));
    if discarded {
        keys.push("discarded".into());
    }
    Ok((line, keys))
}
