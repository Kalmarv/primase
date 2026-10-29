//! Sekiro's light records as `core` light tables: the lights of Ashina Castle's bank (`m11_01_00_00_0000`, the area's
//! bank zero), lowered by `lower-fromsoft-lights` at a `candela-per-power` of 1 (so a light's colour times its intensity
//! over the world's unit is its stored `(byte / 255)^2 * power`) and packed by `LightTable::pack` with each recorded
//! frame's own view, against the records that frame uploaded (`oracles.scripts.sekiro/frames`: each `GXLightOrganize16`
//! dispatch's `cbLightAccParam`, which holds the counts at +272 and +280 and the world-to-view matrix at +64, and its t19
//! and t20 buffers, all from the capture's blobs). The tables are the ones the frame's render graph binds at that step.
//!
//! A recorded record stands on a packed light where their view-space positions are within 0.01 m; the records of effect
//! lights stand on none (effect lights are not read here). Each field of the table is compared where it stands on one.
//! The expected counts are an independent script's over the same frames and bank (kept with the private oracle tools):
//! 55 point records and 1 spot record stand on a bank light, 105 point records on none; their positions, ranges and
//! types all match; the colour of 49 point records is the packed one and of 6 the packed one scaled by one factor below
//! 1 (three distant lights the engine fades, in two frames each), and 11 point records carry a specular colour equal to
//! the colour (the others' banks store a separate specular colour, which the lowering does not lower). The spot's cone
//! vectors 0, 1 and 3, its bounding sphere and its depth range match; its vector 2 does not, as the light has a near
//! plane of 15 and a `core.light` none; its specular is its bank's own. The fields only the shadowed program variants
//! read are 0 in every recorded record: the frames hold no shadowed light, so those stay unknown.
//!
//! Needs `installs.sekiro`, `tools.oodle6` and `oracles.scripts.sekiro/frames`.
#![allow(clippy::unwrap_used)]
mod inputs;

use fromsoft_formats::{Binder, dcx::Decoder};
use inputs::{Game, OracleSet, Tool, install, oodle, oracle};
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::render_graph::{PassWork, RenderGraph, RenderPass};
use primase_core::shader::{BufferSource, LightTable, LightTableKind, LightValue};
use primase_core::{Name, World};
use primase_fromsoft::sekiro;
use std::collections::BTreeMap;
use std::io::BufRead;
mod common;

/// The capture proxies' hash of a program's bytes: FNV-1a 64 from the seed 1469598103934665603.
fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(1_469_598_103_934_665_603_u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn floats(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

/// One frame's organize dispatch as the capture recorded it: its world-to-view matrix (three rows of four) and its
/// point and spot records, as floats.
struct Recorded {
    view: [[f32; 4]; 3],
    points: Vec<Vec<f32>>,
    spots: Vec<Vec<f32>>,
}

/// The records of frame `frame` (`<run> frame_<n>`): the dispatch of the program hashed `organize`, its `cb0` blob's
/// counts and matrix, its t19 and t20 blobs.
fn recorded(
    root: &std::path::Path,
    frame: &str,
    organize: u64,
) -> Result<Recorded, Box<dyn std::error::Error>> {
    let (run, n) = frame.split_once(' ').ok_or("a frame name with no run")?;
    let cap = root.join(run).join("cap");
    let file = std::fs::File::open(cap.join(n).join("events.jsonl"))?;
    for line in std::io::BufReader::new(file).lines() {
        let Ok(e) = serde_json::from_str::<serde_json::Value>(&line?) else {
            continue;
        };
        let cs = &e["cs"];
        if cs["hash"].as_str().and_then(|h| u64::from_str_radix(h, 16).ok()) != Some(organize) {
            continue;
        }
        let blob = |h: &serde_json::Value| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
            let h = h.as_str().ok_or("a binding with no blob")?;
            Ok(std::fs::read(cap.join("blobs").join(format!("{h}.bin")))?)
        };
        let cb0 = cs["cb"]
            .as_array()
            .and_then(|cbs| cbs.iter().find(|c| c[0].as_u64() == Some(0)))
            .ok_or("no cb0")?;
        let cb = blob(&cb0[4])?;
        let word = |at: usize| u32::from_le_bytes([cb[at], cb[at + 1], cb[at + 2], cb[at + 3]]) as usize;
        let (points, spots) = (word(272), word(280));
        let m = floats(&cb[64..112]);
        let view = [0, 1, 2].map(|r| [m[4 * r], m[4 * r + 1], m[4 * r + 2], m[4 * r + 3]]);
        let srv = |slot: u64| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
            let s = cs["srv"]
                .as_array()
                .and_then(|srvs| srvs.iter().find(|s| s["slot"].as_u64() == Some(slot)))
                .ok_or("no such slot")?;
            blob(&s["blob"])
        };
        let (pb, sb) = (srv(19)?, srv(20)?);
        return Ok(Recorded {
            view,
            points: (0..points).map(|i| floats(&pb[208 * i..208 * (i + 1)])).collect(),
            spots: (0..spots).map(|i| floats(&sb[192 * i..192 * (i + 1)])).collect(),
        });
    }
    Err(format!("{frame}: no GXLightOrganize16 dispatch").into())
}

