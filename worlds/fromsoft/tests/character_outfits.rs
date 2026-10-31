//! Installed initializer variants resolve their own protector parts and keep the associated lazy animation read.
mod common;
mod inputs;

use fromsoft_formats::{Game, Keys, dcx::Decoder};
use inputs::{Game as Data, Tool, install, keys, oodle};
use primase_core::convert::Reader;
use primase_fromsoft::sekiro::{CharacterReader, character_outfits};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn installed_initializer_catalogs_and_the_sekiro_outfit_reader_keep_authored_parts() -> Result<()> {
    for (game, data) in [(Game::Sekiro, Data::Sekiro), (Game::EldenRing, Data::Elden)] {
        let (Some(root), Some(folder), Some(library)) = (install(data), keys(data), oodle(Tool::Oodle6))
        else {
            continue;
        };
        let codec = common::oodle(library)?;
        let decoder = Decoder {
            oodle: Some(&codec),
            max_output: 1 << 30,
        };
        let installed = common::install(root, game, Keys::from_folder(folder, game, None)?);
        let archive = installed.open()?;
        let outfits = character_outfits(&installed, &archive, &decoder)?;
        assert!(!outfits.is_empty());
        let readable: Vec<_> = outfits.iter().filter(|outfit| outfit.readable()).collect();
        assert!(!readable.is_empty(), "{game:?}: no readable initializer outfit");
        for outfit in &readable {
            assert_eq!(outfit.animations_of, "c0000");
            for part in &outfit.parts {
                assert!(archive.contains(&format!("/parts/{part}.partsbnd.dcx")));
            }
        }
        println!(
            "{game:?}: {} initializer variants, {} readable",
            outfits.len(),
            readable.len()
        );
        if game == Game::Sekiro {
            let outfit = readable
                .iter()
                .find(|outfit| outfit.initializer == 9000)
                .ok_or("no initializer 9000")?;
            assert_eq!(
                outfit.parts,
                ["fc_m_0200_l", "bd_m_9040_l", "am_m_9000_l", "lg_m_9000_l"]
            );
            let mut reader = CharacterReader::outfit(installed, outfit, decoder)?;
            let (world, _) = reader.read().map_err(|error| error.to_string())?;
            assert!(world.iter::<primase_havok::cloth::HavokCloth>().count() >= 2);
            let notices = reader
                .notices()
                .into_iter()
                .map(|notice| notice.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(notices.contains(
                "bind matrices: installed (approximate; runtime bind initialization not reproduced)"
            ));
            assert!(notices.contains("wind off: origin not traced"));
            assert!(
                world
                    .iter::<primase_fromsoft::sekiro::ClipSource>()
                    .next()
                    .is_some()
            );
            assert!(
                world.iter::<primase_core::Clip>().next().is_none(),
                "tracks stay lazy until selected"
            );
        }
    }
    Ok(())
}

/// The animation hierarchy resolves armor's flattened parent copies without discarding its part-only cloth joints.
/// The already working Wolf is included so the shared reader keeps the Sekiro path as well.
#[test]
fn warrior_confessor_and_wolf_assemble_against_their_authored_animation_hierarchy() -> Result<()> {
    use primase_animation::simulation::Simulation;
    use primase_core::pass::{PassManager, PassRegistry, RunReport};
    use primase_core::{AssemblyReference, Asset, Role, Skeleton};
    for (game, data, rows) in [
        (Game::EldenRing, Data::Elden, &[3001, 3008][..]),
        (Game::Sekiro, Data::Sekiro, &[9000][..]),
    ] {
        let (Some(root), Some(folder), Some(library)) = (install(data), keys(data), oodle(Tool::Oodle6))
        else {
            continue;
        };
        let codec = common::oodle(library)?;
        let installed = common::install(root, game, Keys::from_folder(folder, game, None)?);
        let archive = installed.open()?;
        let decoder = Decoder {
            oodle: Some(&codec),
            max_output: 1 << 30,
        };
        let outfits = character_outfits(&installed, &archive, &decoder)?;
        for &row in rows {
            let outfit = outfits
                .iter()
                .find(|o| o.initializer == row && o.key().ends_with("/m") && o.readable())
                .ok_or("no selected initializer outfit")?;
            let mut reader = CharacterReader::outfit(
                installed.clone(),
                outfit,
                Decoder {
                    oodle: Some(&codec),
                    max_output: 1 << 30,
                },
            )?;
            let (mut world, _) = reader.read().map_err(|error| error.to_string())?;
            assert!(world.iter::<AssemblyReference>().count() >= outfit.parts.len());
            assert!(
                world.iter::<Simulation>().count() >= 2,
                "{game:?}/{row}: expected multiple cloth simulations"
            );
            let mut registry = PassRegistry::new();
            primase_fromsoft::sekiro::passes::register(&mut registry);
            primase_animation::register(&mut registry);
            let manager = PassManager::new(registry);
            let pipeline = "lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program},assemble-character{correspondence=by-name,reference-pose=assembled,tolerance=0.001,reference-tolerance=0.0001,unweighted-rests=report,weighted-rests=report,name=player}";
            let mut report = RunReport::default();
            manager.run(&mut world, &manager.registry.parse(pipeline)?, &mut report)?;
            let assembly = &report.steps.last().ok_or("no assembly step")?.log;
            assert_eq!(assembly.counts["source assembly reference skeletons"], 1);
            assert!(assembly.counts["unweighted detached roots: rest from reference"] > 0);
            if game == Game::EldenRing {
                assert!(
                    assembly.counts["part joints reparented to the reference within reference-tolerance"]
                        >= 4
                );
                let (_, asset) = world
                    .iter::<Asset>()
                    .find(|(_, a)| a.members.iter().any(|(_, r)| *r == Role::Rig))
                    .ok_or("no assembled asset")?;
                let rig = asset
                    .members
                    .iter()
                    .find(|(_, r)| *r == Role::Rig)
                    .ok_or("no assembled rig")?
                    .0;
                let skeleton = world.get::<Skeleton>(rig).ok_or("no assembled skeleton")?;
                for name in ["L_ThighTwist", "R_ThighTwist"] {
                    let joint = skeleton
                        .joints
                        .iter()
                        .find(|j| j.name == name)
                        .ok_or("no thigh twist")?;
                    let parent = skeleton
                        .index_of(joint.parent.ok_or("twist is a root")?)
                        .ok_or("missing twist parent")?;
                    assert_eq!(skeleton.joints[parent].name, name.replace("ThighTwist", "Thigh"));
                }
                if row == 3008 {
                    assert!(
                        assembly
                            .approximations
                            .keys()
                            .any(|line| line.contains("same-parent rests") && line.contains("1.475e-2")),
                        "{:?}",
                        assembly.approximations
                    );
                }
            }
            assert!(world.iter::<Simulation>().count() >= 2);
            assert!(
                world.verify_components().is_empty(),
                "{:?}",
                world.verify_components()
            );
            println!(
                "{game:?}/{row}: {} part rigs, {} reparented, {} unweighted detached roots, {} simulations",
                outfit.parts.len(),
                assembly.counts["part joints reparented to the reference within reference-tolerance"],
                assembly.counts["unweighted detached roots: rest from reference"],
                world.iter::<Simulation>().count()
            );
        }
    }
    Ok(())
}

/// Every starting-class presentation uses the browser's outfit reader, geometry lowering and guarded assembly.
/// No renderer, material baking, game process or native skinning-parity claim is involved.
#[test]
fn all_elden_starting_classes_bind_cloth_against_their_assembled_hierarchy() -> Result<()> {
    use primase_animation::simulation::{Simulation, SimulationVertices};
    use primase_core::pass::{PassManager, PassRegistry, RunReport};
    use primase_core::{Asset, JointMap, Role, Skeleton};
    use std::collections::BTreeMap;

    let (Some(root), Some(folder), Some(library)) =
        (install(Data::Elden), keys(Data::Elden), oodle(Tool::Oodle6))
    else {
        return Ok(());
    };
    let codec = common::oodle(library)?;
    let installed = common::install(
        root,
        Game::EldenRing,
        Keys::from_folder(folder, Game::EldenRing, None)?,
    );
    let archive = installed.open()?;
    let outfits = character_outfits(
        &installed,
        &archive,
        &Decoder {
            oodle: Some(&codec),
            max_output: 1 << 30,
        },
    )?;
    let mut failures = Vec::new();
    for row in 3000..=3009 {
        let variants: Vec<_> = outfits
            .iter()
            .filter(|outfit| outfit.initializer == row)
            .collect();
        assert!(
            !variants.is_empty(),
            "starting initializer {row} absent from installed catalog"
        );
        for gender in ["m", "f"] {
            let key = format!("initializer/{row}/{gender}");
            let selected = variants.iter().find(|outfit| outfit.key() == key).copied();
            // The catalog coalesces presentations only when their parts and unresolved reasons are identical.
            let outfit = selected.unwrap_or(variants[0]);
            let mut result = serde_json::json!({
                "initializer": row, "gender": gender, "key": key,
                "catalog_alias": selected.is_none().then(|| outfit.key()),
                "parts": outfit.parts, "status": "catalog-refused",
                "reason": outfit.unresolved, "cloth_runners": 0,
                "assembly_counts": {}, "assembly_approximations": {},
            });
            if !outfit.readable() {
                if outfit.unresolved.is_empty() {
                    result["reason"] = serde_json::json!(["initializer resolves no drawable parts"]);
                }
                // The no-protector initializer is retained in the census while its base-body producer is
                // investigated separately. It must not hide a failure of any clothed class or missing protector.
                if row != 3009 || !outfit.parts.is_empty() || !outfit.unresolved.is_empty() {
                    failures.push(format!("{key}: {}", result["reason"]));
                }
                println!("STARTING_CLASS {}", serde_json::to_string(&result)?);
                continue;
            }
            let attempt = (|| -> std::result::Result<(), String> {
                let mut reader = CharacterReader::outfit(
                    installed.clone(),
                    outfit,
                    Decoder {
                        oodle: Some(&codec),
                        max_output: 1 << 30,
                    },
                )
                .map_err(|e| e.to_string())?;
                let (mut world, _) = reader.read().map_err(|e| e.to_string())?;
                result["status"] = "assembly-refused".into();
                let mut registry = PassRegistry::new();
                primase_fromsoft::sekiro::passes::register(&mut registry);
                primase_animation::register(&mut registry);
                let manager = PassManager::new(registry);
                let pipeline = manager.registry.parse("lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program},assemble-character{correspondence=by-name,reference-pose=assembled,tolerance=0.001,reference-tolerance=0.0001,unweighted-rests=report,weighted-rests=report,name=player}").map_err(|e| e.to_string())?;
                let mut report = RunReport::default();
                let assembled = manager.run(&mut world, &pipeline, &mut report);
                if let Some(step) = report.steps.last() {
                    result["assembly_counts"] =
                        serde_json::to_value(&step.log.counts).map_err(|e| e.to_string())?;
                    result["assembly_approximations"] =
                        serde_json::to_value(&step.log.approximations).map_err(|e| e.to_string())?;
                }
                assembled.map_err(|e| e.to_string())?;
                let rig = world
                    .iter::<Asset>()
                    .find_map(|(_, asset)| {
                        asset
                            .members
                            .iter()
                            .find_map(|(entity, role)| (*role == Role::Rig).then_some(*entity))
                    })
                    .ok_or("assembled asset has no rig")?;
                let skeleton = world
                    .get::<Skeleton>(rig)
                    .ok_or("assembled rig has no skeleton")?;
                result["status"] = "cloth-binding-refused".into();
                let mut runners = Vec::new();
                for (holder, declaration) in world.iter::<Simulation>() {
                    let mapped = world
                        .iter::<JointMap>()
                        .find(|(_, map)| map.from == declaration.skeleton && map.to == rig)
                        .map(|(_, map)| map.pairs.iter().copied().collect::<BTreeMap<_, _>>());
                    if declaration.skeleton != rig && mapped.is_none() {
                        return Err(format!(
                            "{}: no part-to-assembled joint map",
                            declaration.source.name()
                        ));
                    }
                    let runner = declaration.source.start(skeleton, mapped.as_ref())?;
                    let bound = world
                        .iter::<SimulationVertices>()
                        .filter(|(_, vertices)| vertices.simulation == holder)
                        .map(|(_, vertices)| vertices.primitives.iter().flatten().count())
                        .sum::<usize>();
                    if !declaration.outputs.is_empty() && bound == 0 {
                        return Err(format!(
                            "{}: outputs have no bound mesh primitives",
                            declaration.source.name()
                        ));
                    }
                    runners.push(serde_json::json!({"name": declaration.source.name(), "outputs": declaration.outputs.len(), "bound_primitives": bound, "driven_joints": runner.driven().len()}));
                }
                let errors = world.verify_components();
                if !errors.is_empty() {
                    return Err(format!("component verification: {errors:?}"));
                }
                result["cloth_runners"] = runners.len().into();
                result["cloth"] = runners.into();
                result["status"] = "assembled".into();
                result["reason"] = serde_json::Value::Null;
                Ok(())
            })();
            if let Err(reason) = attempt {
                failures.push(format!("{key}: {reason}"));
                result["reason"] = reason.into();
            }
            println!("STARTING_CLASS {}", serde_json::to_string(&result)?);
        }
    }
    assert!(
        failures.is_empty(),
        "starting-class refusals:\n{}",
        failures.join("\n")
    );
    Ok(())
}
