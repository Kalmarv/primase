//! Elden Ring's light records as `core` light tables: the lights of Stormveil Castle's bank (`m10_00_00_00_0000`, the
//! area's bank zero), lowered by `lower-fromsoft-lights` at a `candela-per-power` of 1 (so a light's colour times its
//! intensity over the world's unit is its stored `(byte / 255)^2 * power`, its specular the same of its specular bytes
//! and power) and packed by `LightTable::pack` with each read-back dispatch's own view, against the records that dispatch
//! read (`oracles.scripts.elden/frames`, session `erlight2`: four `GXLightOrganize8` dispatches read back from the running
//! game before they ran at the Stormveil Castle gate, each with its `cbLightAccParam`, which holds the counts at +272 and
//! +280 and the world-to-view matrix's rows at +64, and its t19 and t20 buffers). The tables are the ones the area's render
//! graph binds at that step.
//!
//! The game draws in a frame whose origin it moves (the dispatches' camera positions are all in it, so the frame states
//! no offset): the translation from it to the map's frame is the one, among those each pair of a recorded record and a
//! lowered light proposes, that puts the most records on a light, and it is whole metres. That translation is the render
//! origin of the session the capture was taken in, not a constant of the game: sessions of the stock game show
//! (-88, 16, 16), as this capture's does, and a shifted origin, (-216, 40, 16), appears in sessions with a character or
//! map mod loaded. The capture is frozen, so its translation is a fixed fact of it, and the test expects it exactly.
//! A recorded record stands on a packed light where their view-space positions are within 0.01 m; the records of effect
//! lights stand on none (effect lights are not read here). The expected counts are an independent script's over the same
//! dispatches and the bank's own bytes (kept with the private oracle tools): the translation (-88, 16, 16); 22 point
//! records and no spot record a dispatch; 8 point records (bank lights 32 and 116 in four dispatches) stand on a bank
//! light and 80 on none; on the 8, the position, `1 / range^2`, the range, the colour and the specular colour are the
//! packed ones, and no packed light falls back to its colour for its specular.
//!
//! `LightType`: the game draws bank light 32, the one shadow-casting light among the records (its `cast_shadows` byte
//! set), with a local shadow, and its four records hold 1.0 there and its shadow's place after it; light 116's hold 0.
//! The consumer here draws no local shadow, so every light packs unshadowed: light 32's record holds 0 and zeros, and
//! the packing names it, with the bank's eight other shadow-casting point lights, as a shadow-casting light packed
//! unshadowed. Packed with its shadow (the place its own records state), its record is the recorded one word for word
//! from `LightType` on, and the light lists leave its entry out of the tile pass (bit 31 clear) and out of the lookup's
//! count copy. The shadow fields of an unshadowed record hold what the buffer held before, so they are not compared.
//! Placed by the render graph's own local shadow atlas (`core.render_graph`'s layout, given the light's centre in the
//! drawing frame and its range), its record is the recorded one from `LightType` on as well, and so is the shadowed
//! pass's `cbLightAccLightParam` at frame 6990 of session `er3`; that pass's `cbLightAccShadowInfo` is the atlas's size
//! and texel step, and the atlas's first face's camera is the one the frame drew that face's casters with (its view word
//! for word, its projection to 1e-6).
//!
//! Needs `installs.elden`, `keys.elden`, `tools.oodle6` and `oracles.scripts.elden/frames`.
#![allow(clippy::unwrap_used)]
mod inputs;

use fromsoft_formats::{Binder, Game, Keys, dcx::Decoder};
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::render_graph::RenderGraph;
use primase_core::shader::{
    BufferSource, ConstantRole, FrameParameter, LightListPart, LightTableKind, LocalShadow, LocalShadows,
    ShaderBinding, ShadowedLight, pack_light_lists,
};
use primase_core::{Light, Transform, World};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro;
use std::collections::BTreeMap;
mod common;
use common::lights::{Tally, compare, floats, fnv1a64, near, organize_tables, shadowed};

