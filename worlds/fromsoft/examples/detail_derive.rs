//! Each of Sekiro's detail-blend G-buffer pixel programs' equation ([`derive`]), its material's words given (seeded, the
//! same each run): the texture roles, the constants solved, the proof's largest difference per role (and on the sets
//! where the weights mix), the weights, gloss scales and alpha, each constant beside the material word it reads (and
//! those that disagree), and the tint fold's cost.
//!
//! `cargo run --release -p primase-fromsoft --example detail_derive -- <folder of Sekiro .ppo> [name filter]`: the
//! shader corpus's `programs/sekiro-ps` (`PRIMASE_SHADER_CORPUS_SEKIRO`), the filter `DetailBlend` by default. A block a
//! program, then the totals.

use std::collections::BTreeMap;
use std::path::PathBuf;

use primase_fromsoft::detail_derive::{WEIGHT_SETS, derive};
use primase_fromsoft::detail_probe::ProgramProbe;

type BoxError = Box<dyn std::error::Error>;

/// A seeded sequence of words (xorshift32), the same for the same seed.
struct Words(u32);

impl Words {
    /// A float from 0 to 1.
    fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
}

fn seed(name: &str) -> u32 {
    name.bytes().fold(2_166_136_261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16_777_619)
    }) | 1
}

fn main() -> Result<(), BoxError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (folder, filter) = match args.as_slice() {
        [folder] => (PathBuf::from(folder), "DetailBlend".to_owned()),
        [folder, filter] => (PathBuf::from(folder), filter.clone()),
        _ => return Err("usage: detail_derive <folder of Sekiro .ppo> [name filter]".into()),
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
    let mut largest: BTreeMap<&'static str, f32> = BTreeMap::new();
    let mut transition: BTreeMap<&'static str, f32> = BTreeMap::new();
    let mut fold = 0f32;
    for path in &files {
        let name = path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut probe = ProgramProbe::new(&std::fs::read(path)?)?;
        let mut words = Words(seed(&name));
        for m in probe.material.clone() {
            let values: Vec<u32> = (0..m.words).map(|_| words.unit().to_bits()).collect();
            probe.set_words(m.buffer, m.word, &values)?;
        }
        println!("{name}");
        let d = match derive(&mut probe, seed(&name)) {
            Ok(d) => d,
            Err(e) => {
                let reason = e.split(':').next().unwrap_or(&e).to_owned();
                println!("  REFUSED {}", &e[..e.len().min(600)]);
                *totals.entry(format!("refused: {reason}")).or_default() += 1;
                continue;
            }
        };
        *totals.entry("proved".into()).or_default() += 1;
        let r = &d.roles;
        println!(
            "  roles SlotRoles {{ base_colour: {}, base_normal: {}, base_mask: {}, details: {:?} }}",
            r.base_colour, r.base_normal, r.base_mask, r.details
        );
        println!("  frame {:?}", d.frame);
        println!(
            "  proof {} sets, largest units {:?}",
            d.proof.sets, d.proof.largest
        );
        for (role, u) in &d.proof.largest {
            let e = largest.entry(role).or_default();
            *e = e.max(*u);
        }
        println!("  weights {:?}", WEIGHT_SETS[d.constants.weight_set]);
        let scales: Vec<_> = d.constants.details.iter().map(|t| t.gloss_scale).collect();
        println!("  gloss scales {scales:?}");
        if let Some((sets, t)) = &d.proof.transition {
            println!("  transition {sets} sets, largest units {t:?}");
            for (role, u) in t {
                let e = transition.entry(role).or_default();
                *e = e.max(*u);
            }
        }
        let alpha = match d.constants.alpha {
            Some(j) => format!("detail {j}'s, proven through its lane"),
            None => "its lane has no role: opaque".into(),
        };
        println!("  alpha {alpha}");
        *totals
            .entry(format!(
                "alpha {}",
                if d.constants.alpha.is_some() {
                    "proven through its lane"
                } else {
                    "lane with no role"
                }
            ))
            .or_default() += 1;
        match d.fold {
            Some(f) => {
                println!("  fold gain {} costs {} units", f.gain, f.largest);
                fold = fold.max(f.largest);
            }
            None => {
                println!("  fold not measured: a tint lane reads no one word");
                *totals.entry("fold not measured".into()).or_default() += 1;
            }
        }
        for f in &d.fields {
            let fields: Vec<String> = f.fields.iter().map(|(n, k)| format!("{n}[{k}]")).collect();
            let verdict = match f.agrees() {
                Some(true) => "agrees",
                Some(false) => "DISAGREES",
                None if f.fields.is_empty() => "the program's own",
                None => "several words",
            };
            println!(
                "  {} = {} from {} ({verdict}{})",
                f.constant,
                f.solved,
                if fields.is_empty() {
                    "-".into()
                } else {
                    fields.join(" ")
                },
                f.expected.map(|e| format!(", expected {e}")).unwrap_or_default()
            );
            *totals.entry(format!("constant {verdict}")).or_default() += 1;
        }
        let disagreements = d.disagreements().count();
        if disagreements > 0 {
            *totals.entry("programs with a disagreement".into()).or_default() += 1;
        }
    }
    println!("== {} programs", files.len());
    for (k, n) in &totals {
        println!("{n:6} {k}");
    }
    println!("largest proof units per role {largest:?}; largest fold cost {fold} units");
    println!("largest transition units per role {transition:?}");
    Ok(())
}
