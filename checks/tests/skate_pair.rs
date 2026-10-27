//! The existing Sekiro soldier and skate idle, through the same table in both directions, and a skin fit census.
mod inputs;

use inputs::{Game, OracleSet, Tool, install, install_file, oodle, oracle, tool};

use fromsoft_formats::{Install, NameDictionary, dcx::Decoder};
use frostbite_formats::folder::{Folder, Source};
use primase_animation::{
    Fit, Placement, Reference, RetargetClip, RetargetSkin, Translation, UnmappedJoints,
    joint_table::JointTable, pair,
};
use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, RunReport};
use primase_core::{Clip, ClipTarget, Entity, JointMap, Name, Skeleton, Skin, World};
use primase_fromsoft::sekiro;
use primase_reskate::{
    game::{clip::LowerSkateClips, rig::render_map},
    read::animation,
};
use serde_json::json;
use std::collections::BTreeSet;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The authored joint table (bone names and correspondence only). It names two games, so it lives with the checks (the
/// toolkit names none), not in the animation toolkit.
const SEKIRO_SKATE_TABLE: &str = include_str!("fixtures/sekiro-human__skate-render.json");

/// The version-6 Oodle codec: the configured game library on Windows, oozextract elsewhere.
#[allow(unsafe_code)]
fn fromsoft_oodle(
    library: inputs::OodleLibrary,
) -> primase_foundations::compress::Result<primase_foundations::compress::oodle::Oodle> {
    // SAFETY: the test configuration names a trusted native library.
    unsafe {
        primase_foundations::compress::oodle::Oodle::open(
            library.path(),
            primase_foundations::compress::oodle::Version::V6,
        )
    }
}

const LOWER: &str = "lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program},select-lod{form=finest}";

fn manager() -> PassManager {
    let mut r = PassRegistry::new();
    primase_core::register(&mut r);
    sekiro::passes::register(&mut r);
    primase_animation::register(&mut r);
    primase_havok::register(&mut r);
    PassManager::new(r)
}
fn run(w: &mut World, text: &str) -> Result<()> {
    let m = manager();
    m.run(w, &m.registry.parse(text)?, &mut RunReport::default())?;
    Ok(())
}
fn named(w: &World, name: &str) -> Result<Entity> {
    w.iter::<Skeleton>()
        .find(|(e, _)| w.get::<Name>(*e).is_some_and(|n| n.0 == name))
        .map(|(e, _)| e)
        .ok_or_else(|| format!("no skeleton {name}").into())
}
fn skin_policy() -> RetargetSkin {
    RetargetSkin {
        fit: Fit::Positions,
        unmapped: UnmappedJoints::Nearest,
        placement: Placement::Automatic,
    }
}

/// The fit report is measured before changing meshes. It is residual/error evidence, not a visual-quality claim.
fn fit_skin(w: &mut World, source: Entity, target: Entity, table: &JointTable) -> Result<serde_json::Value> {
    let pairs = table.resolve(
        w.get::<Skeleton>(source).ok_or("source")?,
        w.get::<Skeleton>(target).ok_or("target")?,
    )?;
    let old: Vec<_> = w.iter::<JointMap>().map(|(e, _)| e).collect();
    for e in old {
        w.despawn(e);
    }
    let map = w.spawn();
    w.insert(
        map,
        JointMap {
            from: source,
            to: target,
            pairs,
        },
    );
    let policy = skin_policy();
    let before = policy.report(w)?;
    if before.is_empty() {
        return Err("no source skin for fit measurement".into());
    }
    let residual = before
        .iter()
        .flat_map(|(_, r)| r.joints.iter())
        .map(|j| j.residual)
        .fold(0.0, f64::max);
    let vertex_motion = before
        .iter()
        .flat_map(|(_, r)| r.groups.iter())
        .map(|g| g.largest_displacement)
        .fold(0.0, f64::max);
    let seams = before
        .iter()
        .filter_map(|(_, r)| r.largest_seam_gap.as_ref())
        .map(|g| g.metres)
        .fold(0.0, f64::max);
    let edge_gap = before
        .iter()
        .filter_map(|(_, r)| r.largest_edge_gap.as_ref())
        .map(|g| g.metres)
        .fold(0.0, f64::max);
    let vertices: u64 = before
        .iter()
        .flat_map(|(_, r)| &r.groups)
        .map(|g| g.vertices)
        .sum();
    let joints: Vec<_> = before.iter().flat_map(|(_, r)| &r.joints).map(|j| json!({"joint":j.joint,"target":j.target,
        "residual_m":j.residual,"segments":j.segments.iter().map(|s| json!({"to":s.to,"source_m":s.source,
            "target_m":s.target,"fitted_m":s.fitted,"target_source_ratio":(s.source > 1e-9).then(|| s.target / s.source)})).collect::<Vec<_>>() })).collect();
    let lines: Vec<_> = before.iter().flat_map(|(_, r)| r.lines()).collect();
    let mut log = PassLog::default();
    policy.run(w, &mut log)?;
    if !w
        .iter::<Skin>()
        .any(|(_, s)| s.bindings.iter().any(|b| b.skeleton == target))
    {
        return Err("retarget-skin did not bind any mesh to target".into());
    }
    Ok(
        json!({"fit":"positions","unmapped":"nearest","placement":"automatic", "max_joint_residual_m":residual,
        "largest_vertex_displacement_m":vertex_motion,"largest_seam_gap_m":seams,"largest_edge_growth_m":edge_gap,
        "vertices":vertices,"joints":joints,"details":lines,
        "approximations":log.approximations,"omitted":log.omitted}),
    )
}

#[test]
#[ignore = "installed Sekiro/skate pair proof; default pair fixtures are synthetic and self-contained"]
fn existing_soldier_and_skate_idle_transfer_both_ways_and_skin_rebinds() -> Result<()> {
    let (Some(root), Some(fs_dll), Some(data), Some(skate_dll)) = (
        install(Game::Sekiro),
        oodle(Tool::Oodle6),
        install_file(Game::Skate, "Data"),
        tool(Tool::Oodle9),
    ) else {
        return Err(
            "explicit pair proof needs Sekiro/skate installs and Oodle6/Oodle9; no game data was measured"
                .into(),
        );
    };
    let codec = fromsoft_oodle(fs_dll)?;
    let decoder = Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    let Some(keys) = inputs::keys(Game::Sekiro) else {
        return Err("needs keys.sekiro; no game data was measured".into());
    };
    let install = Install::new(
        root,
        fromsoft_formats::Game::Sekiro,
        fromsoft_formats::Keys::from_folder(keys, fromsoft_formats::Game::Sekiro, None)?,
    );
    let mut w = World::new();
    sekiro::read_character_with(
        &install,
        "c1010",
        &decoder,
        sekiro::CharacterOptions {
            animations: Some("c1010".into()),
            animation_clips: Some(vec!["a000_003000".into()]),
            ..Default::default()
        },
        &mut w,
    )?;
    run(
        &mut w,
        "lower-havok-clips{absent=neutral,rate=1,clips=a000_003000,root-motion=full},correspond-skeletons{from=c1010 havok skeleton,to=c1010 skeleton,tolerance=0.0001,parents=same},map-clip{unmapped=drop,rests=target,reparented=refuse}",
    )?;
    let human = named(&w, "c1010 skeleton")?;
    let fs_clip = w.iter::<Clip>().next().ok_or("soldier clip")?.1.clone();
    let folder = Folder::open(data)?;
    // SAFETY: explicitly configured trusted game decoder.
    #[allow(unsafe_code)]
    let skate_codec = unsafe {
        primase_foundations::compress::oodle::Oodle::load(
            skate_dll,
            primase_foundations::compress::oodle::Version::V9,
        )
    }?;
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&skate_codec),
    };
    let skeletons = animation::read_skeletons(&src, &mut w)?;
    let entries = animation::list_clips(&src, "Win32/levels/game/dingolevel_mpr/dingolevel_mpr.toc")?;
    let entry = entries
        .iter()
        .find(|c| {
            c.name
                .eq_ignore_ascii_case("animation/dingo/c_proto_ofbnb_idle_lfoot")
        })
        .ok_or("idle fixture")?;
    let skate_clip = animation::decode_clip(&src, entry, skeletons.animation, &mut w)?;
    LowerSkateClips.run(&mut w, &mut PassLog::default())?;
    let map = w.spawn();
    w.insert(
        map,
        JointMap {
            from: skeletons.animation,
            to: skeletons.render,
            pairs: render_map(
                w.get::<Skeleton>(skeletons.animation).ok_or("animation")?,
                w.get::<Skeleton>(skeletons.render).ok_or("render")?,
            )?,
        },
    );
    run(&mut w, "map-clip{unmapped=drop,rests=source,reparented=refuse}")?;
    let skate_clip = w.get::<Clip>(skate_clip).ok_or("lowered idle")?.clone();
    let a = w.get::<Skeleton>(human).ok_or("human")?;
    let b = w.get::<Skeleton>(skeletons.render).ok_or("render")?;
    let (table, report) = pair::available_table(a, b, &JointTable::parse(SEKIRO_SKATE_TABLE)?)?;
    let policy = RetargetClip {
        reference: Reference::Aligned,
        translation: Translation::Root,
    };
    let to_skate = pair::retarget_one(&fs_clip, a, b, skeletons.render, &table, policy)?;
    let to_soldier = pair::retarget_one(&skate_clip, b, a, human, &pair::reversed(&table)?, policy)?;
    let (on_skate, on_soldier) = (&to_skate.clip, &to_soldier.clip);
    for (clip, destination) in [(on_skate, skeletons.render), (on_soldier, human)] {
        assert!(
            clip.channels
                .iter()
                .any(|c| matches!(c.target, ClipTarget::Joint { skeleton, .. } if skeleton == destination))
        );
        assert!(
            clip.channels
                .iter()
                .all(|c| !matches!(c.target, ClipTarget::Joint { skeleton, .. } if skeleton != destination))
        );
    }
    assert_eq!(on_skate.root_motion, fs_clip.root_motion);
    run(&mut w, LOWER)?;
    let fit = fit_skin(&mut w, human, skeletons.render, &table)?;
    println!(
        "SEKIRO_SKATE_PAIR={}",
        json!({"character":"c1010","table":report,"skin":fit,
        "sekiro_to_skate":to_skate.approximations,"skate_to_sekiro":to_soldier.approximations,"skate_key":entry.key})
    );
    Ok(())
}