/// The light tables the graph's `GXLightOrganize16` step binds, by kind.
fn organize_tables(passes: &[RenderPass], world: &World, out: &mut BTreeMap<&'static str, LightTable>) {
    for p in passes {
        match &p.work {
            PassWork::Compute { stage, .. }
                if world
                    .get::<Name>(stage.shader)
                    .is_some_and(|n| n.0.ends_with("GXLightOrganize16.cpo")) =>
            {
                for b in &stage.buffers {
                    if let BufferSource::Lights(t) = &b.source {
                        let kind = match t.kind {
                            LightTableKind::Point => "point",
                            LightTableKind::Spot => "spot",
                        };
                        out.insert(kind, t.clone());
                    }
                }
            }
            PassWork::Group { steps } => organize_tables(steps, world, out),
            _ => {}
        }
    }
}

/// How a packed field compares with the recorded one: per `<kind> <field>`, the matched and the differing records.
#[derive(Default)]
struct Tally(BTreeMap<String, [usize; 2]>);
impl Tally {
    fn add(&mut self, key: String, same: bool) {
        self.0.entry(key).or_default()[usize::from(!same)] += 1;
    }
    fn get(&self, key: &str) -> [usize; 2] {
        self.0.get(key).copied().unwrap_or_default()
    }
}

fn near(a: f32, b: f32, abs: f64) -> bool {
    (f64::from(a) - f64::from(b)).abs() <= abs
}

fn relative(a: f32, b: f32, rel: f64) -> bool {
    (f64::from(a) - f64::from(b)).abs() <= rel * f64::from(b).abs().max(1e-9)
}

/// Each field of `table` in a packed record against the recorded one it stands on.
fn compare(kind: &str, table: &LightTable, packed: &[f32], recorded: &[f32], tally: &mut Tally) {
    for f in &table.fields {
        let at = f.offset as usize / 4;
        let n = f.value.floats() as usize;
        let (p, r) = (&packed[at..at + n], &recorded[at..at + n]);
        let name = |what: &str| format!("{kind} {what} at {}", f.offset);
        match &f.value {
            LightValue::ViewPosition | LightValue::ViewDepthMin | LightValue::ViewDepthMax => {
                tally.add(name("view"), p.iter().zip(r).all(|(a, b)| near(*a, *b, 1e-3)));
            }
            LightValue::Range | LightValue::InverseRangeSquared => {
                tally.add(name("range"), relative(p[0], r[0], 1e-5));
            }
            LightValue::Constant { .. } => tally.add(name("constant"), p == r),
            LightValue::Colour => {
                let exact = p.iter().zip(r).all(|(a, b)| relative(*b, *a, 1e-5));
                // the recorded colour the packed one times one factor below 1, the same in every channel
                let factors: Vec<f64> = p
                    .iter()
                    .zip(r)
                    .filter(|(a, _)| **a > 0.0)
                    .map(|(a, b)| f64::from(*b) / f64::from(*a))
                    .collect();
                let scaled = !exact
                    && !factors.is_empty()
                    && factors
                        .iter()
                        .all(|k| *k < 1.0 && (k - factors[0]).abs() <= 1e-5 * factors[0]);
                tally.add(name("colour exact"), exact);
                tally.add(name("colour scaled"), scaled);
            }
            LightValue::ViewToCone => {
                for v in 0..4 {
                    let (pv, rv) = (&p[4 * v..4 * v + 4], &r[4 * v..4 * v + 4]);
                    let scale = rv.iter().fold(0f32, |m, x| m.max(x.abs())) as f64;
                    tally.add(
                        name(&format!("cone vector {v}")),
                        pv.iter()
                            .zip(rv)
                            .all(|(a, b)| near(*a, *b, 1e-4 * scale.max(1.0))),
                    );
                }
            }
            LightValue::ViewBoundingSphere => {
                tally.add(name("sphere"), p.iter().zip(r).all(|(a, b)| near(*a, *b, 1e-3)));
            }
            // the shadowed variants' fields: the frames hold no shadowed light, every one is 0
            LightValue::Unknown { name: field, why, .. } if why.contains("Sdw") => {
                tally.add(format!("{kind} {field} zero"), r.iter().all(|x| *x == 0.0));
            }
            _ => {}
        }
    }
}

