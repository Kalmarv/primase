//! Installed initializer variants resolve their own protector parts and keep the associated lazy animation read.
mod common;
mod inputs;

use fromsoft_formats::{Game, Keys, dcx::Decoder};
use inputs::{Game as Data, Tool, install, keys, oodle};
use primase_core::convert::Reader;
use primase_fromsoft::sekiro::{CharacterOutfit, CharacterReader, OutfitVariants};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

// Setup-only migration: the program states the same presentation/level choices through the registered pass.
fn character_outfits(
    install: &fromsoft_formats::Install,
    archive: &fromsoft_formats::Archive,
    decoder: &Decoder<'_>,
) -> Result<Vec<CharacterOutfit>> {
    let mut world = primase_core::World::new();
    world.insert_resource(primase_fromsoft::sekiro::character_outfits(
        install, archive, decoder,
    )?);
    let mut registry = primase_core::pass::PassRegistry::new();
    primase_fromsoft::sekiro::passes::register(&mut registry);
    let pipeline = registry
        .parse("select-fromsoft-initializer-outfits{initializer=all,presentation=both,level=prefer-low}")?;
    primase_core::pass::PassManager::new(registry).run(
        &mut world,
        &pipeline,
        &mut primase_core::pass::RunReport::default(),
    )?;
    Ok(world
        .resource::<OutfitVariants>()
        .ok_or("no outfit variants")?
        .0
        .clone())
}

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

/// A player's outfit read without its animations listed still carries the character's skeleton as the assembly
/// reference of every part rig, so its parts assemble. The parts of Elden Ring's female starting outfit (initializer
/// 3002) each store the parents their own authoring gave (the head part parents `L_UpArmTwist` to `L_Clavicle`, the
/// legs part parents `L_Calf` to `Pelvis`); only the character's skeleton states the one hierarchy. The expected
/// counts are those of the installed read: the four parts `hd_m_1380_l`, `bd_f_1380_l`, `am_m_1380_l` and
/// `lg_f_1380_l` (the head and arms rows state a single `m` spelling, the body and legs rows are split by sex).
#[test]
fn an_outfit_read_without_animations_assembles_against_the_players_skeleton() -> Result<()> {
    use primase_core::pass::{MissingInputs, PassManager, PassRegistry, RunReport};
    use primase_core::{AssemblyReference, Asset, Name, Role, Source};
    use primase_fromsoft::sekiro::program::{CharacterPreparation, Materials};
    use primase_fromsoft::sekiro::standalone::{
        InventoryRequest, SelectedModelRequest, StandaloneAsset, read_inventory, read_selected_models,
    };

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
    let decoder = || Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    let mut registry = PassRegistry::new();
    primase_core::register(&mut registry);
    primase_animation::register(&mut registry);
    primase_fromsoft::sekiro::passes::register(&mut registry);
    let manager = PassManager::new(registry).with_missing_inputs(MissingInputs::Skip);

    // the candidates, and the registered selection of initializer 3002, presentation f
    let asset = StandaloneAsset::Character("c0000".to_owned());
    let mut candidates = read_inventory(
        &installed,
        decoder(),
        InventoryRequest {
            asset: &asset,
            names: None,
            npc: None,
            weapons: false,
            save: None,
            attack_of: None,
        },
    )
    .map_err(|error| error.to_string())?;
    let selection = manager.registry.parse(
        "select-fromsoft-initializer-outfits{initializer=3002,presentation=f,level=prefer-low},          select-fromsoft-npc-outfits{level=prefer-low},          select-fromsoft-character-read{outfit=initializer/3002/f,initializer=none},          select-fromsoft-dressing{npc=none,empty=show}",
    )?;
    manager.run(&mut candidates, &selection, &mut RunReport::default())?;

    // the read states no animations: none were asked for
    let read = read_selected_models(
        installed,
        decoder(),
        &candidates,
        SelectedModelRequest {
            asset,
            animations: None,
            weapons: Default::default(),
            grip: None,
            clip_effects: false,
        },
    )
    .map_err(|error| error.to_string())?;
    assert_eq!(
        read.models.dressed_parts,
        ["hd_m_1380_l", "bd_f_1380_l", "am_m_1380_l", "lg_f_1380_l"]
    );
    let parts = read.models.dressed_parts.len();
    let mut world = read.models.world;

    // every part rig references the player's animation skeleton
    let rigs: Vec<_> = world
        .iter::<Asset>()
        .filter(|(root, _)| {
            world
                .get::<Source>(*root)
                .is_some_and(|s| s.0.starts_with("parts/"))
        })
        .flat_map(|(_, asset)| {
            asset
                .members
                .iter()
                .filter(|(_, role)| *role == Role::Rig)
                .map(|(e, _)| *e)
        })
        .collect();
    assert_eq!(rigs.len(), parts);
    for rig in rigs {
        let reference = world
            .get::<AssemblyReference>(rig)
            .ok_or("a part rig without the assembly reference")?;
        assert_eq!(
            world.get::<Name>(reference.skeleton).map(|n| n.0.as_str()),
            Some("c0000 havok skeleton")
        );
    }

    // the example's lowering assembles them
    let texts = CharacterPreparation {
        game: Game::EldenRing,
        materials: Materials::Preserve,
        game_shaders: false,
        water_colour: [48, 98, 122, 190],
        parts,
        select_assets: Vec::new(),
        animations: None,
        dressing: true,
        entity_names: true,
        held: None,
        bake_cache: primase_fromsoft::sekiro::passes::bake_materials::Cache::Off,
    }
    .texts();
    let mut report = RunReport::default();
    // texture baking is not what assembly depends on, and decoding the textures of four parts takes minutes
    let lowering: Vec<_> = texts
        .iter()
        .filter(|text| !text.starts_with("bake-fromsoft-materials"))
        .map(String::as_str)
        .collect();
    manager.run(
        &mut world,
        &manager.registry.parse(&lowering.join(", "))?,
        &mut report,
    )?;
    let count = |pass: &str, key: &str| -> Result<u64> {
        let step = report
            .steps
            .iter()
            .find(|step| step.pass.starts_with(pass))
            .ok_or_else(|| format!("{pass} did not run"))?;
        step.log
            .counts
            .get(key)
            .copied()
            .ok_or_else(|| format!("{pass} counted no {key:?}").into())
    };
    assert_eq!(count("assemble-character", "assembled joints")?, 238);
    assert_eq!(
        count(
            "assemble-character",
            "part joints reparented to the reference within reference-tolerance"
        )?,
        14
    );
    assert_eq!(count("assemble-character", "parts assembled")?, 4);
    assert_eq!(count("place-assets", "parts placed")?, 4);
    Ok(())
}

