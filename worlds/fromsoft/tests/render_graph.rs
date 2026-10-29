//! The frame's render graph (`Options::render_graph`): each game's table against the recorded frames it comes from, and
//! the graph read from each install.
//!
//! The frames are the oracle: the capture proxies' records of the running games (`oracles.scripts.<game>/frames`),
//! read here by this test's own parser, separate from the script that wrote the tables, each program named by the
//! FNV-1a 64 of its bytes as the install's engine binders hold them (the proxies hash a program's bytes the same way).
//! Every frame the table names must draw the table's steps that it has (those in every frame, and those whose frames
//! name it) in the table's order.
//!
//! Needs `installs.sekiro`, `keys.sekiro` and `tools.oodle6` (`installs.elden` and `keys.elden` for Elden Ring); the
//! frames check also `oracles.scripts.<game>/frames`.
#![allow(clippy::unwrap_used)]
mod inputs;

use fromsoft_formats::dcx::Decoder;
use fromsoft_formats::{Archive, Binder, Game as FsGame, Keys};
use inputs::{Game, OracleSet, Tool, install, oodle, oracle};
use primase_core::render_graph::{PassWork, Presence, RenderGraph, RenderPass};
use primase_core::shader::{InputRole, ScreenTriangleValue};
use primase_core::{Resource, World};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro;
use std::collections::{BTreeMap, BTreeSet};
use std::io::BufRead;
mod common;

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The engine programs of `folders`' binders, by the FNV-1a 64 of their bytes: `<folder>/<file>`.
fn program_names(
    archive: &Archive,
    decoder: &Decoder<'_>,
    folders: &BTreeSet<String>,
) -> Result<BTreeMap<u64, String>, Box<dyn std::error::Error>> {
    let mut out = BTreeMap::new();
    for folder in folders {
        let binder = Binder::read(
            &archive.read(&format!("/shader/{folder}.shaderbnd.dcx"), decoder)?,
            decoder,
        )?;
        for f in binder.files {
            let name = f.name.as_deref().unwrap_or("");
            let leaf = name.rsplit(['/', '\\']).next().unwrap_or(name);
            out.insert(fnv1a64(&f.bytes), format!("{folder}/{leaf}"));
        }
    }
    Ok(out)
}

/// The engine program each step draws or dispatches with (a screen step's pixel programs, any of its variants; a
/// compute step's program), with whether the frame `frame` must have it: in every frame, or a step whose frames name
/// `frame`. A group's own steps and a step only some frames have by a trigger are not required.
fn table_programs(
    passes: &[RenderPass],
    frame: &str,
    required: bool,
    out: &mut Vec<(BTreeSet<String>, bool)>,
) {
    for p in passes {
        let needed = required
            && match &p.presence {
                Presence::Always => true,
                Presence::Unknown { frames } => frames.iter().any(|f| f == frame),
                Presence::When(_) => false,
            };
        match &p.work {
            PassWork::Group { steps } => table_programs(steps, frame, false, out),
            _ => {
                if let Some(names) = names_of(p) {
                    out.push((names, needed));
                }
            }
        }
    }
}

thread_local! {
    static SHADER_NAMES: std::cell::RefCell<BTreeMap<primase_core::Entity, String>> = Default::default();
}

fn names_of(p: &RenderPass) -> Option<BTreeSet<String>> {
    SHADER_NAMES
        .with(|names| {
            let names = names.borrow();
            match &p.work {
                PassWork::Screen { stages, .. } => Some(
                    stages
                        .iter()
                        .filter(|s| s.stage == iridia_interface::Stage::Pixel)
                        .filter_map(|s| names.get(&s.shader).cloned())
                        .collect(),
                ),
                PassWork::Compute { stage, .. } => {
                    names.get(&stage.shader).cloned().map(|n| BTreeSet::from([n]))
                }
                _ => None,
            }
        })
        .filter(|s: &BTreeSet<String>| !s.is_empty())
}

