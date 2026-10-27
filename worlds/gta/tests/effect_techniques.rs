//! The techniques of the Enhanced shader database's effects ([`rage_engine_formats::sgd2_effect`]), against what the
//! database says elsewhere:
//!
//! - Every effect's record is read or named: the effects with a technique table are read in full (every pass names
//!   programs the effect lists, no geometry program, no unread table), the others (the post-processing and lighting
//!   effects) are counted.
//! - The passes' programs are the ones the programs' own names pair: in each pass the vertex, pixel, hull and domain
//!   programs carry one permutation hash in their names (`VS_TransformD_ff07e6a3_Wrapped`,
//!   `PS_DeferredTextured_ff07e6a3_Wrapped`), which the pass records never state. The number of passes where they do not
//!   is printed and held to a bound.
//! - The state tables fit what the effects are: `default` has a depth state testing with GREATER (the reversed
//!   depth buffer) and no blend state that blends source alpha over inverse source alpha; `decal` has one, and
//!   `emissive_additive_alpha` a state that adds source alpha times the colour (source alpha, destination one).
//!
//! The counts are printed. Needs `installs.gta` and `keys.gta`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod inputs;

use std::collections::BTreeMap;

use inputs::{Game, install, keys};
use primase_core::World;
use primase_gta::components::{GtaEffect, GtaEffectProgram, GtaEffectTechniques};
use primase_gta::passes::resolve_shaders::split_program_name;
use primase_gta::read::source::GtaSource;

fn hash_of(p: &Option<GtaEffectProgram>) -> Option<String> {
    p.as_ref()
        .and_then(|p| split_program_name(&p.name).map(|(_, h)| h.to_owned()))
}

#[test]
fn every_effect_with_a_technique_table_reads_in_full() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).unwrap();
    let mut world = World::new();
    let (table, _) = source.add_shader_database(&mut world).unwrap();
    println!(
        "{} effects: {} with techniques ({} passes), {} without; unread {:?}",
        table.effects.len(),
        table.techniques,
        table.passes,
        table.without_techniques,
        table.unread_techniques
    );
    assert!(
        table.unread_techniques.is_empty(),
        "{:?}",
        table.unread_techniques
    );
    assert_eq!(table.techniques + table.without_techniques, table.effects.len());
    assert!(table.techniques > 300);

    let (mut passes, mut paired, mut tessellated, mut tessellated_paired) = (0usize, 0usize, 0usize, 0usize);
    let mut unpaired = Vec::new();
    let mut with_blend: BTreeMap<String, bool> = BTreeMap::new();
    let mut with_additive: BTreeMap<String, bool> = BTreeMap::new();
    let mut with_greater: BTreeMap<String, bool> = BTreeMap::new();
    for (entity, e) in world.iter::<GtaEffect>() {
        let Some(t) = world.get::<GtaEffectTechniques>(entity) else {
            continue;
        };
        with_blend.insert(
            e.name.clone(),
            t.blend.iter().any(|b| {
                b.targets
                    .iter()
                    .any(|r| r.enable == 1 && r.src == 5 && r.dest == 6)
            }),
        );
        with_additive.insert(
            e.name.clone(),
            t.blend.iter().any(|b| {
                b.targets
                    .iter()
                    .any(|r| r.enable == 1 && r.src == 5 && r.dest == 2)
            }),
        );
        with_greater.insert(
            e.name.clone(),
            t.depth_stencil
                .iter()
                .any(|d| d.depth_enable == 1 && d.depth_func == 5),
        );
        for technique in &t.techniques {
            assert!(!technique.name.is_empty());
            for pass in &technique.passes {
                passes += 1;
                let hashes: Vec<String> = [&pass.vertex, &pass.pixel, &pass.hull, &pass.domain]
                    .into_iter()
                    .filter_map(hash_of)
                    .collect();
                let one = hashes.windows(2).all(|w| w[0] == w[1]);
                let tessellation = pass.hull.is_some();
                assert_eq!(
                    pass.hull.is_some(),
                    pass.domain.is_some(),
                    "{} {}",
                    e.name,
                    technique.name
                );
                paired += usize::from(one);
                tessellated += usize::from(tessellation);
                tessellated_paired += usize::from(tessellation && one);
                if !one && unpaired.len() < 8 {
                    unpaired.push(format!("{} {}: {hashes:?}", e.name, technique.name));
                }
            }
        }
    }
    println!(
        "{passes} passes: {paired} carry one permutation hash in the names of their programs; of {tessellated} tessellated passes {tessellated_paired}; others, for instance {unpaired:?}"
    );
    assert!(passes > 10_000);
    assert!(paired * 100 >= passes * 90, "{paired} of {passes}: {unpaired:?}");
    // the state tables: opaque `default` does not blend alpha over, `decal` does; `default` tests with GREATER
    assert_eq!(with_blend.get("default"), Some(&false));
    assert_eq!(with_blend.get("decal"), Some(&true));
    assert_eq!(with_additive.get("emissive_additive_alpha"), Some(&true));
    assert_eq!(with_additive.get("default"), Some(&false));
    assert_eq!(with_greater.get("default"), Some(&true));
}

