//! A rider put together from chosen parts of Riders Republic's customization BuildTables: the gender rows of the part
//! tables name male and female meshes, a part whose mesh weights a bone the rig lacks is refused with the reason, and
//! the rider (rig, parts, clips chosen by coverage) lowers under the pass manager with its checks on.
use std::collections::{BTreeMap, HashSet};

use primase_anvil::components::StoredBuildTable;
use primase_anvil::passes::{
    LowerAnvilClips, LowerAnvilMesh, LowerAnvilRig, LowerAnvilSkin, OwnTransform, SelectAnvilClips,
    Static16Normal,
};
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::build_table::{BUILD_TABLE, FEMALE, MALE};
use primase_anvil::read::mesh::{MESH, StoredMesh};
use primase_anvil::read::placements::{Gender, InstancedLevels, Placed, Placer, RiderSpec};
use primase_anvil::read::skeleton::{SKELETON, StoredSkeleton};
use primase_core::World;
use primase_core::clip::{Clip, ClipBinding, Extrapolation, Interpolation};
use primase_core::frame::SourceFrame;
use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::rig::{RigInstance, SkinInstance};
use primase_testdata::{Game, install};

fn open() -> Option<Archive> {
    let dir = install(Game::Riders)?;
    let forges = [
        "DataPC.forge",
        "DataPC_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_WhiteRoom.forge",
    ];
    Archive::open(&forges.map(|f| dir.join(f))).ok()
}

/// The ClassID of the entry of type `type_id` named `name`, the first forge in the order given holding one.
fn id_of(archive: &Archive, type_id: u32, name: &str) -> u64 {
    (0..5)
        .find_map(|forge| {
            archive
                .forge(forge)?
                .entries()
                .find(|(_, n)| n.type_id == type_id && n.name == name)
                .map(|(i, _)| i.id)
        })
        .unwrap_or_else(|| panic!("no {type_id:#010x} named {name}"))
}

fn link_id(p: &primase_anvil::components::BuildProperty) -> u64 {
    match p.value.kind & 0x3F {
        0x1C => p.value.reference().expect("reference").map_or(0, |r| r.id),
        0x12 => p.value.link().expect("link").id,
        _ => 0,
    }
}

