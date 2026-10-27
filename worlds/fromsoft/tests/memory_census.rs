//! A memory census of the area read: how many bytes the process holds at the start of each stage of
//! `sekiro::read_game_area_staged`, the most it held during the stage, and what the finished world retains, for the areas
//! whose whole-area reads were killed past 8 GB (Sekiro's m10, m11_01, m20 and m25, Elden Ring's m30). It decides which stage
//! to make smaller first: a stage whose peak is far above its start holds something it then lets go of; a stage whose
//! start keeps climbing holds something to the end of the read.
//!
//! Every allocation of the process goes through [`Counting`], which keeps the bytes held and the most it held since the
//! last stage began. A read that would hold more than the cap (the debug key `fromsoft.memory_cap_gb`, default 10) aborts
//! the process with a `MEMCAP-EXCEEDED` line naming the stage, rather than the machine's memory manager killing it later
//! and saying nothing; run one area to a process (the debug key `fromsoft.census_areas=sekiro/m11_01_00_00`) so the others
//! are not lost to an abort. Debug keys are set in `GAME_DATA_DEBUG` (`key=value;key=value`, CONTRIBUTING.md).
//!
//! The lines:
//!
//! - `MEMCAP <bytes>`: the cap;
//! - `MEMSTAGE <game> <area> stage <name> start <bytes> peak <bytes> end <bytes> seconds <s>`: the stage that just ended
//!   (the bytes held when it began, its peak, the bytes held when it ended);
//! - `MEMDONE <game> <area> baseline <bytes> peak <bytes> retained <bytes> seconds <s>`: the whole read, and what the
//!   world and report hold when it returns, above the bytes held before it;
//! - `MEMWORLD <game> <area> entities <n> textures <n> texture_bytes <b> primitives <n> position_vertices <v>`: what the
//!   retained bytes are made of, as far as the world's own data says;
//! - `MEMERR <game> <area> <message>`: a read that failed.
//!
//! It asserts nothing but that the allocator saw the read. It is `#[ignore]`d: it reads whole areas (minutes and
//! gigabytes); run it alone, `cargo test -p primase-fromsoft --test memory_census -- --ignored --nocapture`, where the
//! install, its keys and the Oodle library are configured; otherwise it does nothing.
#![allow(
    unsafe_code,
    reason = "a counting global allocator: it forwards every call to the system allocator, which it only adds counters to"
)]
mod inputs;

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

use primase_core::World;
use primase_fromsoft::sekiro::{Options, read_game_area_staged};
use primase_fromsoft::{Game, Install, Keys, NameDictionary, dcx::Decoder};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

/// The stages `read_game_area_staged` names, in order (its documentation's list); the index is what the allocator prints
/// of the stage it was in when the cap was crossed.
const STAGES: [&str; 12] = [
    "(before the read)",
    "definitions and texture catalog",
    "models of every placement",
    "select placements",
    "wanted textures and shader bundles",
    "load textures",
    "spawn prototypes",
    "spawn placements",
    "collision",
    "extras",
    "environment",
    "done",
];

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static CAP: AtomicUsize = AtomicUsize::new(usize::MAX);
static STAGE: AtomicUsize = AtomicUsize::new(0);

/// The system allocator, counting the bytes it holds.
struct Counting;

/// Stops the process, saying (without allocating) which stage it was in and how much it held.
fn over_cap(now: usize) -> ! {
    fn digits(mut n: usize, out: &mut [u8; 20]) -> &[u8] {
        let mut at = out.len();
        loop {
            at -= 1;
            out[at] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        &out[at..]
    }
    let (mut a, mut b) = ([0u8; 20], [0u8; 20]);
    let stage = STAGE.load(Relaxed).min(STAGES.len() - 1);
    let mut stderr = std::io::stderr();
    let _ = stderr.write_all(b"MEMCAP-EXCEEDED stage ");
    let _ = stderr.write_all(STAGES[stage].as_bytes());
    let _ = stderr.write_all(b" bytes ");
    let _ = stderr.write_all(digits(now, &mut a));
    let _ = stderr.write_all(b" cap ");
    let _ = stderr.write_all(digits(CAP.load(Relaxed), &mut b));
    let _ = stderr.write_all(b"\n");
    std::process::abort()
}

fn grow(by: usize) {
    let now = CURRENT.fetch_add(by, Relaxed) + by;
    if now > CAP.load(Relaxed) {
        over_cap(now);
    }
    PEAK.fetch_max(now, Relaxed);
}

// SAFETY: every method forwards to `System` with the arguments it was given and only adds to atomic counters.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        grow(layout.size());
        // SAFETY: the caller's contract is `System`'s.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        grow(layout.size());
        // SAFETY: the caller's contract is `System`'s.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        CURRENT.fetch_sub(layout.size(), Relaxed);
        // SAFETY: the caller's contract is `System`'s.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size >= layout.size() {
            grow(new_size - layout.size());
        } else {
            CURRENT.fetch_sub(layout.size() - new_size, Relaxed);
        }
        // SAFETY: the caller's contract is `System`'s.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The areas whose whole-area reads were killed, by game. Sekiro's m25 is the one the lab's read took 51.9 GB of commit
/// for (a region of 886 x 1095 m, 6038 parts): this census reads it whole, with the reader's own default options.
const AREAS: [(Game, &str, &[&str]); 2] = [
    (
        Game::Sekiro,
        "sekiro",
        &["m10_00_00_00", "m11_01_00_00", "m20_00_00_00", "m25_00_00_00"],
    ),
    (Game::EldenRing, "elden", &["m30_00_00_00"]),
];

