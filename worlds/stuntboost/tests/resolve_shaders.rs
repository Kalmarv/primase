//! Every listed level is resolved; expectations come from its stored glTF and MGFX tables.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod inputs;
use inputs::{Game, install};
use primase_core::pass::{Pass, PassLog};
use primase_core::shader::{ConstantRole, LiftedShader, MaterialPrograms, Shader, ShaderBinding};
use primase_core::{Component, World};
use primase_stuntboost::components::StuntboostMaterial;
use primase_stuntboost::passes::ResolveStuntboostShaders;
use primase_stuntboost::{levels, read_level};
#[test]
fn every_level_accounts_for_every_material_and_retains_the_exact_program() {
    let Some(root) = install(Game::Stuntboost) else {
        return;
    };
    let listed = levels(&root).unwrap();
    let mut totals = std::collections::BTreeMap::<String, u64>::new();
    for level in &listed {
        let mut world = World::new();
        read_level(&root, &level.name, 0.01, &mut world).unwrap();
        let mut log = PassLog::default();
        ResolveStuntboostShaders::default()
            .run(&mut world, &mut log)
            .unwrap();
        let mut bound = 0;
        for (entity, material) in world.iter::<StuntboostMaterial>() {
            let facts = world
                .get::<MaterialPrograms>(entity)
                .expect("each material has program facts or a counted refusal");
            if let Some(binding) = world.get::<ShaderBinding>(entity) {
                bound += 1;
                assert_eq!(binding.stages.len(), 2);
                for stage in &binding.stages {
                    let original = world.get::<Shader>(stage.shader).unwrap();
                    let lifted = world.get::<LiftedShader>(stage.shader).unwrap();
                    assert_eq!(original.interface, lifted.0.interface);
                    lifted.verify(stage.shader, &world).unwrap();
                    for constant in &stage.constants {
                        if constant.name.as_deref() == Some("Color") {
                            assert_eq!(
                                constant.role,
                                ConstantRole::Material(material.base_color.map(f32::to_bits).to_vec())
                            );
                        }
                    }
                }
            } else {
                assert!(facts.0.iter().all(|f| f.not_lifted.is_some()));
            }
        }
        println!(
            "{}: {bound}/{} source materials bound; {:?}",
            level.name,
            world.iter::<StuntboostMaterial>().count(),
            log.omitted
        );
        for (reason, n) in log.omitted {
            *totals.entry(reason).or_default() += n;
        }
    }
    println!("{} levels; refusals {totals:?}", listed.len());
    assert!(!listed.is_empty());
}