#[test]
fn sekiros_light_tables_pack_the_recorded_records() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = install(Game::Sekiro) else {
        return Ok(());
    };
    let Some(frames) = oracle(OracleSet::Scripts, Game::Sekiro, "frames") else {
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
    let mut world = World::default();
    // the bank's lights lie in x -353 .. -82 and z 204 .. 269 of the map's frame
    sekiro::read_game_area(
        &install,
        "m11_01",
        &decoder,
        &sekiro::Options {
            region: Some(sekiro::Region {
                rect: [-360.0, 200.0, -80.0, 275.0],
                margin: 0.0,
            }),
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
            .parse("lower-fromsoft-lights{candela-per-power=1,spot-blend=0.15}")?,
        &mut run,
    )?;
    let graph = world.resource::<RenderGraph>().ok_or("no core.render_graph")?;
    let mut tables = BTreeMap::new();
    organize_tables(&graph.passes, &world, &mut tables);
    let (Some(points), Some(spots)) = (tables.get("point"), tables.get("spot")) else {
        return Err("the organize step binds no point and spot light tables".into());
    };
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
                .is_some_and(|n| n.ends_with("GXLightOrganize16.cpo"))
        })
        .map(|f| fnv1a64(&f.bytes))
        .ok_or("no GXLightOrganize16.cpo")?;
    let table_frames: Vec<String> = {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../src/sekiro/read/render_graph/sekiro.json"))?;
        v["frames"]
            .as_array()
            .ok_or("no frames")?
            .iter()
            .filter_map(|f| f.as_str().map(str::to_owned))
            .collect()
    };
    let mut tally = Tally::default();
    let mut on_none = [0usize; 2];
    for frame in &table_frames {
        let rec = recorded(&frames, frame, organize)?;
        // the game's view takes the map frame to view space; the world frame mirrors the map's z
        let mut view = [[0.0, 0.0, 0.0, 1.0]; 4];
        for (row, v) in view.iter_mut().zip(rec.view) {
            *row = [v[0], v[1], -v[2], v[3]];
        }
        for (k, (table, records)) in [(points, &rec.points), (spots, &rec.spots)]
            .into_iter()
            .enumerate()
        {
            let kind = if k == 0 { "point" } else { "spot" };
            let packed = table.pack(&world, &view)?;
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
    assert_eq!(on_none, [105, 0]);
    // every point record on a bank light: its position, 1 / range^2, range and type are the packed ones
    for key in [
        "point view at 0",
        "point range at 12",
        "point range at 60",
        "point constant at 64",
    ] {
        assert_eq!(tally.get(key), [55, 0], "{key}");
    }
    assert_eq!(tally.get("point colour exact at 16"), [49, 6]);
    assert_eq!(tally.get("point colour scaled at 16"), [6, 49]);
    assert_eq!(tally.get("point colour exact at 32"), [11, 44]);
    for key in [
        "point ProjParam zero",
        "point ShadowCenterPos zero",
        "point ViewportScaleOffset zero",
    ] {
        assert_eq!(tally.get(key), [55, 0], "{key}");
    }
    // the spot: its cone but for the near plane, its sphere and depths; its specular colour is its bank's own
    for key in [
        "spot view at 0",
        "spot range at 12",
        "spot colour exact at 16",
        "spot cone vector 0 at 64",
        "spot cone vector 1 at 64",
        "spot cone vector 3 at 64",
        "spot constant at 128",
        "spot view at 132",
        "spot view at 136",
        "spot sphere at 144",
        "spot ShadowParam.x zero",
        "spot ViewportScaleOffset zero",
    ] {
        assert_eq!(tally.get(key), [1, 0], "{key}");
    }
    assert_eq!(tally.get("spot cone vector 2 at 64"), [0, 1]);
    assert_eq!(tally.get("spot colour exact at 32"), [0, 1]);
    Ok(())
}
