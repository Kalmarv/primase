//! Elden Ring's light records as `core` light tables: the lights of Stormveil Castle's bank (`m10_00_00_00_0000`, the
//! area's bank zero), lowered by `lower-fromsoft-lights` at a `candela-per-power` of 1 (so a light's colour times its
//! intensity over the world's unit is its stored `(byte / 255)^2 * power`, its specular the same of its specular bytes
//! and power) and packed by `LightTable::pack` with each read-back dispatch's own view, against the records that dispatch
//! read (`oracles.scripts.elden/frames`, session `erlight2`: four `GXLightOrganize8` dispatches read back from the running
//! game before they ran, each with its `cbLightAccParam`, which holds the counts at +272 and +280 and the world-to-view
//! matrix's rows at +64, and its t19 and t20 buffers). The tables are the ones the area's render graph binds at that step.
//!
//! The game draws in a frame whose origin it moves (the dispatches' camera positions are all in it, so the frame states
//! no offset): the translation from it to the map's frame is the one, among those each pair of a recorded record and a
//! lowered light proposes, that puts the most records on a light, and it is whole metres. A recorded record stands on a
//! packed light where their view-space positions are within 0.01 m; the records of effect lights stand on none (effect
//! lights are not read here). The expected counts are an independent script's over the same dispatches and the bank's own
//! bytes (kept with the private oracle tools): the translation (-216, 40, 16); 16 point records and no spot record a
//! dispatch; 20 point records (five lights in four dispatches) stand on a bank light and 44 on none; on the 20, the
//! position, `1 / range^2`, the range, `LightType` 0, the colour and the specular colour are the packed ones, and no
//! packed light falls back to its colour for its specular. The shadowed variants' fields of an unshadowed record hold
//! what the buffer held before, so they are not compared.
//!
//! Needs `installs.elden`, `keys.elden`, `tools.oodle6` and `oracles.scripts.elden/frames`.
#![allow(clippy::unwrap_used)]
mod inputs;

use fromsoft_formats::{Binder, Game, Keys, dcx::Decoder};
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::render_graph::RenderGraph;
use primase_core::shader::{BufferSource, ConstantRole, FrameParameter, LightTableKind, ShaderBinding};
use primase_core::{Light, Transform, World};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro;
use std::collections::BTreeMap;
mod common;
use common::lights::{Tally, compare, floats, fnv1a64, near, organize_tables};

/// One read-back dispatch: the world-to-view matrix's three rows, the counted point and spot records as floats.
struct Recorded {
    view: [[f32; 4]; 3],
    points: Vec<Vec<f32>>,
    spots: Vec<Vec<f32>>,
}

/// The read-back dispatches of the program `organize` in a session's capture, in order.
fn recorded(session: &std::path::Path, organize: u64) -> Result<Vec<Recorded>, Box<dyn std::error::Error>> {
    let cap = session.join("cap");
    let mut out = Vec::new();
    for line in std::fs::read_to_string(cap.join("capture.jsonl"))?.lines() {
        if !line.contains("\"event\":\"readback\"") {
            continue;
        }
        let e: serde_json::Value = serde_json::from_str(line)?;
        if e["cs"].as_str().and_then(|h| u64::from_str_radix(h, 16).ok()) != Some(organize) {
            continue;
        }
        let mut paths = BTreeMap::new();
        for c in e["cbvs"].as_array().ok_or("no cbvs")? {
            if let (Some(path), Some(reg)) = (c["path"].as_str(), c["reg"].as_u64()) {
                let kind = if c["srv"].as_bool() == Some(true) {
                    't'
                } else {
                    'b'
                };
                paths.insert(format!("{kind}{reg}"), cap.join(path));
            }
        }
        let read = |slot: &str| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
            Ok(std::fs::read(
                paths.get(slot).ok_or_else(|| format!("no {slot}"))?,
            )?)
        };
        let cb = floats(&read("b0")?);
        let word = |at: usize| cb[at / 4].to_bits() as usize;
        let view = [0, 1, 2].map(|r| [0, 1, 2, 3].map(|c| cb[16 + 4 * r + c]));
        let records = |bytes: Vec<u8>, stride: usize, count: usize| -> Vec<Vec<f32>> {
            (0..count)
                .map(|i| floats(&bytes[stride * i..stride * (i + 1)]))
                .collect()
        };
        out.push(Recorded {
            view,
            points: records(read("t19")?, 208, word(272)),
            spots: records(read("t20")?, 192, word(280)),
        });
    }
    Ok(out)
}

