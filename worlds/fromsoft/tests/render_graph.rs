//! The frame's render graph (`Options::render_graph`): each game's table against the recorded frames it comes from, and
//! the graph read from each install.
//!
//! The frames are the oracle: the capture proxies' records of the running games (`oracles.scripts.<game>/frames`),
//! read here by this test's own parser, separate from the script that wrote the tables, each program named by the
//! proxies' hash of its bytes ([`fnv1a64`]) as the install's engine binders hold them.
//! Every frame the table names must draw the table's steps that it has (those in every frame, and those whose frames
//! name it) in the table's order.
//!
//! Which sampler each texture slot is read through is held to Microsoft's own disassemblers' listings of the programs
//! (`oracles.scripts.<game>/sampler-pairs.json`: fxc's for Sekiro, dxc's for Elden Ring), not to the reader the graph
//! uses; each sampler state the graph states is one the frames bind at that program's register, read from the frames'
//! own records.
//!
//! Needs `installs.sekiro`, `keys.sekiro` and `tools.oodle6` (`installs.elden` and `keys.elden` for Elden Ring); the
//! frames check also `oracles.scripts.<game>/frames`, the pairing check `oracles.scripts.<game>/sampler-pairs.json`.
#![allow(clippy::unwrap_used)]
mod inputs;

use fromsoft_formats::dcx::Decoder;
use fromsoft_formats::{Archive, Binder, Game as FsGame, Keys};
use inputs::{Game, OracleSet, Tool, install, oodle, oracle};
use iridia_interface::{
    AddressMode, Compare, D3dClass, Filter, Filters, MipFilter, ResourceKind, SamplerState,
};
use primase_core::render_graph::{PassWork, Presence, RenderGraph, RenderPass};
use primase_core::shader::{InputRole, ScreenTriangleValue, Shader, StageBinding};
use primase_core::{Resource, World};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro;
use std::collections::{BTreeMap, BTreeSet};
use std::io::BufRead;
mod common;

/// The capture proxies' hash of a program's bytes: FNV-1a 64 from the seed 1469598103934665603, the standard offset
/// basis (14695981039346656037) with its last digit dropped, which the proxies were written with; the standard basis
/// matches no recorded program.
fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(1_469_598_103_934_665_603_u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The engine programs of `folders`' binders, by the proxies' hash of their bytes ([`fnv1a64`]): `<folder>/<file>`.
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

/// Every stage of the graph's screen and compute steps, a group's steps' too.
fn graph_stages(passes: &[RenderPass], out: &mut Vec<StageBinding>) {
    for p in passes {
        match &p.work {
            PassWork::Screen { stages, .. } => out.extend(stages.iter().cloned()),
            PassWork::Compute { stage, .. } => out.push(stage.clone()),
            PassWork::Group { steps } => graph_stages(steps, out),
            _ => {}
        }
    }
}

/// A stage's program: its name and interface.
fn program_of<'w>(world: &'w World, s: &StageBinding) -> (String, &'w iridia_interface::Interface) {
    let name = world.get::<primase_core::Name>(s.shader).unwrap().0.clone();
    (name, &world.get::<Shader>(s.shader).unwrap().interface)
}

/// The D3D register of a resource of `interface`, if its class is `class`.
fn register(
    interface: &iridia_interface::Interface,
    id: iridia_interface::ResourceId,
    class: D3dClass,
) -> Option<u32> {
    interface
        .resource(id)
        .and_then(|r| r.provenance.d3d)
        .filter(|d| d.class == class && d.space == 0)
        .map(|d| d.register)
}