/// Every part table the slot tables list (column 1): its male row's Mesh link is named for `_M_` and its female row's
/// for `_F_` where it names one by gender at all (the evidence the two conditions are genders), counted; and how many
/// of the rows' meshes the base rig (the human Skeleton and the two addons) and the part's own addon cover, counting a
/// bone only where some vertex weights it.
#[test]
fn the_gender_rows_name_gendered_meshes_and_the_rigs_cover_most_parts() {
    let Some(mut archive) = open() else {
        return;
    };
    let hashes = |a: &mut Archive, id: u64| -> HashSet<u32> {
        let at = a.find(id).expect("skeleton");
        StoredSkeleton::parse(&a.resource(at).expect("resource").body)
            .expect("skeleton")
            .bones
            .iter()
            .map(|b| b.name_hash)
            .collect()
    };
    let main = id_of(&archive, SKELETON, "MAD_CHR_Skel_U");
    let base_u = id_of(&archive, SKELETON, "MAD_CHR_SkelAddon_U");
    let base_m = id_of(&archive, SKELETON, "MAD_CHR_SkelAddon_M");
    let base_f = id_of(&archive, SKELETON, "MAD_CHR_SkelAddon_F");
    let mut slots: Vec<(At, String)> = Vec::new();
    let mut tables: BTreeMap<u64, At> = BTreeMap::new();
    for forge in 0..5 {
        for (entry, (i, n)) in archive.forge(forge).expect("forge").entries().enumerate() {
            if n.type_id == BUILD_TABLE {
                tables.entry(i.id).or_insert(At { forge, entry });
                if n.name.starts_with("MAD_CHR_SLOT_") {
                    slots.push((At { forge, entry }, n.name.clone()));
                }
            }
        }
    }
    let (mut rows, mut named, mut wrong, mut covered, mut refused, mut other) =
        (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let mut seen: HashSet<u64> = HashSet::new();
    for (at, _) in &slots {
        let slot = StoredBuildTable::parse(&archive.resource(*at).expect("slot").body).expect("slot");
        for row in &slot.rows {
            for p in row.properties.iter().filter(|p| p.column == 1) {
                let part = link_id(p);
                let Some(&pat) = tables.get(&part) else {
                    continue;
                };
                if !seen.insert(part) {
                    continue;
                }
                let t = StoredBuildTable::parse(&archive.resource(pat).expect("part").body).expect("part");
                for r in &t.rows {
                    let mut mesh = None;
                    let mut skeletons = Vec::new();
                    for (_, id) in r.links().expect("links") {
                        let Some(a) = archive.find(id) else {
                            continue;
                        };
                        match archive.forge(a.forge).expect("forge").names[a.entry].type_id {
                            MESH if mesh.is_none() => mesh = Some((a, id)),
                            SKELETON => skeletons.push(id),
                            _ => {}
                        }
                    }
                    let Some((m, _)) = mesh else {
                        continue;
                    };
                    if r.condition != MALE && r.condition != FEMALE {
                        other += 1;
                        continue;
                    }
                    rows += 1;
                    let name = archive.forge(m.forge).expect("forge").names[m.entry].name.clone();
                    let male = r.condition == MALE;
                    if name.contains("_M_") || name.contains("_F_") {
                        named += 1;
                        if name.contains("_M_") != male {
                            wrong += 1;
                            println!("the other gender's mesh: {name}, condition {:#010x}", r.condition);
                        }
                    }
                    let mut have = hashes(&mut archive, main);
                    have.extend(hashes(&mut archive, base_u));
                    have.extend(hashes(&mut archive, if male { base_m } else { base_f }));
                    for s in skeletons {
                        have.extend(hashes(&mut archive, s));
                    }
                    let stored = StoredMesh::parse(&archive.resource(m).expect("mesh").body).expect("mesh");
                    let bones = stored.bones().expect("bones");
                    let ok = match stored.skin().expect("skin") {
                        Some(skin) => bones
                            .iter()
                            .zip(skin.weighted_slots(bones.len()))
                            .all(|(b, w)| !w || have.contains(&b.bone_id)),
                        None => true,
                    };
                    if ok {
                        covered += 1;
                    } else {
                        refused += 1;
                    }
                }
            }
        }
    }
    println!(
        "{} part tables listed by the slot tables, {rows} male and female rows with a Mesh ({other} more with another \
         condition), {named} of them gendered by name, {wrong} of those on the other gender's condition, {covered} \
         covered by the base rig and the part's addon, {refused} not",
        seen.len()
    );
    assert_eq!(
        (seen.len(), rows, other, named, wrong, covered, refused),
        (451, 815, 2, 814, 0, 767, 48)
    );
}

/// A rider from four parts: placed, its rig lowered with the root replaced, the parts bound to it, and clips chosen
/// by coverage, under the pass manager with its checks on.
#[test]
fn a_rider_of_chosen_parts_places_and_lowers_with_clips() {
    let Some(mut archive) = open() else {
        return;
    };
    let spec = RiderSpec {
        name: "rider".to_owned(),
        skeletons: ["MAD_CHR_Skel_U", "MAD_CHR_SkelAddon_U", "MAD_CHR_SkelAddon_M"]
            .iter()
            .map(|n| id_of(&archive, SKELETON, n))
            .collect(),
        gender: Gender::Male,
        parts: [
            "MAD_CHR_UBODY_001_U_1001",
            "MAD_CHR_LBODY_001_U_1001",
            "MAD_CHR_FOOTW_001_U",
            "MAD_CHR_HEADW_001_U",
        ]
        .iter()
        .map(|n| id_of(&archive, BUILD_TABLE, n))
        .collect(),
    };
    let mut w = World::new();
    let mut placed = Placed::default();
    let rider = {
        let mut placer = Placer::new(
            &mut archive,
            &mut w,
            SourceFrame::WORLD,
            InstancedLevels::Selector,
        )
        .with_clip_candidates();
        placer.place_rider(&spec, &mut placed)
    };
    for p in &rider.parts {
        println!(
            "{}: mesh {:?}, {} skeletons, {} selectors, {} tables, placed {}, refused {:?}",
            p.name,
            p.mesh,
            p.skeletons.len(),
            p.selectors.len(),
            p.tables.len(),
            p.placement.is_some(),
            p.refused
        );
    }
    assert!(rider.rig.is_some() && rider.occurrence.is_some());
    let placed_parts = rider.parts.iter().filter(|p| p.placement.is_some()).count();
    assert!(placed_parts > 0, "{:?}", rider.parts);
    let manager = PassManager::new(PassRegistry::new()).with_pass_checks(true);
    let pipeline = Pipeline {
        passes: vec![
            Box::new(LowerAnvilMesh {
                static16_normal: Static16Normal::Omit,
            }),
            Box::new(LowerAnvilRig {
                own_transform: OwnTransform::Replace,
            }),
            Box::new(LowerAnvilSkin),
            Box::new(SelectAnvilClips {
                min_coverage: 0.9,
                max_clips: 8,
            }),
            Box::new(LowerAnvilClips {
                interpolation: Interpolation::Linear,
                extrapolation: Extrapolation::Hold,
            }),
        ],
    };
    manager
        .run(&mut w, &pipeline, &mut RunReport::default())
        .unwrap_or_else(|e| panic!("{e:#}"));
    let clips = w.iter::<Clip>().count();
    let bindings = w.iter::<ClipBinding>().count();
    let skins = w.iter::<SkinInstance>().count();
    println!(
        "{placed_parts} parts placed, {} rig instances, {skins} skin instances, {clips} clips, {bindings} bindings",
        w.iter::<RigInstance>().count()
    );
    assert_eq!(w.iter::<RigInstance>().count(), 1);
    assert!(skins >= placed_parts && clips > 0 && bindings == clips);
}

/// A part whose mesh weights bones the rig lacks is refused with the reason, and one the table doesn't hold a row
/// for is refused too; neither stops the others.
#[test]
fn a_part_the_rig_cannot_carry_is_refused_with_its_reason() {
    let Some(mut archive) = open() else {
        return;
    };
    // the body part without the base addons: its mesh weights bones only they hold
    let spec = RiderSpec {
        name: "bare".to_owned(),
        skeletons: vec![id_of(&archive, SKELETON, "MAD_CHR_Skel_U")],
        gender: Gender::Male,
        parts: vec![
            id_of(&archive, BUILD_TABLE, "MAD_CHR_LBODY_001_U_1001"),
            id_of(&archive, BUILD_TABLE, "MAD_CHR_FOOTW_001_U"),
            id_of(&archive, BUILD_TABLE, "MAD_CHR_ICON_BUNDLE"),
        ],
    };
    let mut w = World::new();
    let mut placed = Placed::default();
    let rider = Placer::new(
        &mut archive,
        &mut w,
        SourceFrame::WORLD,
        InstancedLevels::Selector,
    )
    .place_rider(&spec, &mut placed);
    let reasons: Vec<Option<&str>> = rider.parts.iter().map(|p| p.refused.as_deref()).collect();
    println!("{reasons:?}");
    assert!(reasons[0].is_some_and(|r| r.contains("bones the rig has no joint for")));
    assert!(reasons[1].is_none());
    assert!(reasons[2].is_some());
    assert!(!placed.refused.is_empty());
}
