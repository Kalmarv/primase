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
//! 1 (three distant lights the engine fades, in two frames each); the specular colour of all 55 and of the spot is the
//! packed one, the light's own specular (`(specular byte / 255)^2 * specular power`, which the engine does not fade),
//! and no packed light falls back to its colour there. The spot's cone vectors 0, 1 and 3, its bounding sphere and its
//! depth range match; its vector 2 does not, as the light has a near plane of 15 and a `core.light` none. The fields
//! only the shadowed program variants read are 0 in every recorded record: the frames hold no shadowed light, so those
//! stay unknown.
//!
//! The organize step's own constants carry the counts and the view where the frames hold them: `g_lightCount` (+272) and
//! `g_spotLightCount` (+280) as the light counts, `g_ViewMatrix` (+64) as the view, its lines unshown as not read
//! (fxc's reflection marks it unused in this program: the records hold view-space positions already).
//!
//! Needs `installs.sekiro`, `tools.oodle6` and `oracles.scripts.sekiro/frames`.
#![allow(clippy::unwrap_used)]
mod inputs;

use fromsoft_formats::{Binder, dcx::Decoder};
use inputs::{Game, OracleSet, Tool, install, oodle, oracle};
use iridia_interface::{MatrixLines, Unshown};
use primase_core::World;
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::render_graph::RenderGraph;
use primase_core::shader::{
    BufferSource, ConstantRole, FrameParameter, LightIndices, LightListPart, LightLookup, LightTableKind,
    ListBits, ShaderBinding, ViewParameter, pack_light_lists,
};
use primase_fromsoft::sekiro;
use std::collections::BTreeMap;
use std::io::BufRead;
mod common;
use common::lights::{Tally, compare, floats, fnv1a64, near, organize_tables};

/// One frame's organize dispatch as the capture recorded it: its world-to-view matrix (three rows of four), its point
/// and spot records as floats, and the lists it wrote where the capture read them back after it.
struct Recorded {
    view: [[f32; 4]; 3],
    points: Vec<Vec<f32>>,
    spots: Vec<Vec<f32>>,
    lists: Option<RecordedLists>,
}

/// The organize dispatch's outputs as read back after it: the index list (u0), the lookup (u1) and the 4-byte count of
/// entries it handed out (u2, which the frame zeroes before the dispatch).
struct RecordedLists {
    indices: Vec<u8>,
    lookup: Vec<u8>,
    entries: u32,
}

/// The value at `bits` of a record.
fn value(record: &[u8], bits: ListBits) -> u32 {
    let at = bits.offset as usize;
    let word = u32::from_le_bytes([record[at], record[at + 1], record[at + 2], record[at + 3]]);
    (word >> bits.shift) & bits.max()
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
    let mut lines = std::io::BufReader::new(file).lines();
    while let Some(line) = lines.next() {
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
        // the outputs read back right after the dispatch, by resource
        let uav = |slot: u64| -> Option<String> {
            cs["uav"]
                .as_array()?
                .iter()
                .find(|u| u["slot"].as_u64() == Some(slot))?["res"]
                .as_str()
                .map(str::to_owned)
        };
        let wanted: Vec<Option<String>> = (0..3).map(uav).collect();
        let mut read: [Option<Vec<u8>>; 3] = [None, None, None];
        for line in lines.by_ref().take(16) {
            let Ok(r) = serde_json::from_str::<serde_json::Value>(&line?) else {
                continue;
            };
            if r["ev"] != "readback" {
                continue;
            }
            for (k, want) in wanted.iter().enumerate() {
                if want.is_some() && r["src"].as_str() == want.as_deref() {
                    // a readback the capture logged but did not keep (sek1's frames keep none) leaves the lists unread
                    let file = r["file"].as_str().ok_or("a readback with no file")?;
                    read[k] = std::fs::read(cap.join(n).join(file)).ok();
                }
            }
        }
        let lists = match read {
            [Some(indices), Some(lookup), Some(count)] if count.len() == 4 => Some(RecordedLists {
                indices,
                lookup,
                entries: u32::from_le_bytes([count[0], count[1], count[2], count[3]]),
            }),
            _ => None,
        };
        return Ok(Recorded {
            view,
            points: (0..points).map(|i| floats(&pb[208 * i..208 * (i + 1)])).collect(),
            spots: (0..spots).map(|i| floats(&sb[192 * i..192 * (i + 1)])).collect(),
            lists,
        });
    }
    Err(format!("{frame}: no GXLightOrganize16 dispatch").into())
}