/// Every texture slot of the graph's stages names the sampler its program reads it through where the disassembler's
/// listing shows exactly one, and none where it shows several or none (a load, or no read). Returns the slots checked
/// and those read through several samplers.
fn check_sampler_pairs(
    graph: &RenderGraph,
    world: &World,
    oracle: &std::path::Path,
) -> Result<(usize, usize), Box<dyn std::error::Error>> {
    let o: serde_json::Value = serde_json::from_slice(&std::fs::read(oracle)?)?;
    let mut stages = Vec::new();
    graph_stages(&graph.passes, &mut stages);
    let (mut checked, mut several) = (0, 0);
    for s in &stages {
        let (name, interface) = program_of(world, s);
        let listed = &o["programs"][&name];
        assert!(listed.is_object(), "{name}: not in the disassemblers' oracle");
        for t in &s.textures {
            let Some(reg) = register(interface, t.resource, D3dClass::ShaderResource) else {
                continue;
            };
            let samplers: Vec<u32> = listed["sampled"][format!("t{reg}")]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|v| v.as_str().unwrap().trim_start_matches('s').parse().unwrap())
                        .collect()
                })
                .unwrap_or_default();
            let expected = match samplers.as_slice() {
                [one] => interface
                    .resources
                    .iter()
                    .find(|r| {
                        matches!(r.kind, ResourceKind::Sampler { .. })
                            && register(interface, r.id, D3dClass::Sampler) == Some(*one)
                    })
                    .map(|r| r.id),
                _ => None,
            };
            assert_eq!(
                t.sampler, expected,
                "{name} t{reg}: the listing reads it through {samplers:?}"
            );
            checked += 1;
            several += usize::from(samplers.len() > 1);
        }
    }
    Ok((checked, several))
}

/// A sampler the frames record, as D3D11's description and D3D12's descriptor number it (their enums agree), read
/// here as the state it is: filter bits 0, 2 and 4 linear mip, magnification and minification, bit 6 anisotropic, bit
/// 7 a comparison; address modes 1 wrap, 2 mirror, 3 clamp, 4 border, 5 mirror once; comparisons 1 never to 8 always.
fn recorded_state(
    filter: u64,
    address: [u64; 3],
    anisotropy: u64,
    compare: u64,
    bias: f64,
    border: [f64; 4],
    lod: [f64; 2],
) -> SamplerState {
    let level = |bit: u64| {
        if filter >> bit & 1 == 1 {
            Filter::Linear
        } else {
            Filter::Nearest
        }
    };
    let mode = |a: u64| match a {
        1 => AddressMode::Repeat,
        2 => AddressMode::Mirror,
        3 => AddressMode::Clamp,
        4 => AddressMode::Border,
        5 => AddressMode::MirrorOnce,
        other => panic!("address mode {other}"),
    };
    let comparison = [
        Compare::Never,
        Compare::Less,
        Compare::Equal,
        Compare::LessEqual,
        Compare::Greater,
        Compare::NotEqual,
        Compare::GreaterEqual,
        Compare::Always,
    ];
    SamplerState {
        wrap: address.map(|a| Some(mode(a))),
        filters: Filters {
            mag: Some(level(2)),
            min: Some(level(4)),
            mip: Some(if filter & 1 == 1 {
                MipFilter::Linear
            } else {
                MipFilter::Nearest
            }),
        },
        max_anisotropy: (filter & 0x40 != 0).then(|| u8::try_from(anisotropy).unwrap()),
        compare: (filter & 0x80 != 0).then(|| comparison[usize::try_from(compare).unwrap() - 1]),
        mip_lod_bias: (bias != 0.0).then_some(bias as f32),
        border: Some(border.map(|c| c as f32)),
        lod: Some(lod.map(|l| l as f32)),
    }
}

