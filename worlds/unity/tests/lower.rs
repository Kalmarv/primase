//! Lowering gate against an independent port's exported materials (`PRIMASE_UNITY_PORT`: its output folder, with
//! `textures/*.png`; `PRIMASE_UNITY_PORT_MATERIALS`: JSON `joined`, each `key`, `file`, `path_id` and the port's record
//! `port` with `shader`, `alpha` (opaque, clip, blend), `cutoff`, `double_sided`, `metallic`, `smoothness`, `texture`
//! (its base-colour image: texture x colour multiplied in linear space, sRGB, alpha dropped when opaque) or `skip`),
//! for the level `level1` of `PRIMASE_UNITY_DATA` read with `PRIMASE_UNITY_TPK`.
//!
//! Per material lowered: alpha mode and cutoff, double-sidedness, metallic and smoothness (when no map replaces them)
//! exactly; the base colour (our texture x factor, encoded as the port encodes it) against the port's image, the largest
//! channel difference reported by shader family: within 1 for the shaders modelled (the port's BC1-3 decoder rounds
//! palettes otherwise, see the texture gate), and for shaders not modelled too, listed per material, except where the
//! port picks other properties (named). Differences in rule are counted by name.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::Path;

use primase_core::Texture;
use primase_core::World;
use primase_core::pbr::{AlphaMode, PbrMaterial, UvTransform};
use primase_image::{Samples, linear_to_srgb, read_png};
use primase_testdata::real_data;
use primase_texcodec::{Decoder, TexDecoder};
use primase_unity::TypeTreePackage;
use primase_unity::components::UnityMaterial;
use primase_unity::level::read_level;
use primase_unity::lower::lower_unity_pbr;
use primase_unity::player::Player;
use serde_json::Value as Json;

