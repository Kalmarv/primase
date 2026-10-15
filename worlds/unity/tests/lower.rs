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
use primase_core::gate::real_data;
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_image::{Samples, linear_to_srgb, read_png};
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
    for j in joined["joined"].as_array().unwrap() {
        let key = (
            j["file"].as_str().unwrap().to_owned(),
            j["path_id"].as_i64().unwrap(),
        );
        let port = &j["port"];
        let Some(&(e, um)) = ours.get(&key) else {
            *rules
                .entry("port material not drawn by our level (terrain trees come with terrain)".into())
                .or_default() += 1;
            continue;
        };
        if port.get("skip").is_some() {
            *rules
                .entry(format!("port skips: {}", port["skip"].as_str().unwrap()))
                .or_default() += 1;
            continue;
        }
        let p: &PbrMaterial = w.get(e).unwrap();
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
    // URP Lit and SpeedTree7 within 1: the port decodes BC1-3 with another palette rounding (the texture gate)
    for (f, i) in &images {
        if f != "not modelled" {
            assert!(i.max <= 1, "{f}: base colour differs by {}", i.max);
        }
    }
}