/// One read-back dispatch: the world-to-view matrix's three rows, the counted point and spot records as floats.
struct Recorded {
    id: serde_json::Value,
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
            id: e["id"].clone(),
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
    // Every bank-zero value survives the reader, including lights outside the frozen camera.
    let source_lights: Vec<_> = world
        .iter::<sekiro::source::LightSource>()
        .map(|(_, s)| s)
        .collect();
    assert_eq!(source_lights.len(), 216);
    assert!(
        source_lights
            .iter()
            .all(|s| s.sharpness.is_finite() && s.sharpness > 0.)
    );
    assert_eq!(
        source_lights
            .iter()
            .map(|s| s.sharpness)
            .fold(f32::INFINITY, f32::min),
        0.2
    );
    assert_eq!(source_lights.iter().map(|s| s.sharpness).fold(0., f32::max), 13.);
    let mut registry = PassRegistry::new();
    primase_fromsoft::sekiro::passes::register(&mut registry);
    let manager = PassManager::new(registry);
    let mut run = RunReport::default();
    manager.run(
        &mut world,
        &manager
            .registry
            .parse("lower-fromsoft-render-graph, lower-fromsoft-lights{candela-per-power=1,spot-blend=0.15,brightness=settled,min-range=0,include-disabled=off}, resolve-fromsoft-shaders{unbound-textures=preview}")?,
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
    // Installed forward programs receive generic list/tile roles, not zero-filled unknown buffers. Exercise their
    // actual descriptors with independent counts: lookup word 2 keeps nonempty tiles alive in the native ER tile pass.
    let mut listed = 0;
    let mut first_lists = None;
    for stage in world.iter::<ShaderBinding>().flat_map(|(_, b)| &b.stages) {
        let lookup = stage.buffers.iter().find_map(|b| match &b.source {
            BufferSource::LightLists(LightListPart::Lookup(l)) => Some(l),
            _ => None,
        });
        let Some(lookup) = lookup else { continue };
        let indices = stage
            .buffers
            .iter()
            .find_map(|b| match &b.source {
                BufferSource::LightLists(LightListPart::Indices(i)) => Some(i),
                _ => None,
            })
            .ok_or("an installed forward program binds lookup without indices")?;
        for parameter in [FrameParameter::LightTileSize, FrameParameter::LightTilesPerRow] {
            assert!(
                stage
                    .constants
                    .iter()
                    .any(|c| c.role == ConstantRole::Frame(parameter))
            );
        }
        first_lists.get_or_insert_with(|| (lookup.clone(), indices.clone()));
        let packed = pack_light_lists(
            lookup,
            indices,
            &[(LightTableKind::Point, 2), (LightTableKind::Spot, 1)],
            &[],
            1,
        )?;
        let words = |bytes: &[u8]| {
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|w| u32::from_le_bytes(*w))
                .collect::<Vec<_>>()
        };
        assert_eq!(words(&packed.lookup), [0, 0x0001_0002, 0x0001_0002]);
        assert_eq!(words(&packed.indices), [0xffff_0000, 0xffff_0001, 0xffff_0000]);
        assert!(packed.unknown.is_empty());
        listed += 1;
    }
    assert!(listed > 0, "no installed material stage binds the light lists");
    println!("material stages with packed light lists and tile roles: {listed}");

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
    assert_eq!(translation, [-88.0, 16.0, 16.0]);
    let mut tally = Tally::default();
    let mut on_none = [0usize; 2];
    let mut specular_from_colour = 0usize;
    let mut full_records = Vec::new();
    // the records the game shadowed, with the packed light they stand on: (dispatch, light, recorded, ours unshadowed)
    let mut game_shadowed = Vec::new();
    let mut unshadowed_casters = Vec::new();
    for d in &dispatches {
        assert_eq!((d.points.len(), d.spots.len()), (22, 0));
        // the world-to-view matrix of the world frame: the map's z mirrored, the drawing frame's origin moved back
        let mut view = [[0.0, 0.0, 0.0, 1.0]; 4];
        for (row, v) in view.iter_mut().zip(d.view) {
            let moved = (0..3).map(|k| f64::from(v[k]) * translation[k]).sum::<f64>();
            *row = [v[0], v[1], -v[2], (f64::from(v[3]) - moved) as f32];
        }
        for (k, (table, records)) in [(points, &d.points), (spots, &d.spots)].into_iter().enumerate() {
            let kind = if k == 0 { "point" } else { "spot" };
            let packed = table.pack(&world, &view, &LocalShadows::none())?;
            specular_from_colour += packed.specular_from_colour;
            assert!(packed.shadowed.is_empty());
            unshadowed_casters.push((k, packed.unshadowed_casters.clone()));
            let entities = packed.lights.clone();
            let stride = table.stride as usize;
            let packed: Vec<Vec<f32>> = (0..packed.lights.len())
                .map(|i| floats(&packed.bytes[stride * i..stride * (i + 1)]))
                .collect();
            for (native_index, r) in records.iter().enumerate() {
                match packed
                    .iter()
                    .enumerate()
                    .find(|(_, p)| (0..3).all(|c| near(p[c], r[c], 0.01)))
                {
                    Some((index, p)) => {
                        compare(kind, table, p, r, &mut tally);
                        if shadowed(table, r) {
                            game_shadowed.push((d, entities[index], r.clone(), p.clone()));
                        }
                        full_records.push(serde_json::json!({
                            "dispatch": d.id, "native_index": native_index, "kind": kind,
                            "light_name": world.get::<primase_core::Name>(entities[index]).map(|n| &n.0),
                            "view": view, "table": table,
                            "ours_words": p.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                            "native_words": r.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                        }));
                    }
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
    assert_eq!(on_none, [80, 0]);
    {
        let path = inputs::scratch().join("point-record-census.json");
        assert_eq!(full_records.len(), 8);
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "scope": "four retained dispatches; all 8 bank-matched point records, every word including unmodeled fields",
                "map_translation": translation, "records": full_records,
            }))?,
        )?;
    }

