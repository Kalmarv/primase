//! A binding census per game: each game's real scenes read, its resolve pass run, and the bindings counted by what is
//! stated and what is not. One line per scene is printed, `CENSUS {json}`, for the notes' table:
//!
//! - the materials, the bound (a `core.shader_binding`), and the pass's own counts and approximations (the unbound by
//!   named reason are among them);
//! - the stages, by stage, with the inputs, constants and outputs left Unknown by reason, the output lanes a program
//!   writes that no output binding states (its D3D signature's written lanes), and the texture slots by what they hold
//!   (a texture, a stated default, nothing);
//! - the render state stated (which parts) against the default.
//!
//! A game with no resolve pass is a line with its materials and programs only. Needs the installs it names; a scene
//! whose install is not configured prints nothing. Run it as one job: `cargo test -p primase-checks --test
//! binding_census -- --nocapture --test-threads=1`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::too_many_lines)]
mod inputs;

use std::collections::{BTreeMap, BTreeSet};

use iridia_interface::{ConstantRole, InputRole, OutputRole, RenderState};
use primase_core::pass::{Options, PassLog, PassManager, PassRegistry, RunReport};
use primase_core::{Component, World};
use primase_core::{shader::ShaderBinding, shader::TextureSource};
use serde_json::{Value, json};

