//! The samplers the texture samples of the Enhanced shader database's programs read through
//! ([`primase_gta::passes::sampler_use`]): every sample of every vertex and pixel program of every technique is traced,
//! either to a sampler the program declares or to a word of a constant buffer that is the program's
//! `g_rage_dynamicsamplerindices` (one word per `gDynamicSamplerIndex_<name>` constant).
//!
//! - The counts are printed: samples, traced, untraced by reason, the words by name.
//! - The static samplers' names ([`primase_gta::passes::sampler_state::named`]) state a state: the form of every static
//!   name the programs use is one the function reads, and the effect's sampler records are held against it (the share
//!   of uses whose effect has a record of that state is printed and bounded).
//! - The effects' sampler records have the one maximum LOD, which the state does not carry (the biases it does).
//!
//! Needs `installs.gta` and `keys.gta`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use std::collections::{BTreeMap, BTreeSet};

use iridia_interface::ResourceKind;
use primase_core::World;
use primase_core::shader::Shader;
use primase_gta::components::{GtaEffect, GtaEffectTechniques, GtaShaderTable};
use primase_gta::passes::sampler_state;
use primase_gta::passes::sampler_use::{self, Through};
use primase_gta::rage::sgd2_table::ProgramTable;
use primase_gta::read::source::GtaSource;
use primase_testdata::{Game, install, keys};

/// The name of the constant buffer and of the constant at a byte offset of the buffer at a bind point.
fn word_name(table: &ProgramTable, buffer: (u32, u32), offset: u32) -> Option<(String, String)> {
    let (i, r) = table.resources.iter().enumerate().find(|(_, r)| {
        u32::from(r.space) == buffer.0 && u32::from(r.register) == buffer.1 && r.constants > 0
    })?;
    let c = table
        .constants_of(i)
        .iter()
        .find(|c| u32::from(c.offset) == offset)?;
    Some((
        table.name_of(r.name)?.to_owned(),
        table.name_of(c.name)?.to_owned(),
    ))
}