/// Explicit full census: every named character binder is considered. Human candidates have all four standard limb
/// roots; other skeletons stay in the log with their missing names. This criterion is reported, not an NPC taxonomy.
#[test]
#[ignore = "full installed Sekiro-to-skate human fit census; run explicitly with both games configured"]
fn every_sekiro_human_candidate_reports_pairs_ratios_and_skin_fit() -> Result<()> {
    let (Some(root), Some(names), Some(fs_dll), Some(data), Some(skate_dll)) = (
        install(Game::Sekiro),
        oracle(OracleSet::Uxm, Game::Sekiro, "dictionary.txt"),
        oodle(Tool::Oodle6),
        install_file(Game::Skate, "Data"),
        tool(Tool::Oodle9),
    ) else {
        return Err("explicit census needs Sekiro/skate installs, Sekiro dictionary and Oodle6/Oodle9; nothing was measured".into());
    };
    let codec = fromsoft_oodle(fs_dll)?;
    let decoder = Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    let Some(keys) = inputs::keys(Game::Sekiro) else {
        return Err("needs keys.sekiro; no game data was measured".into());
    };
    let install = Install::new(
        root,
        fromsoft_formats::Game::Sekiro,
        fromsoft_formats::Keys::from_folder(keys, fromsoft_formats::Game::Sekiro, None)?,
    );
    let archive = install.open()?;
    let dictionary = NameDictionary::read_for(names, fromsoft_formats::Game::Sekiro)?;
    let folder = Folder::open(data)?;
    // SAFETY: explicitly configured trusted game decoder.
    #[allow(unsafe_code)]
    let skate_codec = unsafe {
        primase_foundations::compress::oodle::Oodle::load(
            skate_dll,
            primase_foundations::compress::oodle::Version::V9,
        )
    }?;
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&skate_codec),
    };
    let mut light = World::new();
    let rig = animation::read_skeletons(&src, &mut light)?;
    let render = light.get::<Skeleton>(rig.render).ok_or("render skeleton")?;
    let table = JointTable::parse(SEKIRO_SKATE_TABLE)?;
    let ids: BTreeSet<_> = dictionary
        .names
        .values()
        .flatten()
        .filter_map(|n| {
            let id = n.strip_prefix("/chr/")?.strip_suffix(".chrbnd.dcx")?;
            (id.len() == 5
                && id.starts_with('c')
                && id[1..].bytes().all(|b| b.is_ascii_digit())
                && archive.contains(n))
            .then(|| id.to_owned())
        })
        .collect();
    let (mut flver_members, mut human_candidates, mut fitted_characters, mut refused_characters) =
        (0usize, 0usize, 0usize, 0usize);
    println!(
        "SEKIRO_SKATE_CENSUS_BEGIN={}",
        json!({"character_binders":ids.len(),"criterion":["Root","L_UpperArm","R_UpperArm","L_Thigh","R_Thigh"],
        "scope":"dictionary-named held character binders; unnamed hashes are unmeasured"})
    );
    let binders = ids.len();
    if binders == 0 {
        return Err(
            "configured dictionary listed no held character binders; census denominator is empty".into(),
        );
    }
    for id in ids {
        let skeletons = match sekiro::character_skeletons(&archive, &decoder, &id) {
            Ok(s) => s,
            Err(e) => {
                println!(
                    "SEKIRO_SKATE_CENSUS={}",
                    json!({"character":id,"status":"skeleton unread","reason":e.to_string()})
                );
                continue;
            }
        };
        if skeletons.is_empty() {
            println!(
                "SEKIRO_SKATE_CENSUS={}",
                json!({"character":id,"status":"no FLVER; placeholder or part-built, not measured"})
            );
        }
        for (member, skeleton) in skeletons {
            flver_members += 1;
            let observation = match pair::inspect(&skeleton, render, &table) {
                Ok(report) => report,
                Err(e) => {
                    println!(
                        "SEKIRO_SKATE_CENSUS={}",
                        json!({"character":id,"member":member,
                    "status":"pair inspection refused","reason":e.to_string()})
                    );
                    continue;
                }
            };
            let required = ["Root", "L_UpperArm", "R_UpperArm", "L_Thigh", "R_Thigh"];
            let missing: Vec<_> = required
                .iter()
                .filter(|name| !skeleton.joints.iter().any(|j| j.name == **name))
                .collect();
            let mut row = json!({"character":id,"member":member,"pair_report":observation,"human_candidate_missing_names":missing,
                "pairs_resolved":observation.pairs.iter().filter(|p| p.refusal.is_none()).count(),
                "pairs_unresolved":observation.pairs.iter().filter(|p| p.refusal.is_some()).count(),
                "unmapped_source_joints":observation.unmapped_source.len(),"unmapped_target_joints":observation.unmapped_target.len()});
            if missing.is_empty() {
                human_candidates += 1;
                let measured = (|| -> Result<serde_json::Value> {
                    let mut w = World::new();
                    sekiro::read_character_with(&install, &id, &decoder, Default::default(), &mut w)?;
                    run(&mut w, LOWER)?;
                    let source = named(&w, &format!("{id} skeleton"))?;
                    if w.get::<Skeleton>(source) != Some(&skeleton) {
                        return Err(
                            "FLVER member is not the character reader's selected skeleton; not fitted".into(),
                        );
                    }
                    let target = w.spawn();
                    w.insert(target, render.clone());
                    let (selected, _) = pair::available_table(&skeleton, render, &table)?;
                    fit_skin(&mut w, source, target, &selected)
                })();
                match measured {
                    Ok(fit) => {
                        fitted_characters += 1;
                        row["skin_fit"] = fit;
                        row["status"] = json!("measured human candidate");
                    }
                    Err(e) => {
                        refused_characters += 1;
                        row["status"] = json!("human candidate fit refused");
                        row["reason"] = json!(e.to_string());
                    }
                }
            } else {
                row["status"] = json!("outside stated human-pair criterion; pairs still reported");
            }
            println!("SEKIRO_SKATE_CENSUS={row}");
        }
    }
    // Part-built humans are not represented by their rig-only chrbnd. Keep every authored placement's choices
    // in the log; fit each distinct readable part set once. The established player-9000 fixture covers Wolf too.
    let mut part_sets = BTreeSet::new();
    part_sets.insert(vec![
        "fc_m_0200_l".to_owned(),
        "bd_m_9040_l".to_owned(),
        "am_m_9000_l".to_owned(),
        "lg_m_9000_l".to_owned(),
    ]);
    match fromsoft_formats::equipment::npc_outfit_catalog(&install, &archive, &decoder, &dictionary) {
        Ok(outfits) => {
            for notice in sekiro::outfit_notices("npc-outfit-catalog", &outfits) {
                println!(
                    "SEKIRO_SKATE_OUTFIT_CHOICE {} x{} {:?}",
                    notice.reason, notice.count, notice.examples
                );
            }
            for outfit in outfits {
                println!("SEKIRO_SKATE_OUTFIT={}", json!({"authored":outfit}));
                if !outfit.parts.is_empty() {
                    part_sets.insert(outfit.parts);
                }
            }
        }
        Err(e) => println!(
            "SEKIRO_SKATE_OUTFIT={}",
            json!({"status":"authored equipment census unresolved","reason":e.to_string()})
        ),
    }
    let distinct_part_sets = part_sets.len();
    let (mut fitted_parts, mut refused_parts) = (0usize, 0usize);
    for parts in part_sets {
        let measured = (|| -> Result<serde_json::Value> {
            let mut w = World::new();
            let names: Vec<_> = parts.iter().map(String::as_str).collect();
            sekiro::read_parts_with(&install, &names, &decoder, &Default::default(), &mut w)?;
            run(&mut w, LOWER)?;
            run(
                &mut w,
                "assemble-character{correspondence=by-name,reference-pose=assembled,tolerance=0.01,unweighted-rests=report,weighted-rests=refuse,name=pair_census}",
            )?;
            let source = named(&w, "pair_census skeleton")?;
            let skeleton = w.get::<Skeleton>(source).ok_or("assembled parts")?;
            let (selected, observation) = pair::available_table(skeleton, render, &table)?;
            let target = w.spawn();
            w.insert(target, render.clone());
            Ok(
                json!({"status":"measured part-built candidate","pair_report":observation,"skin_fit":fit_skin(&mut w, source, target, &selected)?}),
            )
        })();
        let row = match measured {
            Ok(row) => {
                fitted_parts += 1;
                row
            }
            Err(e) => {
                refused_parts += 1;
                json!({"status":"part-built fit refused","reason":e.to_string()})
            }
        };
        println!("SEKIRO_SKATE_PARTS={}", json!({"parts":parts,"measurement":row}));
    }
    println!(
        "SEKIRO_SKATE_CENSUS_SUMMARY={}",
        json!({"character_binders":binders,"flver_members":flver_members,
        "human_candidates":human_candidates,"character_fits":fitted_characters,"character_fit_refusals":refused_characters,
        "distinct_part_sets":distinct_part_sets,"part_fits":fitted_parts,"part_fit_refusals":refused_parts})
    );
    Ok(())
}
