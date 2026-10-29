//! Original MGFX execution is the oracle for every resolved material's IR specialisation.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod inputs;
use inputs::{Game, install};
use iridia_exec::probe::AnsweredReads;
use iridia_exec::{ImageSample, PixelShader, ProgramShader};
use iridia_interface::{D3dClass, ResourceKind, Stage};
use primase_core::pass::{Pass, PassLog};
use primase_core::shader::{LiftedShader, ShaderBinding};
use primase_core::{Name, World};
use primase_stuntboost::passes::{Baked, LowerStuntboostMaterials, ResolveStuntboostShaders, Transparent};
use primase_stuntboost::{levels, read_level};
// The GLSL source has no D3D signature. The executor's register API needs explicit
// test input/output rows, assigned in interface order on both copies, without changing IR.
fn execution(mut program: iridia::Program) -> ProgramShader {
    for (output, varyings) in [
        (false, &mut program.interface.inputs),
        (true, &mut program.interface.outputs),
    ] {
        for (index, varying) in varyings.iter_mut().enumerate() {
            let mask = (1u8 << varying.value.width.min(4)) - 1;
            varying.provenance.d3d = Some(iridia_interface::D3dSignature {
                register: index as u32,
                mask,
                read_write_mask: if output { 0 } else { mask },
            });
        }
    }
    ProgramShader::new(program).unwrap()
}
#[test]
fn every_resolved_level_material_matches_original_execution_on_sampled_inputs() {
    let Some(root) = install(Game::Stuntboost) else {
        return;
    };
    let mut materials = 0;
    let mut executions = 0;
    let mut before = std::collections::BTreeSet::new();
    let mut after = std::collections::BTreeSet::new();
    let mut before_all = std::collections::BTreeSet::new();
    let mut after_all = std::collections::BTreeSet::new();
    let mut refused = std::collections::BTreeMap::<String, usize>::new();
    for level in levels(&root).unwrap() {
        let mut world = World::new();
        read_level(&root, &level.name, 0.01, &mut world).unwrap();
        LowerStuntboostMaterials {
            baked: Baked::BaseColour,
            transparent: Transparent::Blend,
        }
        .run(&mut world, &mut PassLog::default())
        .unwrap();
        ResolveStuntboostShaders::default()
            .run(&mut world, &mut PassLog::default())
            .unwrap();
        for (_, lift) in world.iter::<LiftedShader>() {
            before_all.insert(format!("{:?}", lift.0.module));
        }
        for (_, set) in world.iter::<primase_core::shader::ShaderSpecialisations>() {
            for stage in &set.0 {
                if let Some(program) = &stage.program {
                    after_all.insert(format!("{:?}", program.module));
                }
            }
        }
        for (entity, facts) in world.iter::<primase_core::shader::MaterialPrograms>() {
            for fact in &facts.0 {
                if let Some(why) = &fact.not_lifted {
                    println!(
                        "REFUSED {} material {:?} ({}): {why}",
                        level.name,
                        world
                            .get::<Name>(entity)
                            .map(|n| n.0.as_str())
                            .unwrap_or("unnamed"),
                        entity.index()
                    );
                }
            }
        }
        for (entity, binding) in world.iter::<ShaderBinding>() {
            let name = world
                .get::<Name>(entity)
                .map(|n| n.0.as_str())
                .unwrap_or("unnamed");
            let stage = binding.stage(Stage::Pixel).unwrap();
            let source = &world.get::<LiftedShader>(stage.shader).unwrap().0;
            let known = stage.known_words();
            before.insert(format!("{:?}", source.module));
            let result = match iridia::specialise::specialise(source, &known) {
                Ok(r) => r,
                Err(why) => {
                    *refused.entry(why.clone()).or_default() += 1;
                    println!(
                        "REFUSED {} material {name:?} ({}): {why}",
                        level.name,
                        entity.index()
                    );
                    continue;
                }
            };
            after.insert(format!("{:?}", result.program.module));
            let dropped = result
                .unread
                .iter()
                .map(|id| source.interface.resource(*id).unwrap().provenance.name.clone())
                .collect::<Vec<_>>();
            println!(
                "{} material {name:?} ({}): stated {:?}; feature words {:?}; removed {:?}; folded {}",
                level.name,
                entity.index(),
                known,
                result.words,
                dropped,
                result.folded
            );
            let original = execution((**source).clone());
            let specialised = execution(result.program);
            for i in 0..32usize {
                let inputs = (0..16)
                    .map(|r| std::array::from_fn(|c| (((r + c + i) % 29) as f32 / 29. + 0.05).to_bits()))
                    .collect::<Vec<[u32; 4]>>();
                let derivatives = vec![[[0.01f32.to_bits(), 0, 0, 0], [0, 0.01f32.to_bits(), 0, 0]]; 16];
                let mut buffers = Vec::new();
                for resource in &source.interface.resources {
                    if let ResourceKind::ConstantBuffer { layout } = &resource.kind {
                        let slot = resource
                            .provenance
                            .d3d
                            .filter(|d| d.class == D3dClass::ConstantBuffer)
                            .unwrap()
                            .register;
                        let mut rows = vec![[0.25f32.to_bits(); 4]; layout.size.div_ceil(16) as usize];
                        for (&(id, offset), &word) in &known {
                            if id == resource.id {
                                rows[offset as usize / 16][offset as usize / 4 % 4] = word;
                            }
                        }
                        buffers.push((slot, rows));
                    }
                }
                let buffers = buffers
                    .iter()
                    .map(|(slot, rows)| (*slot, rows.as_slice()))
                    .collect::<Vec<_>>();
                let sample = |r: &ImageSample<u32>| {
                    Ok([0.1 + ((r.texture as usize + i) % 7) as f32 * 0.1, 0.2, 0.3, 1.])
                };
                let a = original
                    .execute_with_derivatives(
                        &inputs,
                        &derivatives,
                        &buffers,
                        &mut AnsweredReads::new(sample),
                    )
                    .unwrap();
                let b = specialised
                    .execute_with_derivatives(
                        &inputs,
                        &derivatives,
                        &buffers,
                        &mut AnsweredReads::new(sample),
                    )
                    .unwrap();
                assert_eq!(
                    a.outputs,
                    b.outputs,
                    "{} material {} input {i}",
                    level.name,
                    entity.index()
                );
                assert_eq!(a.discarded, b.discarded);
                executions += 1;
            }
            materials += 1;
        }
    }
    println!(
        "{materials} materials matched; {executions} original/specialised execution pairs; refused {refused:?}"
    );
    println!(
        "distinct pixel programs: {} exact, {} specialised",
        before.len(),
        after.len()
    );
    println!(
        "distinct programs across all stages: {} exact, {} successful specialisations",
        before_all.len(),
        after_all.len()
    );
    assert!(materials > 0, "no material was compared");
    assert!(
        refused.is_empty(),
        "every material bound by the resolver must be specialised or explicitly accounted for"
    );
}