/// A view-space position taken back to the drawing frame: `R^T (p - t)` for the view's rotation rows `R` and
/// translation `t`.
fn drawing_frame(view: &[[f32; 4]; 3], p: &[f32]) -> [f64; 3] {
    let d: Vec<f64> = (0..3).map(|k| f64::from(p[k]) - f64::from(view[k][3])).collect();
    [0, 1, 2].map(|j| (0..3).map(|k| f64::from(view[k][j]) * d[k]).sum())
}

#[test]
fn eldens_light_tables_pack_the_read_back_records() -> Result<(), Box<dyn std::error::Error>> {
    let (Some(root), Some(keys), Some(dll), Some(frames)) = (
        inputs::install(inputs::Game::Elden),
        inputs::keys(inputs::Game::Elden),
        inputs::oodle(inputs::Tool::Oodle6),
        inputs::oracle(inputs::OracleSet::Scripts, inputs::Game::Elden, "frames"),
    ) else {
        return Ok(());
    };
    let install = common::install(
        root,
        Game::EldenRing,
        Keys::from_folder(keys, Game::EldenRing, Game::EldenRing.param_key_sha256())?,
    );
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let mut world = World::default();
    sekiro::read_game_area(
        &install,
        "m10_00_00_00",
        &decoder,
        &sekiro::Options {
            render_graph: true,
            ..Default::default()
        },
        &mut world,
    )?;
    let mut registry = PassRegistry::new();
    primase_fromsoft::sekiro::passes::register(&mut registry);
    let manager = PassManager::new(registry);
    let mut run = RunReport::default();
    manager.run(
        &mut world,
        &manager
            .registry
            .parse("lower-fromsoft-lights{candela-per-power=1,spot-blend=0.15}, resolve-fromsoft-shaders{unbound-textures=preview}")?,
        &mut run,
    )?;
    let graph = world.resource::<RenderGraph>().ok_or("no core.render_graph")?;
    let mut tables = BTreeMap::new();
    let mut constants = Vec::new();
    organize_tables(
        &graph.passes,
        &world,
        "GXLightOrganize8.cpo",
        &mut tables,
        &mut constants,
    );
    let (Some(points), Some(spots)) = (tables.get("point"), tables.get("spot")) else {
        return Err("the organize step binds no point and spot light tables".into());
    };
    // the organize step's counts, from its own reflection, where the dispatches hold them in cbLightAccParam
    let role_at = |offset: u32| {
        constants
            .iter()
            .find(|c| c.offset == offset)
            .map(|c| (c.name.clone(), c.role.clone()))
    };
    assert_eq!(
        role_at(272),
        Some((
            Some("g_lightCount".into()),
            ConstantRole::Frame(FrameParameter::LightCount(LightTableKind::Point))
        ))
    );
    assert_eq!(
        role_at(280),
        Some((
            Some("g_spotLightCount".into()),
            ConstantRole::Frame(FrameParameter::LightCount(LightTableKind::Spot))
        ))
    );
    // the materials whose programs read the scene's lights, as their bindings state them
    let lit = world
        .iter::<ShaderBinding>()
        .flat_map(|(_, b)| b.stages.iter())
        .filter(|s| {
            s.buffers
                .iter()
                .any(|b| matches!(b.source, BufferSource::Lights(_)))
        })
        .count();
    println!("material stages with light tables: {lit}");
    // the organize program's hash, from the install's binder
    let archive = install.open()?;
    let binder = Binder::read(
        &archive.read("/shader/gxrenderershader.shaderbnd.dcx", &decoder)?,
        &decoder,
    )?;
    let organize = binder
        .files
        .iter()
        .find(|f| {
            f.name
                .as_deref()
                .is_some_and(|n| n.ends_with("GXLightOrganize8.cpo"))
        })
        .map(|f| fnv1a64(&f.bytes))
        .ok_or("no GXLightOrganize8.cpo")?;
    let dispatches = recorded(&frames.join("erlight2"), organize)?;
    assert_eq!(dispatches.len(), 4, "the session's read-back dispatches");
    // the bank's lights in the map's frame (the world frame mirrors the map's z)
    let lights: Vec<[f64; 3]> = world
        .iter::<Light>()
        .filter_map(|(e, _)| world.get::<Transform>(e))
        .map(|t| {
            let p = t.cols[3].map(f64::from);
            [p[0], p[1], -p[2]]
        })
        .collect();
    let in_frame: Vec<[f64; 3]> = dispatches
        .iter()
        .flat_map(|d| d.points.iter().map(|r| drawing_frame(&d.view, r)))
        .collect();
    let on_light = |t: [f64; 3]| {
        in_frame
            .iter()
            .filter(|r| {
                lights
                    .iter()
                    .any(|l| (0..3).map(|k| (l[k] - r[k] - t[k]).powi(2)).sum::<f64>().sqrt() <= 0.01)
            })
            .count()
    };
    let mut best = ([0.0; 3], 0);
    for r in &in_frame {
        for l in &lights {
            let t = [0, 1, 2].map(|k| l[k] - r[k]);
            let n = on_light(t);
            if n > best.1 {
                best = (t, n);
            }
        }
    }
    let translation = best.0.map(|x| (x * 1000.0).round() / 1000.0);
    println!(
        "drawing frame to map: {translation:?}, {} records on a light",
        best.1
    );
    assert_eq!(translation, [-216.0, 40.0, 16.0]);
    let mut tally = Tally::default();
    let mut on_none = [0usize; 2];
    let mut specular_from_colour = 0usize;
    for d in &dispatches {
        assert_eq!((d.points.len(), d.spots.len()), (16, 0));
        // the world-to-view matrix of the world frame: the map's z mirrored, the drawing frame's origin moved back
        let mut view = [[0.0, 0.0, 0.0, 1.0]; 4];
        for (row, v) in view.iter_mut().zip(d.view) {
            let moved = (0..3).map(|k| f64::from(v[k]) * translation[k]).sum::<f64>();
            *row = [v[0], v[1], -v[2], (f64::from(v[3]) - moved) as f32];
        }
        for (k, (table, records)) in [(points, &d.points), (spots, &d.spots)].into_iter().enumerate() {
            let kind = if k == 0 { "point" } else { "spot" };
            let packed = table.pack(&world, &view)?;
            specular_from_colour += packed.specular_from_colour;
            let stride = table.stride as usize;
            let packed: Vec<Vec<f32>> = (0..packed.lights.len())
                .map(|i| floats(&packed.bytes[stride * i..stride * (i + 1)]))
                .collect();
            for r in records {
                match packed.iter().find(|p| (0..3).all(|c| near(p[c], r[c], 0.01))) {
                    Some(p) => compare(kind, table, p, r, &mut tally),
                    None => on_none[k] += 1,
                }
            }
        }
    }
    for (key, [same, differ]) in &tally.0 {
        println!("{key}: {same} match, {differ} differ");
    }
    println!(
        "records on no packed light: {} point, {} spot",
        on_none[0], on_none[1]
    );
    assert_eq!(on_none, [44, 0]);
    println!("packed lights whose specular is their colour: {specular_from_colour}");
    assert_eq!(specular_from_colour, 0);
    for key in [
        "point view at 0",
        "point range at 12",
        "point range at 60",
        "point constant at 64",
        "point colour exact at 16",
        "point colour exact at 32",
    ] {
        assert_eq!(tally.get(key), [20, 0], "{key}");
    }
    Ok(())
}