/// A recorded frame's engine draws and dispatches in order, each by its pixel (or compute) program's name, runs of one
/// program as one.
fn frame_programs(
    game: FsGame,
    root: &std::path::Path,
    frame: &str,
    names: &BTreeMap<u64, String>,
    psos: &mut BTreeMap<String, BTreeMap<u64, u64>>,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let (run, n) = frame.split_once(' ').ok_or("a frame name without its run")?;
    let hash = |h: &str| u64::from_str_radix(h, 16).ok();
    let mut out: Vec<String> = Vec::new();
    let mut push = |name: Option<&String>| {
        if let Some(name) = name
            && out.last() != Some(name)
        {
            out.push(name.clone());
        }
    };
    match game {
        FsGame::Sekiro => {
            let file = std::fs::File::open(root.join(run).join("cap").join(n).join("events.jsonl"))?;
            for line in std::io::BufReader::new(file).lines() {
                let Ok(e) = serde_json::from_str::<serde_json::Value>(&line?) else {
                    continue;
                };
                let ev = e["ev"].as_str().unwrap_or("");
                if !(ev.starts_with("Draw") || ev.starts_with("Dispatch")) {
                    continue;
                }
                let program = ["ps", "cs"]
                    .iter()
                    .find_map(|s| e[*s]["hash"].as_str().and_then(hash));
                push(program.and_then(|h| names.get(&h)));
            }
        }
        FsGame::EldenRing => {
            let cap = root.join(run).join("cap");
            let key = cap.to_string_lossy().into_owned();
            if !psos.contains_key(&key) {
                // the pipeline states the frames name: their pixel or compute program
                let mut map = BTreeMap::new();
                let file = std::fs::File::open(cap.join("capture.jsonl"))?;
                for line in std::io::BufReader::new(file).split(b'\n') {
                    let line = line?;
                    if !line.starts_with(b"{\"event\":\"pso\"") {
                        continue;
                    }
                    let Ok(e) = serde_json::from_slice::<serde_json::Value>(&line) else {
                        continue;
                    };
                    let program = ["ps", "cs"]
                        .iter()
                        .find_map(|s| e["shaders"][*s]["hash"].as_str().and_then(hash));
                    if let (Some(id), Some(p)) = (e["id"].as_u64(), program) {
                        map.insert(id, p);
                    }
                }
                psos.insert(key.clone(), map);
            }
            let map = &psos[&key];
            let file = std::fs::File::open(cap.join(format!("{n}.jsonl")))?;
            for line in std::io::BufReader::new(file).lines() {
                let Ok(e) = serde_json::from_str::<serde_json::Value>(&line?) else {
                    continue;
                };
                if !matches!(e["ev"].as_str(), Some("draw" | "dispatch")) {
                    continue;
                }
                push(
                    e["pso"]
                        .as_u64()
                        .and_then(|p| map.get(&p))
                        .and_then(|h| names.get(h)),
                );
            }
        }
    }
    Ok(out)
}

/// Every frame the graph's table names holds its required steps' programs in the table's order (runs of one program as
/// one); returns how many frames were checked.
fn check_frames(
    game: FsGame,
    graph: &RenderGraph,
    world: &World,
    archive: &Archive,
    decoder: &Decoder<'_>,
    frames_root: &std::path::Path,
    frames: &[String],
) -> Result<usize, Box<dyn std::error::Error>> {
    let shader_names: BTreeMap<primase_core::Entity, String> = world
        .iter::<primase_core::shader::Shader>()
        .filter_map(|(e, _)| world.get::<primase_core::Name>(e).map(|n| (e, n.0.clone())))
        .collect();
    let folders: BTreeSet<String> = shader_names
        .values()
        .filter_map(|n| n.split_once('/').map(|(f, _)| f.to_owned()))
        .collect();
    SHADER_NAMES.with(|n| *n.borrow_mut() = shader_names);
    let names = program_names(archive, decoder, &folders)?;
    let mut psos = BTreeMap::new();
    for frame in frames {
        let drawn = frame_programs(game, frames_root, frame, &names, &mut psos)?;
        let mut wanted = Vec::new();
        table_programs(&graph.passes, frame, true, &mut wanted);
        let mut required: Vec<BTreeSet<String>> = Vec::new();
        for (names, needed) in wanted {
            if needed && required.last() != Some(&names) {
                required.push(names);
            }
        }
        let mut at = 0;
        for (i, step) in required.iter().enumerate() {
            let found = drawn[at..].iter().position(|d| step.contains(d));
            let Some(k) = found else {
                panic!(
                    "{game:?} {frame}: required step {i} ({step:?}) not drawn after position {at} of {}; drawn there: {:?}",
                    drawn.len(),
                    &drawn[at..drawn.len().min(at + 8)]
                );
            };
            at += k + 1;
        }
    }
    Ok(frames.len())
}

/// The read graph verifies against its programs, and what its report says of it.
fn read_and_verify(
    install: &fromsoft_formats::Install,
    area: &str,
    region: [f64; 4],
    decoder: &Decoder<'_>,
) -> Result<(World, sekiro::Report), Box<dyn std::error::Error>> {
    let mut world = World::default();
    let report = sekiro::read_game_area(
        install,
        area,
        decoder,
        &sekiro::Options {
            region: Some(sekiro::Region {
                rect: region,
                margin: 0.0,
            }),
            render_graph: true,
            ..Default::default()
        },
        &mut world,
    )?;
    let graph = world.resource::<RenderGraph>().ok_or("no core.render_graph")?;
    Resource::verify(graph, &world)?;
    assert!(report.render_graph.asked);
    assert_eq!(
        (
            report.render_graph.steps,
            report.render_graph.targets,
            report.render_graph.buffers
        ),
        (graph.passes.len(), graph.targets.len(), graph.buffers.len())
    );
    Ok((world, report))
}