/// A Sekiro NPC's outfit parts are authored as one set of the character's own, and the set agrees on every joint's
/// parent: `assemble-character` takes them with no assembly reference, so the NPC read attaches none (only a player's
/// outfit, whose parts come from different sets, takes its character's skeleton as the reference). Observed on the
/// installed c0000 map-enemy initializers 15000, 15010, 15020 and 15030 (3 or 4 parts each): every one assembles
/// all its parts.
#[test]
fn an_installed_npc_outfit_of_several_parts_assembles_without_a_reference() -> Result<()> {
    use primase_core::pass::{MissingInputs, PassManager, PassRegistry, RunReport};
    use primase_core::{AssemblyReference, Asset, Role};
    use primase_fromsoft::sekiro::outfit_data::NpcOutfitVariants;
    use primase_fromsoft::sekiro::passes::bake_materials::Cache;
    use primase_fromsoft::sekiro::program::{CharacterPreparation, Materials};
    use primase_fromsoft::sekiro::standalone::{
        InventoryRequest, SelectedModelRequest, StandaloneAsset, read_inventory, read_selected_models,
    };
    let game = Game::Sekiro;
    let (Some(library), Some(root), Some(folder), Some(names)) = (
        oodle(Tool::Oodle6),
        install(Data::Sekiro),
        keys(Data::Sekiro),
        inputs::oracle(inputs::OracleSet::Uxm, Data::Sekiro, "dictionary.txt"),
    ) else {
        return Ok(());
    };
    let codec = common::oodle(library)?;
    let decoder = || Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    let installed = common::install(root, game, Keys::from_folder(folder, game, None)?);
    let mut registry = PassRegistry::new();
    primase_core::register(&mut registry);
    primase_animation::register(&mut registry);
    primase_fromsoft::sekiro::passes::register(&mut registry);
    let manager = PassManager::new(registry).with_missing_inputs(MissingInputs::Skip);
    let asset = StandaloneAsset::Character("c0000".to_owned());
    let mut candidates = read_inventory(
        &installed,
        decoder(),
        InventoryRequest {
            asset: &asset,
            names: Some(names.as_ref()),
            npc: Some(0),
            weapons: false,
            save: None,
            attack_of: None,
        },
    )
    .map_err(|error| error.to_string())?;
    let select = manager.registry.parse(
        "select-fromsoft-initializer-outfits{initializer=all,presentation=authored,level=prefer-low}, select-fromsoft-npc-outfits{level=prefer-low}",
    )?;
    manager.run(&mut candidates, &select, &mut RunReport::default())?;
    let mut variants: Vec<_> = candidates
        .resource::<NpcOutfitVariants>()
        .ok_or("no NPC outfit variants")?
        .0
        .iter()
        .filter(|variant| variant.parts.len() >= 2 && variant.unresolved.is_empty())
        .filter_map(|variant| Some((variant.init?, variant.parts.clone())))
        .collect();
    variants.sort();
    assert_eq!(
        variants
            .iter()
            .map(|(init, parts)| (*init, parts.len()))
            .collect::<Vec<_>>(),
        [(15000, 4), (15010, 4), (15020, 4), (15030, 3)]
    );
    for (init, parts) in variants {
        let mut candidates = candidates.clone();
        let choose = manager.registry.parse(&format!(
            "select-fromsoft-character-read{{outfit=none,initializer={init}}}"
        ))?;
        manager.run(&mut candidates, &choose, &mut RunReport::default())?;
        let read = read_selected_models(
            installed.clone(),
            decoder(),
            &candidates,
            SelectedModelRequest {
                asset: asset.clone(),
                animations: None,
                weapons: Default::default(),
                grip: None,
                clip_effects: false,
            },
        )
        .map_err(|error| error.to_string())?;
        assert_eq!(read.models.dressed_parts, parts, "initializer {init}");
        let mut world = read.models.world;
        for (_, asset) in world.iter::<Asset>() {
            for (entity, role) in &asset.members {
                assert!(
                    *role != Role::Rig || world.get::<AssemblyReference>(*entity).is_none(),
                    "initializer {init}: an NPC part rig carries an assembly reference"
                );
            }
        }
        let texts = CharacterPreparation {
            game,
            materials: Materials::Preserve,
            game_shaders: false,
            water_colour: [48, 98, 122, 190],
            parts: parts.len(),
            select_assets: Vec::new(),
            animations: None,
            dressing: false,
            entity_names: false,
            held: None,
            bake_cache: Cache::Off,
        }
        .texts();
        let mut report = RunReport::default();
        // texture baking is not what assembly depends on, and decoding the textures of four parts takes minutes
        let lowering: Vec<_> = texts
            .iter()
            .filter(|text| !text.starts_with("bake-fromsoft-materials"))
            .map(String::as_str)
            .collect();
        manager.run(
            &mut world,
            &manager.registry.parse(&lowering.join(", "))?,
            &mut report,
        )?;
        let assembled = report
            .steps
            .iter()
            .find(|step| step.pass.starts_with("assemble-character"))
            .and_then(|step| step.log.counts.get("parts assembled").copied());
        assert_eq!(assembled, Some(parts.len() as u64), "initializer {init}");
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

    let appearance_notice = "player body and face not drawn (character-creator meshes not in scope)";

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
                // The no-protector initializer names the omitted character-creator meshes. It must not hide
                // a failure of any clothed class or missing protector.
                if row != 3009 || !outfit.parts.is_empty() || outfit.unresolved != [appearance_notice] {
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
                let notices = reader
                    .notices()
                    .into_iter()
                    .map(|notice| notice.to_string())
                    .collect::<Vec<_>>();
                if !notices.iter().any(|notice| notice.contains(appearance_notice)) {
                    return Err("outfit reader did not disclose its missing player body and face".into());
                }
                result["appearance_notice"] = appearance_notice.into();
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