/// The pass records name the effect's state tables: over the effects with two or more records of a table, the values of
/// the pass byte are exactly the indices of the table (each record used, none past it), and an effect with one record or
/// none names index 0 on every pass; and the records the passes name fit what the techniques are called.
#[test]
fn the_passes_name_the_records_of_their_effects_state_tables() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).unwrap();
    let mut world = World::new();
    source.add_shader_database(&mut world).unwrap();
    let (mut multi, mut covered) = ([0usize; 3], [0usize; 3]);
    let mut violations = Vec::new();
    // by technique name: (passes, passes whose render target 0 write mask is 8 / enabled blend with factors / opaque)
    let (mut write_alpha, mut write_alpha_masked) = (0usize, 0usize);
    let (mut refraction, mut refraction_blended) = (0usize, 0usize);
    let (mut reflection, mut reflection_opaque) = (0usize, 0usize);
    let (mut imposters, mut imposters_uncullled) = (0usize, 0usize);
    for (entity, effect) in world.iter::<GtaEffect>() {
        let Some(t) = world.get::<GtaEffectTechniques>(entity) else {
            continue;
        };
        let tables = [t.rasterizer.len(), t.depth_stencil.len(), t.blend.len()];
        let mut used: [std::collections::BTreeSet<u8>; 3] = Default::default();
        for technique in &t.techniques {
            for pass in &technique.passes {
                for (k, index) in [pass.rasterizer, pass.depth_stencil, pass.blend]
                    .into_iter()
                    .enumerate()
                {
                    used[k].insert(index);
                }
                let blend = t.blend.get(usize::from(pass.blend)).map(|b| b.targets[0]);
                let name = technique.name.as_str();
                if name.starts_with("deferredsubsamplewritealpha") {
                    write_alpha += 1;
                    write_alpha_masked += usize::from(blend.is_some_and(|b| b.write_mask == 8));
                }
                // the licence plate's refraction passes are opaque (its one exception)
                if name.starts_with("lightweight")
                    && name.contains("WaterRefractionAlpha")
                    && effect.name != "vehicle_licenseplate"
                {
                    refraction += 1;
                    refraction_blended +=
                        usize::from(blend.is_some_and(|b| b.enable == 1 && b.src == 5 && b.dest == 6));
                }
                if (name.starts_with("reflectiongbuffer") || name.starts_with("mirrorReflectiongbuffer"))
                    && !t.blend.is_empty()
                {
                    reflection += 1;
                    reflection_opaque +=
                        usize::from(blend.is_some_and(|b| b.enable == 0 && b.write_mask == 0xf));
                }
                // the cut-out billboards of the distant trees are drawn two-sided (their shadow is not; the water effects' `imposter_draw` is another thing)
                if name.starts_with("imposter")
                    && name != "impostershadow_draw"
                    && (effect.name.starts_with("trees") || effect.name == "bvh_skinning")
                {
                    imposters += 1;
                    imposters_uncullled += usize::from(
                        t.rasterizer
                            .get(usize::from(pass.rasterizer))
                            .is_some_and(|r| r.cull == 1),
                    );
                }
            }
        }
        for k in 0..3 {
            if tables[k] >= 2 {
                multi[k] += 1;
                let all: std::collections::BTreeSet<u8> = (0..u8::try_from(tables[k]).unwrap()).collect();
                if used[k] == all {
                    covered[k] += 1;
                } else {
                    violations.push(format!(
                        "{} table {k}: {} records, passes name {:?}",
                        effect.name, tables[k], used[k]
                    ));
                }
            } else if used[k].iter().any(|&i| i != 0) {
                violations.push(format!(
                    "{} table {k}: {} records, passes name {:?}",
                    effect.name, tables[k], used[k]
                ));
            }
        }
    }
    println!(
        "effects with two or more records whose passes name exactly their indices: rasterizer {}/{}, depth-stencil {}/{}, blend {}/{}",
        covered[0], multi[0], covered[1], multi[1], covered[2], multi[2]
    );
    println!(
        "deferredsubsamplewritealpha passes with the alpha-only write mask {write_alpha_masked}/{write_alpha}; water refraction passes blending source alpha over its inverse {refraction_blended}/{refraction}; reflection passes opaque in all channels {reflection_opaque}/{reflection}; imposter passes without culling {imposters_uncullled}/{imposters}"
    );
    assert!(violations.is_empty(), "{violations:?}");
    assert!(multi[0] >= 15 && multi[1] >= 50 && multi[2] >= 150, "{multi:?}");
    assert_eq!(write_alpha_masked, write_alpha);
    assert_eq!(refraction_blended, refraction);
    assert_eq!(reflection_opaque, reflection);
    assert_eq!(imposters_uncullled, imposters);
}