/// The full-screen inputs the graph's vertex stages bind: Sekiro's frames record the full-screen triangle's bytes, Elden
/// Ring's none.
fn screen_triangle_inputs(graph: &RenderGraph) -> usize {
    fn walk(passes: &[RenderPass], n: &mut usize) {
        for p in passes {
            match &p.work {
                PassWork::Screen { stages, .. } => {
                    for s in stages {
                        *n += s
                            .inputs
                            .iter()
                            .filter(|i| {
                                matches!(
                                    i.role,
                                    InputRole::ScreenTriangle(ScreenTriangleValue::Position)
                                        | InputRole::ScreenTriangle(ScreenTriangleValue::TextureCoordinate)
                                )
                            })
                            .count();
                    }
                }
                PassWork::Group { steps } => walk(steps, n),
                _ => {}
            }
        }
    }
    let mut n = 0;
    walk(&graph.passes, &mut n);
    n
}

#[test]
fn sekiros_render_graph_is_its_recorded_frames() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = install(Game::Sekiro) else {
        return Ok(());
    };
    let Some(install) = common::sekiro(&root)? else {
        return Ok(());
    };
    let Some(dll) = oodle(Tool::Oodle6) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let (world, report) = read_and_verify(&install, "m11_01", [-170.0, 180.0, -50.0, 300.0], &decoder)?;
    let graph = world.resource::<RenderGraph>().unwrap();
    // nine frames of four sessions at 1920 x 1080; the picture the last tone map's RGB10A2 target
    assert_eq!(
        (report.render_graph.frames, report.render_graph.view),
        (9, [1920, 1080])
    );
    let picture = graph.picture.unwrap() as usize;
    assert_eq!(
        graph.targets[picture].format,
        primase_core::render_graph::TargetFormat::Rgb10A2Unorm
    );
    assert!(screen_triangle_inputs(graph) > 0);
    if let Some(frames) = oracle(OracleSet::Scripts, Game::Sekiro, "frames") {
        let table_frames = frames_of(FsGame::Sekiro);
        let checked = check_frames(
            FsGame::Sekiro,
            graph,
            &world,
            &install.open()?,
            &decoder,
            &frames,
            &table_frames,
        )?;
        assert_eq!(checked, 9);
    }
    Ok(())
}

#[test]
fn elden_rings_render_graph_is_its_recorded_frames() -> Result<(), Box<dyn std::error::Error>> {
    let (Some(root), Some(keys), Some(dll)) = (
        install(Game::Elden),
        inputs::keys(Game::Elden),
        oodle(Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let install = common::install(
        root,
        FsGame::EldenRing,
        Keys::from_folder(keys, FsGame::EldenRing, FsGame::EldenRing.param_key_sha256())?,
    );
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let (world, report) = read_and_verify(&install, "m60_42_36_00", [-10.0, -10.0, 10.0, 10.0], &decoder)?;
    let graph = world.resource::<RenderGraph>().unwrap();
    assert_eq!(
        (report.render_graph.frames, report.render_graph.view),
        (7, [2560, 1440])
    );
    let picture = graph.picture.unwrap() as usize;
    assert_eq!(
        graph.targets[picture].format,
        primase_core::render_graph::TargetFormat::Rgb10A2Unorm
    );
    // the capture records no vertex buffer's bytes: no input is the full-screen triangle
    assert_eq!(screen_triangle_inputs(graph), 0);
    if let Some(frames) = oracle(OracleSet::Scripts, Game::Elden, "frames") {
        let table_frames = frames_of(FsGame::EldenRing);
        let checked = check_frames(
            FsGame::EldenRing,
            graph,
            &world,
            &install.open()?,
            &decoder,
            &frames,
            &table_frames,
        )?;
        assert_eq!(checked, 7);
    }
    Ok(())
}

/// The frames a game's table names (`<run> frame_<n>`), from the table as stored.
fn frames_of(game: FsGame) -> Vec<String> {
    let text = match game {
        FsGame::Sekiro => include_str!("../src/sekiro/read/render_graph/sekiro.json"),
        FsGame::EldenRing => include_str!("../src/sekiro/read/render_graph/elden.json"),
    };
    let v: serde_json::Value = serde_json::from_str(text).unwrap();
    v["frames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap().to_owned())
        .collect()
}