/// Our base colour as the port encodes it: RGBA8 pixels of level 0 and their size.
fn base_image(w: &World, p: &PbrMaterial) -> (u32, u32, Vec<u8>) {
    let srgb = |x: f32| linear_to_srgb(f64::from(x));
    let factor = p.base_color;
    let Some(r) = p.base_color_texture else {
        let px = [
            srgb(factor[0]),
            srgb(factor[1]),
            srgb(factor[2]),
            f64::from(factor[3]),
        ]
        .map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
        return (4, 4, px.repeat(16));
    };
    let t: &Texture = w.get(r.texture).unwrap();
    let mut px = Decoder::default()
        .decode_rgba8(t.format, t.width, t.height, &t.levels[0])
        .unwrap();
    let white = factor[..3].iter().all(|&c| c == 1.0);
    for p in px.as_chunks_mut::<4>().0 {
        if !white {
            for c in 0..3 {
                let lin = primase_image::srgb_to_linear(f64::from(p[c]) / 255.0) * f64::from(factor[c]);
                p[c] = (linear_to_srgb(lin).clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
        p[3] = ((f64::from(p[3]) / 255.0 * f64::from(factor[3])).clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    (t.width, t.height, px)
}

#[derive(Default)]
struct Images {
    materials: u64,
    equal: u64,
    channels: u64,
    differ: u64,
    max: u8,
    at: String,
}

#[test]
fn lowering_matches_the_port() {
    let (Some(data), Some(tpk), Some(port_dir), Some(joined)) = (
        real_data("PRIMASE_UNITY_DATA"),
        real_data("PRIMASE_UNITY_TPK"),
        real_data("PRIMASE_UNITY_PORT"),
        real_data("PRIMASE_UNITY_PORT_MATERIALS"),
    ) else {
        return;
    };
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let mut w = World::new();
    read_level(&player, "level1", &mut w).unwrap();
    as_stored(&mut w);
    let log = lower_unity_pbr(&mut w).unwrap();
    println!(
        "lowering: {:?}\napproximations: {:#?}",
        log.counts, log.approximations
    );
    let joined: Json = serde_json::from_slice(&std::fs::read(&joined).unwrap()).unwrap();
    let ours: BTreeMap<(String, i64), (primase_core::Entity, &UnityMaterial)> = w
        .iter::<UnityMaterial>()
        .map(|(e, m)| ((m.material.file.clone(), m.material.path_id), (e, m)))
        .collect();
    let mut rules: BTreeMap<String, u64> = BTreeMap::new();
    let mut images: BTreeMap<String, Images> = BTreeMap::new();
    let mut fields_differ = Vec::new();
    let mut not_modelled = Vec::new();
    let joined = joined["joined"].as_array().unwrap();
    assert!(!joined.is_empty(), "the port's material list is empty");
    let mut compared = 0usize;
    let mut missing: Vec<String> = Vec::new();
    for j in joined {
        let key = (
            j["file"].as_str().unwrap().to_owned(),
            j["path_id"].as_i64().unwrap(),
        );
        let port = &j["port"];
        let Some(&(e, um)) = ours.get(&key) else {
            missing.push(j["key"].as_str().unwrap().to_owned());
            continue;
        };
        if port.get("skip").is_some() {
            *rules
                .entry(format!("port skips: {}", port["skip"].as_str().unwrap()))
                .or_default() += 1;
            continue;
        }
        let p: &PbrMaterial = w.get(e).unwrap();
        compared += 1;
        let shader = port["shader"].as_str().unwrap().to_owned();
        let family = if port["exact"].as_bool().unwrap() {
            shader.clone()
        } else {
            "not modelled".to_owned()
        };
        // fields
        let alpha = match p.alpha_mode {
            AlphaMode::Opaque => "opaque",
            AlphaMode::Mask { .. } => "clip",
            AlphaMode::Blend => "blend",
        };
        let port_alpha = port["alpha"].as_str().unwrap();
        if alpha != port_alpha {
            if alpha == "blend"
                && port_alpha == "opaque"
                && p.base_color_texture.is_none()
                && p.base_color[3] >= 0.999
            {
                *rules
                    .entry("port draws untextured opaque-alpha blends opaque".into())
                    .or_default() += 1;
            } else if alpha == "blend" && port_alpha == "opaque" && um.float("_Surface") != Some(1.0) {
                // the port reads blending from URP Lit's `_Surface` alone; the material's draw pass blends
                *rules
                    .entry("port takes blending from _Surface only: a blending draw pass drawn opaque".into())
                    .or_default() += 1;
            } else {
                fields_differ.push(format!("{}: alpha {alpha} vs {port_alpha}", j["key"]));
            }
        }
        if let AlphaMode::Mask { cutoff } = p.alpha_mode
            && f64::from(cutoff) != port["cutoff"].as_f64().unwrap()
        {
            fields_differ.push(format!("{}: cutoff {cutoff} vs {}", j["key"], port["cutoff"]));
        }
        if p.double_sided != port["double_sided"].as_bool().unwrap() {
            fields_differ.push(format!("{}: double sided", j["key"]));
        }
        if family == "Universal Render Pipeline/Lit" {
            if um.float("_WorkflowMode") == Some(0.0) {
                *rules
                    .entry("specular workflow: we lower a dielectric, the port keeps _Metallic".into())
                    .or_default() += 1;
            } else if p.metallic_roughness_texture.is_some() {
                *rules
                    .entry("metallic and smoothness from a map (the port keeps the factors)".into())
                    .or_default() += 1;
            } else {
                let s = 1.0 - f64::from(p.roughness);
                if f64::from(p.metallic) != port["metallic"].as_f64().unwrap()
                    || (s - port["smoothness"].as_f64().unwrap()).abs() > f64::from(f32::EPSILON)
                {
                    fields_differ.push(format!(
                        "{}: metallic {} smoothness {s} vs {} {}",
                        j["key"], p.metallic, port["metallic"], port["smoothness"]
                    ));
                }
            }
        }
        // the base colour image
        let png = read_png(
            &std::fs::read(
                Path::new(&port_dir)
                    .join("textures")
                    .join(port["texture"].as_str().unwrap()),
            )
            .unwrap(),
        )
        .unwrap()
        .image;
        let Samples::U8(theirs) = png.samples else {
            panic!("16-bit image")
        };
        let (width, height, mine) = base_image(&w, p);
        let img = images.entry(family.clone()).or_default();
        img.materials += 1;
        if (png.width, png.height) != (width, height) {
            fields_differ.push(format!(
                "{}: image {width}x{height} vs {}x{}",
                j["key"], png.width, png.height
            ));
            continue;
        }
        let opaque = port_alpha == "opaque";
        let mut equal = true;
        for (i, (a, b)) in mine.iter().zip(&theirs).enumerate() {
            if opaque && i % 4 == 3 {
                continue;
            }
            img.channels += 1;
            let d = a.abs_diff(*b);
            if d != 0 {
                img.differ += 1;
                equal = false;
            }
            if d > img.max {
                img.max = d;
                img.at = format!(
                    "{} ({}) pixel {} channel {}: {a} vs {b}",
                    j["key"],
                    um.shader.as_deref().unwrap_or(""),
                    i / 4,
                    i % 4
                );
            }
        }
        if equal {
            img.equal += 1;
        }
        if family == "not modelled" {
            let max = mine
                .iter()
                .zip(&theirs)
                .enumerate()
                .filter(|(i, _)| !(opaque && i % 4 == 3))
                .map(|(_, (a, b))| a.abs_diff(*b))
                .max()
                .unwrap_or(0);
            if shader == "FlatKit/Water" {
                *rules
                    .entry("FlatKit/Water: the port draws _ColorShallow, we the usual base names".into())
                    .or_default() += 1;
            } else if max > 1 {
                fields_differ.push(format!("{}: base colour differs by {max} ({shader})", j["key"]));
            }
            not_modelled.push(format!(
                "{} [{}]: largest difference {max}; port: {}",
                j["key"], shader, port["notes"]
            ));
        }
    }
    println!("rules that differ, by name: {rules:#?}");
    for (f, i) in &images {
        println!(
            "base colour, {f}: {} materials, {} equal; {} channels, {} differ, largest difference {} ({})",
            i.materials, i.equal, i.channels, i.differ, i.max, i.at
        );
    }
    println!("not modelled, per material: {not_modelled:#?}");
    println!("fields that differ: {fields_differ:#?}");
    assert!(fields_differ.is_empty());
    // every material the port exported is one our level draws, and each not skipped by the port was compared
    let skipped: u64 = rules
        .iter()
        .filter(|(k, _)| k.starts_with("port skips"))
        .map(|(_, n)| n)
        .sum();
    println!("port materials not among our level's: {missing:?}");
    // the one the port exported that no renderer or terrain of the level references (an independent reader finds
    // none; the port picked it up elsewhere): anything else missing is ours to explain
    assert_eq!(
        missing,
        vec!["SnowURP01_medium_coverage_on_objects_1c6c4e".to_owned()],
        "port materials missing from our level"
    );
    assert_eq!(
        compared as u64 + skipped + missing.len() as u64,
        joined.len() as u64
    );
    assert!(compared > 0, "no material compared");
    // URP Lit and SpeedTree7 within 1: the port decodes BC1-3 with another palette rounding (the texture gate)
    for (f, i) in &images {
        if f != "not modelled" {
            assert!(i.max <= 1, "{f}: base colour differs by {}", i.max);
        }
    }
}

/// Draw order as Unity's render queue, on a fact an independent reader (UnityPy) states for this level: the ice-fishing
/// water (FlatKit/Water) has `m_CustomRenderQueue` 2050 (Geometry plus URP's `_QueueOffset` 50) and lowers opaque.
/// The queue rules themselves are gated by the unit cases in `components.rs`.
#[test]
fn draw_order_follows_the_render_queue() {
    use primase_core::{DrawLayer, DrawOrder, Name};
    let (Some(data), Some(tpk)) = (real_data("PRIMASE_UNITY_DATA"), real_data("PRIMASE_UNITY_TPK")) else {
        return;
    };
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let mut w = World::new();
    read_level(&player, "level1", &mut w).unwrap();
    as_stored(&mut w);
    lower_unity_pbr(&mut w).unwrap();
    let water: Vec<_> = w
        .iter::<UnityMaterial>()
        .filter(|(e, m)| {
            m.shader.as_deref() == Some("FlatKit/Water")
                && w.get::<Name>(*e)
                    .is_some_and(|n| n.0 == "My Water for Icefishing")
        })
        .map(|(e, _)| *w.get::<DrawOrder>(e).unwrap())
        .collect();
    assert_eq!(water, vec![DrawOrder::conventional(DrawLayer::Opaque, 2050)]);
    // every lowered material has one
    assert!(w.iter::<UnityMaterial>().all(|(e, _)| w.has::<DrawOrder>(e)));
}

/// The level's water and tree materials state their usage, and no other material does. Facts read with UnityPy:
/// `My Water for Icefishing` (FlatKit/Water) draws the eight fishing-hole quads; `Tree Mat` (SpeedTree 7, keywords
/// `EFFECT_HUE_VARIATION GEOM_TYPE_MESH`) draws the Evergreen trees, trunk and needles in one mesh; `sled_lvl_mat 12`
/// (`Shader Graphs/ocean`) draws a spray-paint bottle and stays ordinary. The table's rules are gated by the unit
/// cases in `lower.rs`.
#[test]
fn water_and_trees_state_their_usage() {
    use primase_core::{MaterialUsage, Name};
    use std::collections::BTreeSet;
    let (Some(data), Some(tpk)) = (real_data("PRIMASE_UNITY_DATA"), real_data("PRIMASE_UNITY_TPK")) else {
        return;
    };
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let mut w = World::new();
    read_level(&player, "level1", &mut w).unwrap();
    as_stored(&mut w);
    lower_unity_pbr(&mut w).unwrap();
    let used: BTreeSet<(String, MaterialUsage)> = w
        .iter::<MaterialUsage>()
        .map(|(e, u)| (w.get::<Name>(e).unwrap().0.clone(), *u))
        .collect();
    assert_eq!(
        used,
        BTreeSet::from([
            ("My Water for Icefishing".to_owned(), MaterialUsage::Water),
            ("Tree Mat".to_owned(), MaterialUsage::Foliage),
        ])
    );
    let ocean = w
        .iter::<UnityMaterial>()
        .filter(|(_, m)| m.shader.as_deref() == Some("Shader Graphs/ocean"))
        .count();
    assert_eq!(ocean, 1, "the spray-paint bottle's material is read");
}

/// URP Lit samples every map through `_BaseMap`'s tiling (one `TRANSFORM_TEX` in its vertex pass), so each lowered
/// slot carries that transform whatever its own property's tiling says. Counts the slots whose own tiling differs
/// (the case a per-property reading gets wrong).
#[test]
fn lit_slots_sample_through_the_base_maps_tiling() {
    let (Some(data), Some(tpk)) = (real_data("PRIMASE_UNITY_DATA"), real_data("PRIMASE_UNITY_TPK")) else {
        return;
    };
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let mut w = World::new();
    read_level(&player, "level1", &mut w).unwrap();
    as_stored(&mut w);
    lower_unity_pbr(&mut w).unwrap();
    let tiling = |m: &UnityMaterial, name: &str| {
        m.textures
            .iter()
            .find(|t| t.name == name)
            .map_or(([1.0; 2], [0.0; 2]), |t| (t.scale, t.offset))
    };
    let (mut slots, mut own_differs) = (0, 0);
    for (e, m) in w.iter::<UnityMaterial>() {
        if m.shader.as_deref() != Some("Universal Render Pipeline/Lit") {
            continue;
        }
        let p: &PbrMaterial = w.get(e).unwrap();
        let base = tiling(m, "_BaseMap");
        let want = (base != ([1.0; 2], [0.0; 2])).then(|| UvTransform::from_bottom_left(base.0, base.1));
        for (property, slot) in [
            ("_BaseMap", p.base_color_texture),
            ("_MetallicGlossMap", p.metallic_roughness_texture),
            ("_BumpMap", p.normal_texture),
            ("_OcclusionMap", p.occlusion_texture),
            ("_EmissionMap", p.emissive_texture),
        ] {
            let Some(r) = slot else { continue };
            slots += 1;
            own_differs += u64::from(tiling(m, property) != base);
            assert_eq!(r.transform, want, "{property} of {:?}", m.material);
        }
    }
    println!("URP Lit slots: {slots}, through the base map's tiling; {own_differs} whose own tiling differs");
    assert!(slots > 0);
}

/// The level as stored: `resolve-state` with the defaults, then LOD 0 of every prototype.
fn as_stored(w: &mut primase_core::World) {
    let mut run = primase_core::pass::RunReport::default();
    primase_core::pass::PassManager::new(primase_core::pass::PassRegistry::new())
        .run(
            w,
            &primase_core::pass::Pipeline {
                passes: vec![
                    Box::new(primase_core::state::ResolveState::default()),
                    Box::new(primase_core::SelectLod {
                        choice: primase_core::LodChoice::Finest,
                    }),
                ],
            },
            &mut run,
        )
        .unwrap();
}
