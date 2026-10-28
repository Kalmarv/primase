//! A timed area read, stage by stage: the benchmark for the FromSoft area reader. It reads one map the way the area
//! tests do and prints the wall time of each of the read's stages ([`read_game_area_staged`]) and the world it made;
//! with `--digest` it also prints a digest of every component and resource of that world (their `Debug` text, which is
//! stable between runs of one build and between builds that change no component), the proof that a change to the read
//! path changed no output.
//!
//! `cargo run --release -p primase-fromsoft --example bench_area -- <sekiro|elden> <install> <keys folder> <Oodle
//! library> <map> [--region x0,z0,x1,z1,margin] [--parents] [--threads N] [--digest]`, for example
//! `bench_area sekiro <install> <keys> <oo2core_6_win64.dll> m11_01 --region=-170,180,-50,300,30`. Report the file
//! cache's state (the first run after other reads is warm), `--threads` (the RSA unwrap of the archive indexes) and the
//! machine's load with the numbers.
#![allow(unsafe_code, reason = "opens the Oodle library the caller names")]
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::unwrap_used,
    reason = "a benchmark program"
)]

use std::fmt::Write as _;
use std::num::NonZeroUsize;
use std::path::Path;
use std::time::Instant;

use fromsoft_formats::{Game, Install, Keys, dcx::Decoder};
use primase_core::World;
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro::{self, Parents, Region};
use sha2::{Digest, Sha256};

/// A hasher that `write!` can feed.
struct Feed(Sha256);
impl std::fmt::Write for Feed {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.update(s.as_bytes());
        Ok(())
    }
}

fn digest(world: &World) -> String {
    let mut feed = Feed(Sha256::new());
    world.visit_components_debug(&mut |name, version, entity, value| {
        writeln!(feed, "{name}|{version}|{entity:?}|{value:?}").unwrap();
    });
    world.visit_resources_debug(&mut |name, version, value| {
        writeln!(feed, "resource {name}|{version}|{value:?}").unwrap();
    });
    feed.0.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (game, root, keys, library, map) = (args.next(), args.next(), args.next(), args.next(), args.next());
    let (Some(game), Some(root), Some(keys), Some(library), Some(map)) = (game, root, keys, library, map)
    else {
        return Err("usage: bench_area <sekiro|elden> <install> <keys folder> <Oodle library> <map> [--region=x0,z0,x1,z1,margin] [--parents] [--threads N] [--digest]".into());
    };
    let (mut region, mut parents, mut threads, mut want_digest) = (None, Parents::None, 1usize, false);
    while let Some(a) = args.next() {
        if let Some(r) = a.strip_prefix("--region=") {
            let v: Vec<f32> = r.split(',').map(str::parse).collect::<Result<_, _>>()?;
            let [x0, z0, x1, z1, margin] = v[..] else {
                return Err("--region needs x0,z0,x1,z1,margin".into());
            };
            region = Some(Region {
                rect: [x0, z0, x1, z1],
                margin,
            });
        } else if a == "--parents" {
            parents = Parents::Include;
        } else if a == "--threads" {
            threads = args.next().ok_or("--threads needs a value")?.parse()?;
        } else if a == "--digest" {
            want_digest = true;
        } else {
            return Err(format!("unknown option {a}").into());
        }
    }
    let game = match game.as_str() {
        "sekiro" => Game::Sekiro,
        "elden" => Game::EldenRing,
        other => return Err(format!("unknown game {other:?}").into()),
    };
    let keys = Keys::from_folder(&keys, game, game.param_key_sha256())?;
    let install = Install::new(&root, game, keys).with_threads(NonZeroUsize::new(threads).ok_or("threads")?);
    // SAFETY: the library is the game's own, named by the caller.
    let oodle = unsafe { Oodle::open(Some(Path::new(&library)), Version::V6) }?;
    let decoder = Decoder {
        oodle: Some(&oodle),
        max_output: 1 << 30,
    };
    let options = sekiro::Options {
        region,
        parents,
        ..Default::default()
    };
    let mut world = World::default();
    let start = Instant::now();
    let mut last = (String::from("open"), start);
    let mut stages: Vec<(String, f64)> = Vec::new();
    let report =
        sekiro::read_game_area_staged(&install, &map, &decoder, &options, &mut world, &mut |name| {
            let now = Instant::now();
            stages.push((last.0.clone(), (now - last.1).as_secs_f64()));
            last = (name.to_owned(), now);
        })?;
    let total = start.elapsed().as_secs_f64();
    println!("{game:?} {map}, {threads} thread(s) for the index unwrap; read {total:.3} s");
    for (name, secs) in &stages {
        println!("  {name:<40} {secs:8.3} s");
    }
    println!(
        "{} entities, {} placements, {} prototypes, {} materials",
        world.entity_count(),
        report.placements,
        report.prototypes,
        report.materials
    );
    if want_digest {
        let t = Instant::now();
        println!(
            "world digest {} ({:.1} s)",
            digest(&world),
            t.elapsed().as_secs_f64()
        );
    }
    let t = Instant::now();
    drop(world);
    println!("drop of the world {:.3} s", t.elapsed().as_secs_f64());
    Ok(())
}