/// The recorded lists of `frame`, read with the forward materials' parts: every tile's entries lie inside the index list
/// and name a record of their kind ([`Recorded`]'s counts), the tiles' counts sum to the entries the organize program
/// handed out, every entry's slices and flag are printed, and how many of the frame's lights the game's lists leave out
/// of a tile on average (ours list every light in every tile).
fn check_lists(
    frame: &str,
    lookup: &LightLookup,
    indices: &LightIndices,
    lists: &RecordedLists,
    records: [usize; 2],
) {
    let tiles: Vec<&[u8]> = lists.lookup.chunks_exact(lookup.stride as usize).collect();
    let entries: Vec<&[u8]> = lists.indices.chunks_exact(indices.stride as usize).collect();
    let (mut listed, mut flagged, mut all_slices) = (0u64, 0u64, 0u64);
    let mut culled = 0u64;
    for tile in &tiles {
        let mut at = value(tile, lookup.first) as usize;
        let mut in_tile = 0u64;
        for (kind, bits) in &lookup.counts {
            let k = usize::from(*kind == LightTableKind::Spot);
            for _ in 0..value(tile, *bits) {
                let entry = entries[at];
                let index = value(entry, indices.index) as usize;
                assert!(
                    index < records[k],
                    "{frame}: a {kind:?} entry names record {index} of {}",
                    records[k]
                );
                flagged += u64::from(indices.constants.iter().all(|c| value(entry, c.bits) == c.value));
                all_slices += u64::from(value(entry, indices.slices) == indices.slices.max());
                at += 1;
                in_tile += 1;
            }
        }
        listed += in_tile;
        culled += (records[0] + records[1]) as u64 - in_tile;
    }
    println!(
        "{frame}: {} tiles list {listed} entries (the organize count {}), {flagged} with bit 31 (the tile-lighting \
         pass visits them), {all_slices} in every slice; the game leaves out {:.2} of {} lights per tile on average",
        tiles.len(),
        lists.entries,
        culled as f64 / tiles.len() as f64,
        records[0] + records[1]
    );
    assert_eq!(
        listed,
        u64::from(lists.entries),
        "{frame}: the tiles' counts against the organize count"
    );
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
            .parse("lower-fromsoft-render-graph, lower-fromsoft-lights{candela-per-power=1,spot-blend=0.15}, resolve-fromsoft-shaders{unbound-textures=preview}")?,
        &mut run,
    )?;
    let graph = world.resource::<RenderGraph>().ok_or("no core.render_graph")?;
    let mut tables = BTreeMap::new();
    let mut constants = Vec::new();
    organize_tables(
        &graph.passes,
        &world,
        "GXLightOrganize16.cpo",
        &mut tables,
        &mut constants,
    );
    let (Some(points), Some(spots)) = (tables.get("point"), tables.get("spot")) else {
        return Err("the organize step binds no point and spot light tables".into());
    };
    // the organize step's counts and view, from its own reflection, where the frames hold them in cbLightAccParam
    // (the counts at +272 and +280, the world-to-view matrix's three rows at +64)
    let role_at = |offset: u32| {
        constants
            .iter()
            .find(|c| c.offset == offset)
            .map(|c| (c.name.clone(), c.role.clone(), c.lines.clone()))
    };
    assert_eq!(
        role_at(272),
        Some((
            Some("g_lightCount".into()),
            ConstantRole::Frame(FrameParameter::LightCount(LightTableKind::Point)),
            None
        ))
    );
    assert_eq!(
        role_at(280),
        Some((
            Some("g_spotLightCount".into()),
            ConstantRole::Frame(FrameParameter::LightCount(LightTableKind::Spot)),
            None
        ))
    );
    assert_eq!(
        role_at(64),
        Some((
            Some("g_ViewMatrix".into()),
            ConstantRole::View(ViewParameter::View),
            Some(MatrixLines::Unshown(Unshown::NotRead))
        ))
    );
    // the forward materials' light lists, as their bindings state them
    let (mut lookup, mut index_list, mut lit) = (None::<LightLookup>, None::<LightIndices>, 0usize);
    for (_, binding) in world.iter::<ShaderBinding>() {
        for stage in &binding.stages {
            let mut any = false;
            for b in &stage.buffers {
                match &b.source {
                    BufferSource::LightLists(LightListPart::Lookup(l)) => {
                        lookup.get_or_insert_with(|| l.clone());
                        any = true;
                    }
                    BufferSource::LightLists(LightListPart::Indices(x)) => {
                        index_list.get_or_insert_with(|| x.clone());
                        any = true;
                    }
                    _ => {}
                }
            }
            lit += usize::from(any);
        }
    }
    println!("material stages with light lists: {lit}");
    let (Some(lookup), Some(index_list)) = (lookup, index_list) else {
        return Err("no material binding states light lists".into());
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
    let mut list_frames = 0usize;
    let mut specular_from_colour = 0usize;
    for frame in &table_frames {
        let rec = recorded(&frames, frame, organize)?;
        if let Some(lists) = &rec.lists {
            list_frames += 1;
            check_lists(
                frame,
                &lookup,
                &index_list,
                lists,
                [rec.points.len(), rec.spots.len()],
            );
        }
        // the game's view takes the map frame to view space; the world frame mirrors the map's z
        let mut view = [[0.0, 0.0, 0.0, 1.0]; 4];
        for (row, v) in view.iter_mut().zip(rec.view) {
            *row = [v[0], v[1], -v[2], v[3]];
        }
        let mut packed_counts = [0u32; 2];
        for (k, (table, records)) in [(points, &rec.points), (spots, &rec.spots)]
            .into_iter()
            .enumerate()
        {
            let kind = if k == 0 { "point" } else { "spot" };
            let packed = table.pack(&world, &view)?;
            packed_counts[k] = packed.lights.len() as u32;
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
        // our lists for the frame: every packed light in every one of the frame's tiles
        if let Some(lists) = &rec.lists {
            let tiles = (lists.lookup.len() / lookup.stride as usize) as u32;
            let ours = pack_light_lists(
                &lookup,
                &index_list,
                &[
                    (LightTableKind::Point, packed_counts[0]),
                    (LightTableKind::Spot, packed_counts[1]),
                ],
                tiles,
            )?;
            assert_eq!(ours.lookup.len(), lists.lookup.len(), "{frame}");
        }
    }
    println!("frames with recorded light lists: {list_frames}");
    assert!(list_frames > 0, "no frame holds the organize step's lists");
    for (key, [same, differ]) in &tally.0 {
        println!("{key}: {same} match, {differ} differ");
    }
    println!(
        "records on no packed light: {} point, {} spot",
        on_none[0], on_none[1]
    );
    assert_eq!(on_none, [105, 0]);
    // every bank light states its own specular: none packs its colour there
    println!("packed lights whose specular is their colour: {specular_from_colour}");
    assert_eq!(specular_from_colour, 0);
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
    assert_eq!(tally.get("point colour exact at 32"), [55, 0]);
    for key in [
        "point ProjParam zero",
        "point ShadowCenterPos zero",
        "point ViewportScaleOffset zero",
    ] {
        assert_eq!(tally.get(key), [55, 0], "{key}");
    }
    // the spot: its cone but for the near plane, its sphere and depths, its colours
    for key in [
        "spot view at 0",
        "spot range at 12",
        "spot colour exact at 16",
        "spot colour exact at 32",
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
    Ok(())
}
