//! `resolve-anvil-shaders` on a sample of the Materials of the six forges (Riders Republic build 25394987): up to
//! `PER_TEMPLATE` Materials of every MaterialTemplate, their textures read, each bound Material's `core.shader_binding`
//! verified (`Component::verify`: its shaders, resources, lanes and offsets are the programs'), and the Materials
//! bound and left unbound counted, with the reason, for both vertex-layout choices. Prints the counts.
use std::collections::BTreeMap;

use primase_anvil::components::AnvilMaterial;
use primase_anvil::passes::ResolveAnvilShaders;
use primase_anvil::read::archive::Archive;
use primase_anvil::read::material::{MATERIAL, StoredMaterial};
use primase_anvil::read::placements::{InstancedLevels, Placed, Placer};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::shader::ShaderBinding;
use primase_core::{Component, Entity, World};
use primase_testdata::real_data;

const PER_TEMPLATE: usize = 4;

fn options(layout: &str) -> Options {
    Options::from([
        ("unset-textures".to_owned(), "white".to_owned()),
        ("vertex-layout".to_owned(), layout.to_owned()),
    ])
}

#[test]
fn a_sample_of_every_template_is_bound_or_left_with_a_reason() {
    let Some(dir) = real_data("PRIMASE_ANVIL_DIR") else {
        return;
    };
    let order = [
        "DataPC_MAD_WhiteRoom_patch_01.forge",
        "DataPC_MAD_WhiteRoom.forge",
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_patch_01.forge",
        "DataPC.forge",
    ];
    let mut archive = Archive::open(&order.map(|f| dir.join(f))).expect("archive");
    // Material ids by their template's name, a few each
    let mut by_template: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for f in 0..order.len() {
        let forge = archive.forge(f).expect("forge");
        let entries: Vec<(usize, u64)> = (0..forge.names.len())
            .filter(|&k| forge.names[k].type_id == MATERIAL)
            .map(|k| (k, forge.index[k].id))
            .collect();
        for (entry, id) in entries {
            let body = archive
                .resource(primase_anvil::read::archive::At { forge: f, entry })
                .expect("material")
                .body;
            let Ok(m) = StoredMaterial::parse(&body) else {
                continue;
            };
            let Some(t) = m.references[0].and_then(|t| archive.find(t.id)) else {
                continue;
            };
            let name = archive.forge(t.forge).expect("forge").names[t.entry].name.clone();
            let ids = by_template.entry(name).or_default();
            if ids.len() < PER_TEMPLATE && !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    let ids: Vec<u64> = by_template.values().flatten().copied().collect();
    println!("{} templates, {} Materials sampled", by_template.len(), ids.len());
    let mut w = World::new();
    let mut placed = Placed::default();
    let materials: Vec<Entity> = {
        let mut placer =
            Placer::new(&mut archive, &mut w, SourceFrame::WORLD, InstancedLevels::Slot).with_shaders();
        ids.iter()
            .filter_map(|&id| placer.material(id, &mut placed))
            .collect()
    };
    println!(
        "read {} Materials ({} refused)",
        materials.len(),
        placed.refused.len()
    );
    for layout in ["none", "2"] {
        let mut world = w.clone();
        let mut log = PassLog::default();
        ResolveAnvilShaders::from_options(&options(layout))
            .expect("options")
            .run(&mut world, &mut log)
            .expect("run");
        let mut bound = 0;
        let mut stages = BTreeMap::new();
        for &e in &materials {
            if let Some(b) = world.get::<ShaderBinding>(e) {
                b.verify(e, &world).unwrap_or_else(|why| {
                    panic!(
                        "{:?}: {why}",
                        world
                            .get::<AnvilMaterial>(e)
                            .and_then(|m| m.template_name.clone())
                    )
                });
                bound += 1;
                *stages.entry(b.stages.len()).or_insert(0usize) += 1;
            }
        }
        println!("vertex-layout={layout}: {bound} bound, by stage count {stages:?}");
        for (what, n) in &log.counts {
            println!("  {n:>6}  {what}");
        }
        for (what, n) in &log.approximations {
            println!("  {n:>6}  (approximation) {what}");
        }
        assert!(bound > 0);
    }
}
