//! What the engine draws a material with when its effect has no `deferred_draw` (the evidence for
//! `resolve_shaders::fallback_technique`), from the shader database and a region's materials:
//!
//! - The `draw` technique of an effect is its forward pass with up to eight lights: its pixel program is the one of the
//!   effect's `lightweight8_draw` (`PS_Textured_Eight`, beside `PS_Textured_Zero` and `Four` for 0 and 4 lights; the
//!   copy of an effect that also has a depth-of-field pass is named `_DOF`), and `PS_Textured_Eight` writes the one
//!   colour target.
//! - `deferredalphaclip_draw` is a G-buffer pass: where an effect has it beside `deferred_draw` the two write the same
//!   targets and name the same blend, depth and rasterizer records (199 of 200 effects; the licence plate's writes one
//!   target), and the eight effects without a `deferred_draw` that have it write the five G-buffer targets.
//! - A material stores its render bucket (0 opaque, 1 alpha, 2 decal, 3 cutout, 6 water: `passes::lower::draw_order`),
//!   and the materials of the effects with no `deferred_draw` are in the alpha (1), cutout (3) and water (6) buckets:
//!   every cutout one has an effect with `deferredalphaclip_draw`, every other one with `draw`, but the foam and pool
//!   water effects, which have neither.
//!
//! The counts are printed and the statements asserted. Needs `installs.gta` and `keys.gta`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use std::collections::BTreeMap;

use primase_core::World;
use primase_core::shader::{Shader, SystemValue};
use primase_gta::components::{
    GtaEffect, GtaEffectTechniques, GtaMaterialEffect, GtaPass, GtaShader, GtaTechnique,
};
use primase_gta::passes::resolve_shaders::{fallback_technique, split_program_name};
use primase_gta::read::source::GtaSource;
use primase_testdata::{Game, debug, install, keys};

fn database() -> Option<(GtaSource, World)> {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return None;
    };
    let source = GtaSource::open(&dir, &keys).unwrap();
    let mut world = World::new();
    source.add_shader_database(&mut world).unwrap();
    Some((source, world))
}

fn technique<'a>(t: &'a GtaEffectTechniques, name: &str) -> Option<&'a GtaTechnique> {
    t.techniques.iter().find(|t| t.name == name)
}

fn pixel_name(pass: &GtaPass) -> Option<&str> {
    pass.pixel.as_ref().map(|p| p.name.as_str())
}