    println!("packed lights whose specular is their colour: {specular_from_colour}");
    assert_eq!(specular_from_colour, 0);
    for key in [
        "point view at 0",
        "point range at 12",
        "point range at 60",
        "point translucency at 48",
        "point translucency at 52",
        "point colour exact at 16",
        "point colour exact at 32",
        "point inverse falloff distance at 28",
    ] {
        assert_eq!(tally.get(key), [8, 0], "{key}");
    }

    // LightType: the game shadows bank light 32, the one shadow-casting light the records stand on, in every dispatch;
    // light 116 (casting none) is unshadowed; ours packs both unshadowed
    assert_eq!(tally.get("point shadowed at 64"), [4, 4]);
    let bank: Vec<(primase_core::Entity, &sekiro::source::LightSource)> =
        world.iter::<sekiro::source::LightSource>().collect();
    let light_32 = bank[32].0;
    // the bank's light 32 by its own stored position (the world frame mirrors the map's z)
    let at_32 = world
        .get::<Transform>(light_32)
        .ok_or("light 32 has no transform")?
        .cols[3];
    assert!(
        at_32
            .iter()
            .zip([-122.254, 24.0794, -15.4499])
            .all(|(a, b)| near(*a, b, 1e-3)),
        "{at_32:?}"
    );
    assert!(bank[32].1.cast_shadows);
    assert!(!bank[116].1.cast_shadows);
    assert_eq!(game_shadowed.len(), 4);
    for (_, light, recorded, ours) in &game_shadowed {
        assert_eq!(*light, light_32);
        assert_eq!(
            recorded[16], 1.0,
            "the game's record of light 32 says it is shadowed"
        );
        assert!(
            recorded[20..52].iter().any(|x| *x != 0.0),
            "and places its shadow"
        );
        assert_eq!(ours[16].to_bits(), 0.0f32.to_bits(), "ours packs it unshadowed");
        assert!(ours[20..52].iter().all(|x| x.to_bits() == 0));
    }
    // the packing names every shadow-casting light it packs unshadowed: the bank's nine shadow-casting point lights,
    // light 32 among them, in every dispatch, and no spot
    let casters: Vec<primase_core::Entity> = bank
        .iter()
        .filter(|(_, s)| s.cast_shadows && s.kind == sekiro::source::LightKind::Point)
        .map(|(e, _)| *e)
        .collect();
    assert_eq!(casters.len(), 9);
    assert!(casters.contains(&light_32));
    assert_eq!(unshadowed_casters.len(), 8);
    for (k, named) in &unshadowed_casters {
        let mut named = named.clone();
        named.sort();
        assert_eq!(named, if *k == 0 { casters.clone() } else { Vec::new() });
    }
    println!(
        "light 32 ({}): shadowed by the game in 4 of 4 dispatches, packed unshadowed and named",
        world
            .get::<primase_core::Source>(light_32)
            .map(|s| s.0.as_str())
            .unwrap_or("?")
    );

