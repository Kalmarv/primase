//! The techniques of the Enhanced shader database's effects ([`primase_gta::rage::sgd2_effect`]), against what the
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
use std::collections::BTreeMap;

use primase_core::World;
use primase_gta::components::{GtaEffect, GtaEffectProgram, GtaEffectTechniques};
use primase_gta::passes::resolve_shaders::split_program_name;
use primase_gta::read::source::GtaSource;
use primase_testdata::{Game, install, keys};

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