#[test]
fn every_texture_sample_reads_through_a_named_sampler() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).unwrap();
    let mut world = World::new();
    source.add_shader_database(&mut world).unwrap();
    let (mut programs, mut samples, mut traced, mut direct) = (0usize, 0usize, 0usize, 0usize);
    let mut untraced: BTreeMap<String, usize> = BTreeMap::new();
    let mut not_named: Vec<String> = Vec::new();
    let mut undeclared: Vec<String> = Vec::new();
    let mut unread: BTreeSet<String> = BTreeSet::new();
    let mut names: BTreeMap<String, usize> = BTreeMap::new();
    let mut image_words: BTreeMap<((u32, u32), String), usize> = BTreeMap::new();
    let mut computed_images = 0usize;
    let mut seen = BTreeSet::new();
    let mut not_lifted: BTreeMap<String, usize> = BTreeMap::new();
    let (mut static_uses, mut static_held) = (0usize, 0usize);
    let mut static_missing: BTreeMap<String, usize> = BTreeMap::new();
    let mut max_lod: BTreeSet<u32> = BTreeSet::new();
    let mut biases: BTreeSet<u32> = BTreeSet::new();
    for (entity, effect) in world.iter::<GtaEffect>() {
        let Some(techniques) = world.get::<GtaEffectTechniques>(entity) else {
            continue;
        };
        for r in &techniques.samplers {
            max_lod.insert(r.max_lod.to_bits());
            biases.insert(r.mip_lod_bias.to_bits());
        }
        for program in effect.programs.iter().filter_map(|p| p.shader) {
            let Some(sh) = world.get::<Shader>(program) else {
                continue;
            };
            let Some(table) = world.get::<GtaShaderTable>(program).map(|t| &t.0) else {
                continue;
            };
            let found = match sampler_use::uses(&sh.bytecode) {
                Ok(f) => f,
                Err(why) => {
                    if seen.insert(program) {
                        *not_lifted.entry(why).or_default() += 1;
                    }
                    continue;
                }
            };
            // each program once for the counts; its static names against every effect that lists it
            let first = seen.insert(program);
            if first {
                programs += 1;
                samples += found.uses.len() + found.untraced.len();
                traced += found.uses.len();
                computed_images += found.computed_images;
                for u in &found.untraced {
                    *untraced.entry(u.clone()).or_default() += 1;
                }
            }
            for u in &found.uses {
                if first && let Some((b, o)) = u.image {
                    let named = word_name(table, b, o)
                        .map_or("(no constant)".to_owned(), |(b, c)| format!("{b}.{c}"));
                    *image_words.entry((u.texture, named)).or_default() += 1;
                }
                let (buffer, offset) = match u.through {
                    Through::Heap { buffer, offset } => (buffer, offset),
                    Through::Direct(at) => {
                        if first {
                            direct += 1;
                            let declared = sh.interface.resources.iter().any(|r| {
                                matches!(r.kind, ResourceKind::Sampler { .. })
                                    && r.provenance.d3d.is_some_and(|b| (b.space, b.register) == at)
                            });
                            if !declared {
                                undeclared.push(format!("{} {u:?}", effect.name));
                            }
                        }
                        continue;
                    }
                };
                let Some((buffer_name, constant)) = word_name(table, buffer, offset) else {
                    if first {
                        not_named.push(format!("{u:?}: no constant"));
                    }
                    continue;
                };
                if buffer_name != "g_rage_dynamicsamplerindices"
                    || !constant.starts_with("gDynamicSamplerIndex_")
                {
                    if first {
                        not_named.push(format!("{u:?}: {buffer_name} {constant}"));
                    }
                    continue;
                }
                if first {
                    *names.entry(constant.clone()).or_default() += 1;
                }
                if constant.contains("_sdi") {
                    match sampler_state::named(&constant) {
                        None => {
                            unread.insert(constant);
                        }
                        Some(state) => {
                            static_uses += 1;
                            if sampler_state::held_by(&state, &techniques.samplers) {
                                static_held += 1;
                            } else {
                                *static_missing.entry(constant).or_default() += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    println!(
        "{programs} programs: {samples} samples, {traced} traced, {direct} through a declared sampler, {computed_images} of a texture array indexed by a computed value; untraced {untraced:?}; not lifted {not_lifted:?}"
    );
    println!("sampler words by name: {names:?}");
    for ((texture, word), n) in &image_words {
        println!("IMAGE {texture:?} indexed by {word}: {n}");
    }
    println!(
        "static samplers: {static_uses} uses, {static_held} with a record of the stated state in the effect's table; without one {static_missing:?}"
    );
    println!(
        "maximum LODs of the records: {:?}; biases: {:?}",
        max_lod.iter().map(|b| f32::from_bits(*b)).collect::<Vec<_>>(),
        biases.iter().map(|b| f32::from_bits(*b)).collect::<Vec<_>>()
    );
    assert!(
        undeclared.is_empty(),
        "{:?}",
        &undeclared[..undeclared.len().min(5)]
    );
    assert!(not_named.is_empty(), "{:?}", &not_named[..not_named.len().min(5)]);
    assert!(
        unread.is_empty(),
        "static names the state function does not read: {unread:?}"
    );
    assert!(untraced.is_empty(), "{untraced:?}");
    assert!(samples > 5_000);
    // the table holds the stated state for most uses; the rest are listed above
    // (the anisotropic names, in the effects whose table has no record of that anisotropy, are the ones it lacks)
    assert!(
        static_held * 100 > static_uses * 85,
        "{static_held} of {static_uses}"
    );
    assert!(
        static_missing.keys().all(|n| n.contains("Anisotropic")),
        "{static_missing:?}"
    );
    assert_eq!(max_lod, BTreeSet::from([12.0f32.to_bits()]));
}