    // light 32 packed with the shadow its own records place: its record is the game's, LightType and the shadow's place
    // word for word, every other field as above; the lists leave its entry out of the tile pass and the count copy
    let (lookup, indices) = first_lists.ok_or("no light lists")?;
    for (d, _, recorded, _) in &game_shadowed {
        let shadow = LocalShadow {
            projection: recorded[20..24].to_vec(),
            centre: [recorded[24], recorded[25], recorded[26], recorded[27]],
            faces: recorded[28..52].as_chunks::<4>().0.to_vec(),
        };
        let mut view = [[0.0, 0.0, 0.0, 1.0]; 4];
        for (row, v) in view.iter_mut().zip(d.view) {
            let moved = (0..3).map(|k| f64::from(v[k]) * translation[k]).sum::<f64>();
            *row = [v[0], v[1], -v[2], (f64::from(v[3]) - moved) as f32];
        }
        let shadows = LocalShadows([(light_32, shadow)].into());
        let packed = points.pack(&world, &view, &shadows)?;
        let at = packed
            .lights
            .iter()
            .position(|e| *e == light_32)
            .ok_or("light 32 not packed")?;
        assert_eq!(packed.shadowed, vec![at as u32]);
        assert!(!packed.unshadowed_casters.contains(&light_32));
        assert_eq!(packed.unshadowed_casters.len(), 8);
        let ours = floats(&packed.bytes[208 * at..208 * (at + 1)]);
        let words = |f: &[f32]| f.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(words(&ours[16..17]), words(&recorded[16..17]), "LightType");
        assert_eq!(
            words(&ours[20..52]),
            words(&recorded[20..52]),
            "the shadow's place"
        );
        let mut one = Tally::default();
        compare("point", points, &ours, recorded, &mut one);
        for (key, [_, differ]) in &one.0 {
            if !key.contains("colour scaled") {
                assert_eq!(*differ, 0, "{key}");
            }
        }
        for key in [
            "point shadowed at 64",
            "point shadow place at 80",
            "point shadow place at 96",
            "point shadow place at 112",
        ] {
            assert_eq!(one.get(key), [1, 0], "{key}");
        }
        let n = packed.lights.len() as u32;
        let lists = pack_light_lists(
            &lookup,
            &indices,
            &[(LightTableKind::Point, n), (LightTableKind::Spot, 0)],
            &packed
                .shadowed
                .iter()
                .map(|i| (LightTableKind::Point, *i))
                .collect::<Vec<_>>(),
            1,
        )?;
        let list_words = |bytes: &[u8]| {
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|w| u32::from_le_bytes(*w))
                .collect::<Vec<_>>()
        };
        assert_eq!(list_words(&lists.lookup), [0, n, n - 1]);
        let want: Vec<u32> = (0..n)
            .map(|i| i | 0x7fff_0000 | if i == at as u32 { 0 } else { 0x8000_0000 })
            .collect();
        assert_eq!(list_words(&lists.indices), want);
    }

    // light 32 placed by the graph's own local shadow atlas, given its centre in the drawing frame (its stored position
    // less the capture's origin) and its range: its record is the game's word for word from LightType on
    let atlas = graph
        .local_shadow_atlas
        .as_ref()
        .ok_or("the render graph states no local shadow atlas")?;
    let range = world
        .get::<Light>(light_32)
        .and_then(|l| l.range)
        .ok_or("light 32 has no range")?;
    let map = [at_32[0], at_32[1], -at_32[2]];
    let centre = [0, 1, 2].map(|k| map[k] - translation[k] as f32);
    let placed = atlas.place(&[ShadowedLight {
        light: light_32,
        centre,
        range,
    }])?;
    assert_eq!(placed.faces.len(), 6);
    for (d, _, recorded, _) in &game_shadowed {
        let mut view = [[0.0, 0.0, 0.0, 1.0]; 4];
        for (row, v) in view.iter_mut().zip(d.view) {
            let moved = (0..3).map(|k| f64::from(v[k]) * translation[k]).sum::<f64>();
            *row = [v[0], v[1], -v[2], (f64::from(v[3]) - moved) as f32];
        }
        let packed = points.pack(&world, &view, &placed.shadows)?;
        let at = packed
            .lights
            .iter()
            .position(|e| *e == light_32)
            .ok_or("light 32 not packed")?;
        let ours = floats(&packed.bytes[208 * at..208 * (at + 1)]);
        let words = |f: &[f32]| f.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(
            words(&ours[16..52]),
            words(&recorded[16..52]),
            "light 32's record from LightType on, placed by the atlas"
        );
    }

    // its first face's camera is the one the game drew the face's casters with (er3 frame 6990's cbSceneParam of the
    // casters: FC_MatrixView's rows at +80, FC_MatrixProj at +256): the view value for value (exact; the game's forward
    // row holds a -0.0 where ours holds 0.0, the same value), the projection to 1e-6 (the game's far plane is 8.0000057,
    // not the range's 8, so its depth row differs in the last bits)
    let scene = blob(&frames.join("er3"), "9a2574ee8926b3f5")?;
    let face = &placed.faces[0];
    let game_view: Vec<f32> = scene[20..32].to_vec();
    let our_view: Vec<f32> = face.view[..3].iter().flatten().copied().collect();
    assert_eq!(
        our_view, game_view,
        "the +X face's view: ours {our_view:?}, the game's {game_view:?}"
    );
    let game_projection = &scene[64..80];
    for (k, (ours, game)) in face.projection.iter().flatten().zip(game_projection).enumerate() {
        assert!(
            (ours - game).abs() <= 1e-6,
            "the +X face's projection element {k}: ours {ours}, the game's {game}"
        );
    }
    assert_eq!(face.near, 0.008);

    // the shadowed pass's own constants at er3 frame 6990: cb1 (cbLightAccLightParam) is light 32's record, and from
    // LightType on it is the one the atlas places (the frame's origin is the capture's, (-88, 16, 16), too); cb2
    // (cbLightAccShadowInfo) is the atlas's size and its texel's step
    let cb1 = blob(&frames.join("er3"), "433e9a61961462c2")?;
    let shadow = &placed.shadows.0[&light_32];
    let mut place = vec![1.0f32, 0.0, 0.0, 0.0];
    place.extend(&shadow.projection);
    place.extend(shadow.centre);
    place.extend(shadow.faces.iter().flatten());
    assert_eq!(
        place.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
        cb1[16..52].iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
        "frame 6990's cb1 from LightType on"
    );
    let cb2 = blob(&frames.join("er3"), "a602b39fb6b42f83")?;
    let [w, h] = atlas.size.map(|s| s as f32);
    assert_eq!(cb2[..4], [w, h, 1.0 / w, 1.0 / h]);
    Ok(())
}

/// A constant buffer a session's capture holds, as floats: the blob of `hash`.
fn blob(session: &std::path::Path, hash: &str) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    for line in std::fs::read_to_string(session.join("cap").join("capture.jsonl"))?.lines() {
        if !line.contains("\"event\":\"blob\"") || !line.contains(hash) {
            continue;
        }
        let e: serde_json::Value = serde_json::from_str(line)?;
        if e["hash"].as_str() != Some(hash) {
            continue;
        }
        let hex = e["hex"].as_str().ok_or("a blob with no contents")?;
        let bytes = (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16))
            .collect::<Result<Vec<u8>, _>>()?;
        return Ok(floats(&bytes));
    }
    Err(format!("no blob {hash} in {}", session.display()).into())
}