/// Sampler states by program name, stage (`vertex`, `pixel`, `compute`) and register.
type SamplerStates = BTreeMap<(String, &'static str, u32), Vec<SamplerState>>;

/// The sampler states the frames bind: per program name, stage (`vertex`, `pixel`, `compute`) and register, every
/// state some draw or dispatch of the frames' sessions binds there.
fn frame_sampler_states(
    game: FsGame,
    root: &std::path::Path,
    frames: &[String],
    names: &BTreeMap<u64, String>,
) -> Result<SamplerStates, Box<dyn std::error::Error>> {
    let hash = |v: &serde_json::Value| v.as_str().and_then(|h| u64::from_str_radix(h, 16).ok());
    let f = |v: &serde_json::Value| v.as_f64().unwrap();
    let u = |v: &serde_json::Value| v.as_u64().unwrap();
    let mut out: BTreeMap<(String, &'static str, u32), Vec<SamplerState>> = BTreeMap::new();
    let mut add = |name: Option<&String>, stage: &'static str, reg: u32, state: SamplerState| {
        if let Some(name) = name {
            let states = out.entry((name.clone(), stage, reg)).or_default();
            if !states.contains(&state) {
                states.push(state);
            }
        }
    };
    let runs: BTreeSet<&str> = frames
        .iter()
        .filter_map(|f| f.split_once(' ').map(|(r, _)| r))
        .collect();
    for run in runs {
        match game {
            FsGame::Sekiro => {
                // the sampler objects of the session's frames, then each frame's draws and dispatches
                let mut records = Vec::new();
                for frame in frames.iter().filter(|f| f.starts_with(&format!("{run} "))) {
                    let n = frame.split_once(' ').unwrap().1;
                    let file = std::fs::File::open(root.join(run).join("cap").join(n).join("events.jsonl"))?;
                    for line in std::io::BufReader::new(file).lines() {
                        if let Ok(e) = serde_json::from_str::<serde_json::Value>(&line?) {
                            records.push(e);
                        }
                    }
                }
                let samplers: BTreeMap<String, &serde_json::Value> = records
                    .iter()
                    .filter(|e| e["ev"] == "sampler")
                    .filter_map(|e| Some((e["id"].as_str()?.to_owned(), e)))
                    .collect();
                for e in &records {
                    for (key, stage) in [("vs", "vertex"), ("ps", "pixel"), ("cs", "compute")] {
                        let Some(bound) = e[key]["smp"].as_array() else {
                            continue;
                        };
                        for b in bound {
                            let (Some(reg), Some(r)) =
                                (b[0].as_u64(), b[1].as_str().and_then(|id| samplers.get(id)))
                            else {
                                continue;
                            };
                            let a = &r["address"];
                            let state = recorded_state(
                                u(&r["filter"]),
                                [u(&a[0]), u(&a[1]), u(&a[2])],
                                u(&r["aniso"]),
                                u(&r["compare"]),
                                f(&r["lod_bias"]),
                                [
                                    f(&r["border"][0]),
                                    f(&r["border"][1]),
                                    f(&r["border"][2]),
                                    f(&r["border"][3]),
                                ],
                                [f(&r["lod"][0]), f(&r["lod"][1])],
                            );
                            add(
                                hash(&e[key]["hash"]).and_then(|h| names.get(&h)),
                                stage,
                                u32::try_from(reg)?,
                                state,
                            );
                        }
                    }
                }
            }
            FsGame::EldenRing => {
                let file = std::fs::File::open(root.join(run).join("cap").join("capture.jsonl"))?;
                for line in std::io::BufReader::new(file).split(b'\n') {
                    let line = line?;
                    if !(line.starts_with(b"{\"event\":\"draw\"")
                        || line.starts_with(b"{\"event\":\"dispatch\""))
                    {
                        continue;
                    }
                    let Ok(e) = serde_json::from_slice::<serde_json::Value>(&line) else {
                        continue;
                    };
                    for p in e["root"].as_array().into_iter().flatten() {
                        // the parameter's visibility: 0 every stage, 1 vertex, 5 pixel
                        let stages: &[(&str, &'static str)] = match p["vis"].as_u64() {
                            Some(0) => &[("vs", "vertex"), ("ps", "pixel"), ("cs", "compute")],
                            Some(1) => &[("vs", "vertex")],
                            Some(5) => &[("ps", "pixel")],
                            _ => &[],
                        };
                        for entry in p["entries"].as_array().into_iter().flatten() {
                            if entry["t"] != "smp" || entry["space"].as_u64() != Some(0) {
                                continue;
                            }
                            let d = &entry["desc"];
                            let state = recorded_state(
                                u(&d["filter"]),
                                [u(&d["u"]), u(&d["v"]), u(&d["w"])],
                                u(&d["aniso"]),
                                u(&d["cmp"]),
                                f(&d["bias"]),
                                [
                                    f(&d["border"][0]),
                                    f(&d["border"][1]),
                                    f(&d["border"][2]),
                                    f(&d["border"][3]),
                                ],
                                [f(&d["min_lod"]), f(&d["max_lod"])],
                            );
                            let reg = u32::try_from(u(&entry["reg"]))?;
                            for &(key, stage) in stages {
                                add(hash(&e[key]).and_then(|h| names.get(&h)), stage, reg, state);
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Every sampler state the graph's stages state is one the frames bind at that program's stage and register. Returns
/// the states checked.
fn check_sampler_states(
    game: FsGame,
    graph: &RenderGraph,
    world: &World,
    archive: &Archive,
    decoder: &Decoder<'_>,
    frames_root: &std::path::Path,
    frames: &[String],
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut stages = Vec::new();
    graph_stages(&graph.passes, &mut stages);
    let folders: BTreeSet<String> = stages
        .iter()
        .filter_map(|s| program_of(world, s).0.split_once('/').map(|(f, _)| f.to_owned()))
        .collect();
    let names = program_names(archive, decoder, &folders)?;
    let recorded = frame_sampler_states(game, frames_root, frames, &names)?;
    let mut checked = 0;
    for s in &stages {
        let (name, interface) = program_of(world, s);
        let stage = match s.stage {
            iridia_interface::Stage::Vertex => "vertex",
            iridia_interface::Stage::Pixel => "pixel",
            _ => "compute",
        };
        for b in &s.samplers {
            let reg = register(interface, b.resource, D3dClass::Sampler).unwrap();
            let seen = recorded
                .get(&(name.clone(), stage, reg))
                .cloned()
                .unwrap_or_default();
            assert!(
                seen.contains(&b.state),
                "{name} {stage} s{reg}: the graph states {:?}; the frames bind {seen:?}",
                b.state
            );
            checked += 1;
        }
    }
    Ok(checked)
}

/// The read graph verifies against its programs, and what its report says of it.
fn read_and_verify(
    install: &fromsoft_formats::Install,
    area: &str,
    region: [f32; 4],
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
        let states = check_sampler_states(
            FsGame::Sekiro,
            graph,
            &world,
            &install.open()?,
            &decoder,
            &frames,
            &table_frames,
        )?;
        assert_eq!(states, report.render_graph.samplers);
        assert!(states > 0);
    }
    // the graph's programs read t36 of the subsurface blurs and t3 of the ambient occlusion's reprojection through two
    // samplers each, as the listings show
    if let Some(pairs) = oracle(OracleSet::Scripts, Game::Sekiro, "sampler-pairs.json") {
        let (checked, several) = check_sampler_pairs(graph, &world, &pairs)?;
        assert!(checked > 0);
        assert_eq!(several, report.render_graph.textures_through_several_samplers);
    }
    assert_eq!(report.render_graph.untraced_reads, 0);
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
        let states = check_sampler_states(
            FsGame::EldenRing,
            graph,
            &world,
            &install.open()?,
            &decoder,
            &frames,
            &table_frames,
        )?;
        assert_eq!(states, report.render_graph.samplers);
        assert!(states > 0);
    }
    if let Some(pairs) = oracle(OracleSet::Scripts, Game::Elden, "sampler-pairs.json") {
        let (checked, several) = check_sampler_pairs(graph, &world, &pairs)?;
        assert!(checked > 0);
        assert_eq!(several, report.render_graph.textures_through_several_samplers);
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
