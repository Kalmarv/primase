//! The table records of the textures a program binds: a texture shares its register and space with other records (the
//! sampler heap's, a constant buffer's), and the tag tells them apart. Every texture of every vertex and pixel program
//! has exactly one record with a texture tag (`resolve-gta-shaders` takes it) but a debug program; the other records at its
//! bind point are counted by tag.
//!
//! Needs `installs.gta` and `keys.gta`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use std::collections::{BTreeMap, BTreeSet};

use iridia_interface::ResourceKind;
use primase_core::World;
use primase_core::shader::Shader;
use primase_gta::components::{GtaEffect, GtaShaderTable};
use primase_gta::read::source::GtaSource;
use primase_testdata::{Game, install, keys};

/// The tags `resolve-gta-shaders` takes for a texture.
const TEXTURE_TAGS: [u16; 4] = [258, 322, 514, 770];

#[test]
fn a_texture_has_one_record_of_a_texture_tag() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).unwrap();
    let mut world = World::new();
    source.add_shader_database(&mut world).unwrap();
    let mut seen = BTreeSet::new();
    let (mut textures, mut one) = (0usize, 0usize);
    let mut wrong: Vec<String> = Vec::new();
    let mut others: BTreeMap<u16, usize> = BTreeMap::new();
    for (_, effect) in world.iter::<GtaEffect>() {
        for program in &effect.programs {
            let Some(e) = program.shader else { continue };
            // the vertex and pixel programs hold every resource in their table
            if !seen.insert(e) || !(program.name.starts_with("VS_") || program.name.starts_with("PS_")) {
                continue;
            }
            let (Some(sh), Some(table)) = (
                world.get::<Shader>(e),
                world.get::<GtaShaderTable>(e).map(|t| &t.0),
            ) else {
                continue;
            };
            for r in &sh.interface.resources {
                let (ResourceKind::Texture { .. }, Some(bind)) = (&r.kind, r.provenance.d3d) else {
                    continue;
                };
                textures += 1;
                let here: Vec<_> = table
                    .resources
                    .iter()
                    .filter(|t| u32::from(t.register) == bind.register && u32::from(t.space) == bind.space)
                    .collect();
                let own = here.iter().filter(|t| TEXTURE_TAGS.contains(&t.tag)).count();
                if own == 1 {
                    one += 1;
                } else {
                    wrong.push(format!(
                        "{}: {own} of {:?}",
                        program.name,
                        here.iter().map(|t| t.tag).collect::<Vec<_>>()
                    ));
                }
                for t in here.iter().filter(|t| !TEXTURE_TAGS.contains(&t.tag)) {
                    *others.entry(t.tag).or_default() += 1;
                }
            }
        }
    }
    println!(
        "{textures} textures, {one} with one texture-tagged record; the other records at their bind points by tag: {others:?}"
    );
    assert!(textures > 10_000);
    // the one exception is a debug program of the ray-tracing structure (`PS_DebugBvh`), which has no record at all
    assert!(
        wrong
            .iter()
            .all(|w| w.starts_with("PS_DebugBvh") && w.ends_with("0 of []")),
        "{} of {textures}: {:?}",
        wrong.len(),
        &wrong[..wrong.len().min(8)]
    );
    assert!(wrong.len() <= 1, "{wrong:?}");
}