/// The cap in bytes: the debug key `fromsoft.memory_cap_gb` (a whole or decimal number of GiB), else 10 GiB.
fn cap_bytes() -> usize {
    inputs::debug("fromsoft.memory_cap_gb")
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|gb| gb.is_finite() && *gb > 0.0)
        .map_or(10 << 30, |gb| (gb * (1u64 << 30) as f64) as usize)
}

/// The areas to read: the debug key `fromsoft.census_areas` (a comma list of `<game>/<area>` or `<area>`), else every one
/// of [`AREAS`].
fn wanted(game: &str, area: &str) -> bool {
    inputs::debug("fromsoft.census_areas").is_none_or(|list| {
        list.split(',')
            .map(str::trim)
            .any(|item| item == area || item == format!("{game}/{area}"))
    })
}

/// What the world's own data says it holds: its textures' bytes and its primitives' vertices.
fn world_facts(world: &World) -> (usize, usize, usize, usize, usize) {
    use primase_core::{Mesh, Semantic, Stream, Texture};
    let textures = world.iter::<Texture>().count();
    let texture_bytes: usize = world
        .iter::<Texture>()
        .map(|(_, t)| t.levels.iter().map(Vec::len).sum::<usize>())
        .sum();
    let (mut primitives, mut vertices) = (0, 0);
    for (_, mesh) in world.iter::<Mesh>() {
        for primitive in mesh.primitives() {
            primitives += 1;
            if let Some(Stream::F32x3(positions)) = primitive.streams.get(&Semantic::Position) {
                vertices += positions.len();
            }
        }
    }
    (
        world.entity_count(),
        textures,
        texture_bytes,
        primitives,
        vertices,
    )
}

/// One area's read, stage by stage.
fn census(game: Game, label: &str, install: &Install, area: &str, decoder: &Decoder<'_>) {
    let mut world = World::new();
    let baseline = CURRENT.load(Relaxed);
    PEAK.store(baseline, Relaxed);
    STAGE.store(0, Relaxed);
    let started = Instant::now();
    // the stage that is running: its name, when it began, and the bytes held then
    let mut running: Option<(String, Instant, usize)> = None;
    let mut stage = |name: &str| {
        if let Some((previous, began, start)) = running.take() {
            println!(
                "MEMSTAGE {label} {area} stage {previous} start {start} peak {} end {} seconds {:.2}",
                PEAK.load(Relaxed),
                CURRENT.load(Relaxed),
                began.elapsed().as_secs_f64()
            );
        }
        STAGE.store(STAGES.iter().position(|s| *s == name).unwrap_or(0), Relaxed);
        let now = CURRENT.load(Relaxed);
        PEAK.store(now, Relaxed);
        running = Some((name.to_owned(), Instant::now(), now));
    };
    let read = read_game_area_staged(
        install,
        area,
        decoder,
        &Options::default(),
        &mut world,
        &mut stage,
    );
    let peak = PEAK.load(Relaxed);
    let retained = CURRENT.load(Relaxed).saturating_sub(baseline);
    match read {
        Ok(_) => {
            println!(
                "MEMDONE {label} {area} baseline {baseline} peak {peak} retained {retained} seconds {:.2}",
                started.elapsed().as_secs_f64()
            );
            let (entities, textures, texture_bytes, primitives, vertices) = world_facts(&world);
            println!(
                "MEMWORLD {label} {area} entities {entities} textures {textures} texture_bytes {texture_bytes} primitives {primitives} position_vertices {vertices}"
            );
        }
        Err(error) => println!("MEMERR {label} {area} {error}"),
    }
    let _ = game;
}

#[test]
#[ignore = "reads whole areas of the installed games: minutes and gigabytes; run it alone with --ignored --nocapture"]
fn what_an_area_read_holds_at_each_stage() -> Result<()> {
    CAP.store(cap_bytes(), Relaxed);
    println!("MEMCAP {}", CAP.load(Relaxed));
    let Some(dll) = inputs::oodle(inputs::Tool::Oodle6) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let mut read_any = false;
    for (game, label, areas) in AREAS {
        let data = match game {
            Game::Sekiro => inputs::Game::Sekiro,
            Game::EldenRing => inputs::Game::Elden,
        };
        let (Some(root), keys) = (inputs::install(data), inputs::keys(data)) else {
            continue;
        };
        let keys = match (game, keys) {
            (Game::Sekiro, _) => Keys::published(Game::Sekiro).ok_or("Sekiro's keys")?,
            (_, Some(folder)) => Keys::from_folder(folder, game)?,
            (_, None) => continue,
        };
        let install = Install::new(root, game, keys);
        // the dictionary is not needed by the read; it is listed so a census names what it could not have read
        let _ = inputs::oracle(inputs::OracleSet::Uxm, data, "dictionary.txt")
            .map(|path| NameDictionary::read_for(path, game));
        let archive = install.open()?;
        for area in areas.iter().filter(|area| wanted(label, area)) {
            if !archive.contains(&format!("/map/mapstudio/{area}.msb.dcx")) {
                println!("MEMERR {label} {area} the archives hold no such map");
                continue;
            }
            census(game, label, &install, area, &decoder);
            read_any = true;
        }
    }
    assert!(
        !read_any || PEAK.load(Relaxed) > 0,
        "the allocator saw nothing of a read that ran"
    );
    Ok(())
}