/// A reason string with the hashes that vary by material taken out, so equal reasons count together.
fn reason(why: &str) -> String {
    let mut out = String::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.len() == 8
            && run.chars().all(|c| c.is_ascii_hexdigit())
            && run.chars().any(|c| c.is_ascii_alphabetic())
        {
            out.push_str("<hash>");
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in why.chars() {
        if c.is_ascii_alphanumeric() {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

fn bump(map: &mut BTreeMap<String, u64>, key: impl Into<String>) {
    *map.entry(key.into()).or_default() += 1;
}

/// What the bindings of `world` state, and what they leave Unknown or unbound, as JSON.
fn tally(world: &World, game: &str, scene: &str, materials: usize, log: &PassLog) -> Value {
    let mut stages: BTreeMap<String, u64> = BTreeMap::new();
    let mut stages_unknown_inputs: BTreeMap<String, u64> = BTreeMap::new();
    let mut inputs: BTreeMap<String, u64> = BTreeMap::new();
    let mut constants: BTreeMap<String, u64> = BTreeMap::new();
    let mut constants_total = 0u64;
    let mut outputs: BTreeMap<String, u64> = BTreeMap::new();
    let mut outputs_total = 0u64;
    let mut unbound_written: BTreeMap<String, u64> = BTreeMap::new();
    let mut textures: BTreeMap<String, u64> = BTreeMap::new();
    let mut state_fields: BTreeMap<String, u64> = BTreeMap::new();
    let (mut bound, mut state_default, mut state_stated) = (0u64, 0u64, 0u64);
    let mut variants = 0u64;
    // the shaders' interfaces verify too (the resource, register and colour-output rules), counted with the bindings'
    for (component, _, why) in world.verify_components() {
        if component == "core.shader" {
            bump(
                &mut state_fields,
                format!("SHADER FAILS VERIFY: {}", reason(&why)),
            );
        }
    }
    for (e, b) in world.iter::<ShaderBinding>() {
        if let Err(why) = b.verify(e, world) {
            bump(
                &mut state_fields,
                format!("BINDING FAILS VERIFY: {}", reason(&why)),
            );
        }
        bound += 1;
        for s in &b.stages {
            let name = format!("{:?}", s.stage);
            bump(&mut stages, name.clone());
            variants += u64::from(s.variant.is_some());
            let mut unknown_here = false;
            for i in &s.inputs {
                if let InputRole::Unknown { why } = &i.role {
                    unknown_here = true;
                    bump(&mut inputs, format!("{name}: {}", reason(why)));
                }
            }
            if unknown_here {
                bump(&mut stages_unknown_inputs, name.clone());
            }
            for c in &s.constants {
                constants_total += 1;
                if let ConstantRole::Unknown { why } = &c.role {
                    bump(&mut constants, format!("{name}: {}", reason(why)));
                }
            }
            for o in &s.outputs {
                outputs_total += 1;
                if let OutputRole::Unknown { why } = &o.role {
                    bump(
                        &mut outputs,
                        format!(
                            "{name}: {}{} lanes {:04b}: {}",
                            o.label.name,
                            o.label.index,
                            o.lanes,
                            reason(why)
                        ),
                    );
                }
            }
            // the lanes the program writes that no output binding states
            if let Some(shader) = world.get::<primase_core::shader::Shader>(s.shader) {
                for (label, lanes) in s.unbound_written_lanes(&shader.interface) {
                    bump(
                        &mut unbound_written,
                        format!("{name}: {}{} lanes {lanes:04b}", label.name, label.index),
                    );
                }
            }
            for t in &s.textures {
                match &t.source {
                    TextureSource::Texture { .. } => bump(&mut textures, "bound texture"),
                    TextureSource::Default { why, .. } => {
                        bump(&mut textures, format!("default: {}", reason(why)))
                    }
                    TextureSource::Frame(_) => bump(&mut textures, "the frame's texture"),
                    TextureSource::Missing => bump(&mut textures, "missing"),
                }
            }
        }
        if b.state == RenderState::default() {
            state_default += 1;
        } else {
            state_stated += 1;
            let s = &b.state;
            for (field, on) in [
                ("blend", s.blend.is_some()),
                ("depth", s.depth.is_some()),
                ("cull", s.cull.is_some()),
                ("alpha_test", s.alpha_test.is_some()),
                ("targets", s.targets.iter().any(Option::is_some)),
                ("stencil", s.stencil.is_some()),
                ("depth_bias", s.depth_bias.is_some()),
                ("fill", s.fill.is_some()),
                ("winding", s.front_counter_clockwise.is_some()),
            ] {
                if on {
                    bump(&mut state_fields, field);
                }
            }
        }
    }
    json!({
        "game": game, "scene": scene, "materials": materials, "bound": bound,
        "unbound": materials as u64 - bound.min(materials as u64),
        "variant_stages": variants,
        "stages": stages, "stages_with_unknown_inputs": stages_unknown_inputs,
        "unknown_inputs": inputs,
        "constants_total": constants_total, "unknown_constants": constants,
        "outputs_total": outputs_total, "unknown_outputs": outputs, "unbound_written_lanes": unbound_written,
        "textures": textures,
        "state_default": state_default, "state_stated": state_stated, "state_fields": state_fields,
        "pass_counts": log.counts, "pass_approximations": log.approximations,
    })
}

fn print(v: &Value) {
    println!("CENSUS {v}");
}

// ---- GTA V Enhanced: Vespucci Beach ----

#[test]
fn gta_vespucci() {
    use inputs::{Game, install, keys};
    use primase_gta::components::GtaShader;
    use primase_gta::passes::resolve_shaders::RESOLVE_GTA_SHADERS;
    use primase_gta::read::source::GtaSource;
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).unwrap();
    let (read, _) = source.read([-1500.0, -1650.0, -1150.0, -1330.0], 600.0).unwrap();
    let (mut world, _): (World, _) = source.load_all(&read).unwrap();
    let materials = world.iter::<GtaShader>().count();
    source.add_shader_database(&mut world).unwrap();
    let technique = "deferred_draw";
    {
        let options = Options::from([
            ("technique".to_owned(), technique.to_owned()),
            ("unset-textures".to_owned(), "white".to_owned()),
        ]);
        let mut w = world.clone();
        let mut log = PassLog::default();
        RESOLVE_GTA_SHADERS
            .instance(&options)
            .unwrap()
            .run(&mut w, &mut log)
            .unwrap();
        print(&tally(
            &w,
            "GTA V Enhanced",
            &format!("Vespucci, {technique}"),
            materials,
            &log,
        ));
    }
}

// ---- Riders Republic: the cityschool campus and the skate bowls ----

#[test]
fn rr_scenes() {
    use inputs::{Game, install};
    use primase_anvil::components::AnvilMaterial;
    use primase_anvil::passes::RESOLVE_ANVIL_SHADERS;
    use primase_anvil::read::convert::{
        AnvilOptions, AnvilReader, CellSelection, Edition, ForgeFile, GridSelection, Levels,
    };
    use primase_anvil::read::placements::{InstancedCollision, InstancedLevels};
    use primase_core::Instance;
    use primase_core::convert::Reader;
    use primase_core::frame::SourceFrame;
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    for (scene, rect, grids) in [
        (
            "cityschool campus",
            [-3584.0, 1152.0, -3456.0, 1280.0],
            vec!["ShortRange", "GRN_Template_TerrainWorldSmall"],
        ),
        (
            "skate bowls",
            [-7552.0, -5504.0, -7168.0, -5120.0],
            vec!["ShortRange", "GRN_Template_TerrainWorldSmall"],
        ),
    ] {
        let options = AnvilOptions {
            forges: [
                ("DataPC_MAD_USNAT_patch_01.forge", Edition::Patch),
                ("DataPC_MAD_USNAT.forge", Edition::Base),
                ("DataPC_patch_01.forge", Edition::Patch),
                ("DataPC.forge", Edition::Base),
            ]
            .map(|(f, edition)| ForgeFile {
                path: dir.join(f),
                edition,
            })
            .to_vec(),
            frame: SourceFrame::ENU,
            grids: grids
                .iter()
                .map(|g| GridSelection {
                    grid: (*g).into(),
                    cells: CellSelection::Over {
                        rect,
                        levels: Levels::Finest,
                    },
                })
                .collect(),
            instanced_levels: InstancedLevels::Every,
            instanced_collision: InstancedCollision::PerInstance,
            terrain: None,
        };
        let mut reader = AnvilReader::open(&options).unwrap();
        let (read, _) = reader.read().unwrap();
        let roots: BTreeSet<_> = read.iter::<Instance>().map(|(e, _)| e).collect();
        let (mut world, _) = reader.load(&read, &roots).unwrap();
        let materials = world.iter::<AnvilMaterial>().count();
        let options = Options::from([("unset-textures".to_owned(), "white".to_owned())]);
        let mut log = PassLog::default();
        RESOLVE_ANVIL_SHADERS
            .instance(&options)
            .unwrap()
            .run(&mut world, &mut log)
            .unwrap();
        print(&tally(&world, "Riders Republic", scene, materials, &log));
    }
}

// ---- Unity: SkateBIRD level1, Sledding Game level1 ----

fn unity_scene(game: &str, label: &str, data: &std::path::Path, tpk: &std::path::Path, level: &str) {
    use primase_unity::components::{
        UnityLight, UnityMaterial, UnityPrograms, UnityRenderSettings, UnityRenderer,
    };
    use primase_unity::level::{LevelOptions, read_level_with};
    use primase_unity::program::read_programs;
    use primase_unity::resolve_shaders::RESOLVE_UNITY_SHADERS;
    use unity_formats::TypeTreePackage;
    use unity_formats::player::Player;
    use unity_formats::player::RenderPipeline;
    let player = Player::open(
        data,
        TypeTreePackage::parse(&std::fs::read(tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let mut w = World::new();
    // the quality level is a runtime setting: the highest the player lists
    let quality = player.quality_levels().unwrap().last().map(|l| l.name.clone());
    read_level_with(&player, level, &LevelOptions { quality }, &mut w).unwrap();
    let pipeline = w
        .resource::<UnityRenderSettings>()
        .and_then(|s| s.pipeline.clone())
        .unwrap_or(RenderPipeline::BuiltIn);
    let shaders: BTreeSet<String> = w
        .iter::<UnityMaterial>()
        .filter_map(|(_, m)| m.shader.clone())
        .collect();
    let names: Vec<&str> = shaders.iter().map(String::as_str).collect();
    read_programs(&player, &mut w, &names, &pipeline).unwrap();
    let materials = w.iter::<UnityMaterial>().count();
    let roots: BTreeSet<_> = w.iter::<UnityPrograms>().map(|(e, _)| e).collect();
    let closure = w.closure(roots.iter().copied());
    // the lights and renderers state the scene's draws (`keywords=scene`); their meshes are not needed
    let lights: Vec<_> = w.iter::<UnityLight>().map(|(_, l)| l.clone()).collect();
    let renderers: Vec<_> = w.iter::<UnityRenderer>().map(|(_, r)| r.clone()).collect();
    let mut w = w.subset(&closure);
    for l in lights {
        let e = w.spawn();
        w.insert(e, l);
    }
    for r in renderers {
        let e = w.spawn();
        w.insert(e, r);
    }
    let tiers: BTreeSet<usize> = w
        .iter::<UnityPrograms>()
        .flat_map(|(_, p)| p.variants.iter().map(|v| v.tier))
        .collect();
    // the first tier, with no keywords and with the scene's own (`scene`): the keywords decide which fragment variants
    // exist
    for tier in tiers.iter().take(1) {
        for keywords in ["none", "scene"] {
            let mut world = w.clone();
            let options = Options::from([
                ("tier".to_owned(), tier.to_string()),
                ("keywords".to_owned(), keywords.to_owned()),
                ("colour-space".to_owned(), "linear".to_owned()),
            ]);
            let mut log = PassLog::default();
            RESOLVE_UNITY_SHADERS
                .instance(&options)
                .unwrap()
                .run(&mut world, &mut log)
                .unwrap();
            let scene = format!("{label} {level} ({pipeline:?}, tier {tier}, keywords {keywords})");
            print(&tally(&world, game, &scene, materials, &log));
        }
    }
}

#[test]
fn unity_levels() {
    use inputs::{Game, Tool, install_file, tool};
    let Some(tpk) = tool(Tool::UnityTpk) else {
        return;
    };
    if let Some(data) = install_file(Game::Skatebird, "SkateBIRD_Data") {
        unity_scene("SkateBIRD", "SkateBIRD", &data, &tpk, "level1");
    }
    if let Some(data) = install_file(Game::Sledding, "Sledding Game_Data") {
        unity_scene("Sledding Game", "Sledding Game", &data, &tpk, "level1");
    }
}

// ---- FromSoft: Sekiro m11_01, Elden Ring m14 and Liurnia ----

/// The geometry the earlier reader produced by itself, as explicit choices (the area tests' legacy pipeline).
const LEGACY_GEOMETRY: &str = "drop-fromsoft-far, fromsoft-apply-rigid-skinning, \
    lower-fromsoft-geometry{pose=rigid,uv-tiling=apply,winding=normals,degenerates=drop,leave-out=sky+light-volume+shadow+billboard,blocker-cards=record,speedtree-frame=program}, \
    select-lod{form=finest}";

fn fromsoft_run(world: &mut World) -> RunReport {
    let mut registry = PassRegistry::new();
    primase_fromsoft::sekiro::passes::register(&mut registry);
    registry.add(&primase_core::SELECT_LOD);
    let m = PassManager::new(registry).with_conservation(primase_core::ledger::Conservation::Enforce);
    let mut report = RunReport::default();
    let text = format!("{LEGACY_GEOMETRY}, resolve-fromsoft-shaders{{unbound-textures=preview}}");
    m.run(world, &m.registry.parse(&text).unwrap(), &mut report)
        .unwrap();
    report
}

#[allow(unsafe_code)]
fn oodle(path: std::path::PathBuf) -> primase_foundations::compress::oodle::Oodle {
    // SAFETY: the test configuration names a trusted native library.
    unsafe {
        primase_foundations::compress::oodle::Oodle::load(
            path,
            primase_foundations::compress::oodle::Version::V6,
        )
        .unwrap()
    }
}

#[test]
fn fromsoft_areas() {
    use inputs::{Game, Tool, install, keys, tool};
    use primase_fromsoft::materials::Material;
    use primase_fromsoft::{Game as FsGame, Install, Keys, dcx::Decoder, sekiro};
    let Some(dll) = tool(Tool::Oodle6) else {
        return;
    };
    let dll = oodle(dll);
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let finish = |mut world: World, game: &str, scene: &str| {
        let materials = world.iter::<Material>().count();
        let report = fromsoft_run(&mut world);
        let log = &report.steps.last().unwrap().log;
        print(&tally(&world, game, scene, materials, log));
    };
    if let Some(root) = install(Game::Sekiro) {
        let mut world = World::default();
        sekiro::read_area(
            &root,
            "m11_01",
            &decoder,
            &sekiro::Options {
                region: Some(sekiro::Region {
                    rect: [-170., 180., -50., 300.],
                    margin: 30.0,
                }),
                ..Default::default()
            },
            &mut world,
        )
        .unwrap();
        finish(world, "Sekiro", "m11_01 slice (-170,180)..(-50,300)");
    }
    if let (Some(root), Some(keys)) = (install(Game::Elden), keys(Game::Elden)) {
        let install = Install::new(
            root,
            FsGame::EldenRing,
            Keys::from_folder(keys, FsGame::EldenRing).unwrap(),
        );
        for (map, parents, label) in [
            (
                "m14_00_00_00",
                sekiro::Parents::None,
                "m14_00_00_00 (Academy of Raya Lucaria)",
            ),
            (
                "m60_35_46_00",
                sekiro::Parents::Include,
                "m60_35_46_00 (Liurnia tile, with parents)",
            ),
        ] {
            let mut world = World::default();
            let options = sekiro::Options {
                parents,
                ..Default::default()
            };
            sekiro::read_game_area(&install, map, &decoder, &options, &mut world).unwrap();
            finish(world, "Elden Ring", label);
        }
    }
}

// ---- STUNTBOOST: the level Bed, and the one effect that draws its materials ----

#[test]
fn stuntboost_bed() {
    use inputs::{Game, install};
    use monogame_formats::mgfx::Effect;
    use monogame_formats::xnb::Xnb;
    use primase_stuntboost::components::StuntboostMaterial;
    use primase_stuntboost::read::level::read_level;
    let Some(root) = install(Game::Stuntboost) else {
        return;
    };
    let effects = root.join("Content/Effects");
    let mut world = World::new();
    read_level(&root, "Bed", 1.0, &mut world).unwrap();
    let mut kinds: BTreeMap<String, u64> = BTreeMap::new();
    let mut fields: BTreeMap<String, u64> = BTreeMap::new();
    let mut materials = 0usize;
    for (_, m) in world.iter::<StuntboostMaterial>() {
        materials += 1;
        let kind = if m.name.starts_with("C_") {
            "collision (physics) material"
        } else if m.name.starts_with("bake_mat_") {
            "bake material"
        } else {
            "other material"
        };
        bump(&mut kinds, kind);
        if kind != "collision (physics) material" {
            bump(&mut fields, format!("alpha {}", m.alpha_mode));
            bump(&mut fields, format!("shading {:?}", m.shading));
            bump(
                &mut fields,
                if m.base_color_texture.is_some() {
                    "base colour texture"
                } else {
                    "no base colour texture"
                },
            );
            bump(
                &mut fields,
                if m.double_sided {
                    "double sided"
                } else {
                    "single sided"
                },
            );
        }
    }
    // the effects: the Render effect draws a level's materials
    let mut programs = BTreeMap::new();
    {
        let rel = "D3/Render.xnb";
        let bytes = std::fs::read(effects.join(rel)).unwrap();
        let xnb = Xnb::parse(&bytes).unwrap();
        let effect = Effect::parse(xnb.effect().unwrap()).unwrap();
        programs.insert(
            rel,
            json!({
                "shaders": effect.shaders.len(), "parameters": effect.parameters.len(),
                "techniques": effect.techniques.iter().map(|t| (t.name.clone(), t.passes.len())).collect::<Vec<_>>(),
                "samplers": effect.shaders.iter().map(|s| s.samplers.len()).sum::<usize>(),
                "constant_buffers": effect.constant_buffers.len(),
            }),
        );
    }
    print(
        &json!({"game": "STUNTBOOST", "scene": "Level Bed", "materials": materials, "binding_pass": false,
        "material_kinds": kinds, "material_fields": fields, "effects": programs,
        "note": "no resolve pass; every level material is drawn by D3/Render.fx"}),
    );
}