/// The `SV_Target` indices and widths a pass's pixel program writes.
fn targets(world: &World, pass: &GtaPass) -> Vec<(u32, u32)> {
    pass.pixel
        .as_ref()
        .and_then(|p| p.shader)
        .and_then(|s| world.get::<Shader>(s))
        .map(|s| {
            s.interface
                .outputs
                .iter()
                .filter(|v| matches!(v.system, None | Some(SystemValue::Target(_))))
                .map(|v| (v.label.index, v.value.width.into()))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn the_draw_technique_is_the_eight_light_forward_pass() {
    let Some((_, world)) = database() else {
        return;
    };
    let (mut both, mut single_colour) = (0usize, 0usize);
    for (e, effect) in world.iter::<GtaEffect>() {
        let Some(t) = world.get::<GtaEffectTechniques>(e) else {
            continue;
        };
        let (Some(draw), Some(eight)) = (
            technique(t, "draw").and_then(|t| t.passes.first()),
            technique(t, "lightweight8_draw").and_then(|t| t.passes.first()),
        ) else {
            continue;
        };
        both += 1;
        // the eight-light pass of an effect that also has a depth-of-field pass (`_DOF`) or its own copy for the
        // lightweight techniques (`_lightweight8`) is the same program under that name
        let stem = |p: Option<&str>| {
            p.and_then(split_program_name)
                .map(|(stem, _)| stem.replace("_DOF", "").replace("_lightweight8", ""))
        };
        assert_eq!(
            stem(pixel_name(draw)),
            stem(pixel_name(eight)),
            "{}: draw and lightweight8_draw run different programs",
            effect.name
        );
        if pixel_name(draw).and_then(split_program_name).map(|(s, _)| s) == Some("PS_Textured_Eight") {
            single_colour += 1;
            assert_eq!(targets(&world, draw), [(0, 4)], "{}", effect.name);
        }
    }
    println!("{both} effects with draw and lightweight8_draw, {single_colour} running PS_Textured_Eight");
    assert!(both > 100 && single_colour > 100);
}

#[test]
fn the_alpha_clip_pass_is_a_g_buffer_pass_like_deferred_draw() {
    let Some((_, world)) = database() else {
        return;
    };
    let (mut both, mut alike) = (0usize, 0usize);
    for (e, effect) in world.iter::<GtaEffect>() {
        let Some(t) = world.get::<GtaEffectTechniques>(e) else {
            continue;
        };
        let (Some(deferred), Some(clip)) = (
            technique(t, "deferred_draw").and_then(|t| t.passes.first()),
            technique(t, "deferredalphaclip_draw").and_then(|t| t.passes.first()),
        ) else {
            continue;
        };
        both += 1;
        let same = targets(&world, deferred) == targets(&world, clip)
            && (deferred.rasterizer, deferred.depth_stencil, deferred.blend)
                == (clip.rasterizer, clip.depth_stencil, clip.blend);
        if same {
            alike += 1;
        } else {
            println!(
                "DIFFERENT {}: deferred_draw writes {:?}, deferredalphaclip_draw {:?}",
                effect.name,
                targets(&world, deferred),
                targets(&world, clip)
            );
        }
    }
    println!(
        "{both} effects with deferred_draw and deferredalphaclip_draw, {alike} with the same targets and state"
    );
    assert!(both > 100 && alike + 1 >= both);
    // the effects without deferred_draw that have the alpha-clip pass write the same five targets
    let mut without = 0usize;
    for (e, _) in world.iter::<GtaEffect>() {
        let Some(t) = world.get::<GtaEffectTechniques>(e) else {
            continue;
        };
        if technique(t, "deferred_draw").is_some() {
            continue;
        }
        let Some(clip) = technique(t, "deferredalphaclip_draw").and_then(|t| t.passes.first()) else {
            continue;
        };
        without += 1;
        assert_eq!(targets(&world, clip).len(), 5);
    }
    println!("{without} effects with deferredalphaclip_draw and no deferred_draw");
    assert!(without >= 5);
}

/// The materials of a region (Vespucci Beach by default; the debug key `gta.region`) whose effect has no `deferred_draw`,
/// by render bucket and the pass the fallback rule finds for them.
#[test]
fn the_buckets_of_the_materials_without_a_g_buffer_pass() {
    let Some((source, _)) = database() else {
        return;
    };
    let region: Vec<f32> = debug("gta.region")
        .unwrap_or_else(|| "-1500,-1650,-1150,-1330".into())
        .split(',')
        .map(|v| v.trim().parse().unwrap())
        .collect();
    let (read, _) = source
        .read([region[0], region[1], region[2], region[3]], 600.0)
        .unwrap();
    let (mut world, _): (World, _) = source.load_all(&read).unwrap();
    source.add_shader_database(&mut world).unwrap();
    // (bucket, found pass or none) -> materials, by effect
    let mut found: BTreeMap<(u8, String), (usize, BTreeMap<String, usize>)> = BTreeMap::new();
    for (e, link) in world.iter::<GtaMaterialEffect>() {
        let (Some(shader), Some(effect), Some(t)) = (
            world.get::<GtaShader>(e),
            world.get::<GtaEffect>(link.effect),
            world.get::<GtaEffectTechniques>(link.effect),
        ) else {
            continue;
        };
        if technique(t, "deferred_draw").is_some() {
            continue;
        }
        let pass = fallback_technique(t, "deferred_draw", shader.render_bucket)
            .map_or("none".to_owned(), |(name, _)| name);
        let entry = found.entry((shader.render_bucket, pass)).or_default();
        entry.0 += 1;
        *entry.1.entry(effect.name.clone()).or_default() += 1;
    }
    for ((bucket, pass), (n, effects)) in &found {
        let mut top: Vec<_> = effects.iter().collect();
        top.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        let top: Vec<String> = top.iter().take(6).map(|(e, n)| format!("{e}:{n}")).collect();
        println!(
            "BUCKET {bucket} -> {pass}: {n} materials in {} effects ({})",
            effects.len(),
            top.join(" ")
        );
    }
    for (bucket, pass) in found.keys() {
        assert!(
            [1, 3, 6].contains(bucket),
            "a material of bucket {bucket} has no deferred_draw"
        );
        if *bucket == 3 {
            assert!(pass == "deferredalphaclip_draw" || pass == "none");
        }
    }
    let by = |bucket: u8, pass: &str| found.get(&(bucket, pass.to_owned())).map_or(0, |(n, _)| *n);
    assert!(by(3, "deferredalphaclip_draw") > 100);
    assert!(by(1, "draw") > 1_000);
    assert!(by(6, "draw") >= 20);
}
