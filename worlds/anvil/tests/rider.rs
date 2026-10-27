//! A rider put together from chosen parts of Riders Republic's customization BuildTables: the gender rows of the part
//! tables name male and female meshes, a part whose mesh weights a bone the rig lacks is refused with the reason, and
//! the rider (rig, parts, clips chosen by coverage) lowers under the pass manager with its checks on.
mod inputs;

use std::collections::{BTreeMap, HashSet};

use anvil_formats::archive::{Archive, At};
use anvil_formats::build_table::StoredBuildTable;
use anvil_formats::build_table::{BUILD_TABLE, FEMALE, MALE};
use anvil_formats::mesh::MATERIAL;
use anvil_formats::mesh::{MESH, StoredMesh};
use anvil_formats::rider::{BEARD_COLOURS, HAIR_COLOURS, colours};
use anvil_formats::skeleton::{SKELETON, StoredSkeleton};
use inputs::{Game, install};
use primase_anvil::components::AnvilMesh;
use primase_anvil::passes::{
    LowerAnvilClips, LowerAnvilMesh, LowerAnvilRig, LowerAnvilSkin, OwnTransform, SelectAnvilClips,
    Static16Normal,
};
use primase_anvil::read::meshes::anvil_mesh;
use primase_anvil::read::placements::{
    Gender, InstancedLevels, Placed, Placer, RiderPartChoice, RiderSpec, Variant,
};
use primase_core::clip::{Clip, ClipBinding, Extrapolation, Interpolation};
use primase_core::frame::SourceFrame;
use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::rig::{RigInstance, SkinInstance};
use primase_core::{Instance, World};

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

fn link_id(p: &anvil_formats::build_table::BuildProperty) -> u64 {
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

fn choice(archive: &Archive, name: &str, variant: Variant) -> RiderPartChoice {
    RiderPartChoice {
        table: id_of(archive, BUILD_TABLE, name),
        variant,
    }
}

fn base(archive: &Archive) -> Vec<u64> {
    ["MAD_CHR_Skel_U", "MAD_CHR_SkelAddon_U", "MAD_CHR_SkelAddon_M"]
        .iter()
        .map(|n| id_of(archive, SKELETON, n))
        .collect()
}

/// A rider from four parts, each with the variant its materials table states: placed, its rig lowered with the root
/// replaced, the parts bound to it, and clips chosen by coverage, under the pass manager with its checks on; the
/// upper body is drawn with the Materials its variant table names, not the generic ones its mesh names.
#[test]
fn a_rider_of_chosen_parts_places_and_lowers_with_clips() {
    let Some(mut archive) = open() else {
        return;
    };
    let spec = RiderSpec {
        name: "rider".to_owned(),
        skeletons: base(&archive),
        gender: Gender::Male,
        parts: [
            "MAD_CHR_UBODY_001_U_1001",
            "MAD_CHR_LBODY_001_U_1001",
            "MAD_CHR_FOOTW_001_U",
            "MAD_CHR_HEADW_001_U",
        ]
        .iter()
        .map(|n| choice(&archive, n, Variant::Stated))
        .collect(),
    };
    let generic = id_of(&archive, MATERIAL, "MAD_CHR_UBody_MF_ALL");
    let effective = id_of(&archive, MATERIAL, "MAD_CHR_UBody_001_MF_1001_T001_V001");
    let mut w = World::new();
    let mut placed = Placed::default();
    let rider = {
        let mut placer = Placer::new(
            &mut archive,
            &mut w,
            SourceFrame::WORLD,
            InstancedLevels::Selector,
        )
        .with_materials()
        .with_clip_candidates();
        placer.place_rider(&spec, &mut placed)
    };
    for p in &rider.parts {
        println!(
            "{}: {} meshes, {} variants listed, variant {:?}, refused variant {:?}, placed {}, refused {:?}",
            p.name,
            p.meshes.len(),
            p.variants.len(),
            p.variant.as_ref().map(|v| v.pairs.len()),
            p.variant_refused,
            p.placements.len(),
            p.refused
        );
    }
    assert!(rider.rig.is_some() && rider.occurrence.is_some());
    let placed_parts = rider.parts.iter().filter(|p| !p.placements.is_empty()).count();
    assert_eq!(placed_parts, 4, "{:?}", rider.parts);
    // the upper body: its Stated variant replaces the generic body Material with the variant's
    let upper = &rider.parts[0];
    let v = upper.variant.as_ref().expect("the upper body's stated variant");
    assert!(v.pairs.contains(&(generic, effective)), "{:?}", v.pairs);
    let instance = w.get::<Instance>(upper.placements[0]).expect("instance");
    let mesh = w.get::<AnvilMesh>(instance.prototype).expect("mesh");
    assert!(mesh.materials.contains(&effective) && !mesh.materials.contains(&generic));
    assert!(
        mesh.materials
            .iter()
            .zip(&mesh.material_entities)
            .all(|(m, e)| (*m != effective) || e.is_some())
    );
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

/// A head part's row names two meshes (the head and the eyelashes): both place.
#[test]
fn a_head_part_places_all_the_meshes_its_row_names() {
    let Some(mut archive) = open() else {
        return;
    };
    let spec = RiderSpec {
        name: "head".to_owned(),
        skeletons: base(&archive),
        gender: Gender::Male,
        parts: vec![choice(&archive, "MAD_CHR_HEAD_001_U", Variant::Own)],
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
    let head = &rider.parts[0];
    println!("{:?}", head);
    assert_eq!((head.meshes.len(), head.placements.len()), (2, 2));
    assert!(head.refused.is_empty() && head.variant.is_none() && head.variant_refused.is_none());
}

/// A part whose mesh weights bones the rig lacks is refused with the reason, and one the table doesn't hold a row
/// for is refused too; neither stops the others. A stated variant a part doesn't have is refused as a variant only.
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
            choice(&archive, "MAD_CHR_LBODY_001_U_1001", Variant::Own),
            choice(&archive, "MAD_CHR_FOOTW_001_U", Variant::Own),
            choice(&archive, "MAD_CHR_ICON_BUNDLE", Variant::Own),
            choice(&archive, "MAD_CHR_HEAD_001_U", Variant::Stated),
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
    let reasons: Vec<&[String]> = rider.parts.iter().map(|p| p.refused.as_slice()).collect();
    println!("{reasons:?}");
    assert!(
        reasons[0]
            .iter()
            .any(|r| r.contains("bones the rig has no joint for"))
    );
    assert!(reasons[1].is_empty() && !rider.parts[1].placements.is_empty());
    assert!(!reasons[2].is_empty());
    // the head part has no materials table of variant tables: its Stated variant is refused, its meshes still place
    assert!(rider.parts[3].variant_refused.is_some());
    assert!(!rider.parts[3].placements.is_empty() || !rider.parts[3].refused.is_empty());
    assert!(!placed.refused.is_empty());
}

/// Every part table the slot tables list, both genders, with the variant its materials table states: how many list
/// variants, how many state one, and that the generic Materials the variant tables name are the part meshes' own.
#[test]
fn the_variant_tables_replace_the_materials_the_part_meshes_name() {
    let Some(mut archive) = open() else {
        return;
    };
    let mut slots: Vec<At> = Vec::new();
    let mut tables: BTreeMap<u64, At> = BTreeMap::new();
    for forge in 0..5 {
        for (entry, (i, n)) in archive.forge(forge).expect("forge").entries().enumerate() {
            if n.type_id == BUILD_TABLE {
                tables.entry(i.id).or_insert(At { forge, entry });
                if n.name.starts_with("MAD_CHR_SLOT_") {
                    slots.push(At { forge, entry });
                }
            }
        }
    }
    let mut parts: Vec<u64> = Vec::new();
    for at in &slots {
        let slot = StoredBuildTable::parse(&archive.resource(*at).expect("slot").body).expect("slot");
        for row in &slot.rows {
            for p in row.properties.iter().filter(|p| p.column == 1) {
                let id = link_id(p);
                if tables.contains_key(&id) && !parts.contains(&id) {
                    parts.push(id);
                }
            }
        }
    }
    let mut w = World::new();
    let mut all = Vec::new();
    for gender in [Gender::Male, Gender::Female] {
        let mut placer = Placer::new(
            &mut archive,
            &mut w,
            SourceFrame::WORLD,
            InstancedLevels::Selector,
        );
        for &table in &parts {
            all.push(placer.describe_part(
                RiderPartChoice {
                    table,
                    variant: Variant::Stated,
                },
                gender,
            ));
        }
    }
    let (mut described, mut listed, mut stated, mut refused, mut matched, mut replaced) =
        (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let mut reasons: BTreeMap<String, u64> = BTreeMap::new();
    for part in &all {
        if !part.refused.is_empty() || part.meshes.is_empty() {
            continue;
        }
        described += 1;
        listed += u64::from(!part.variants.is_empty());
        match (&part.variant, &part.variant_refused) {
            (Some(v), _) => {
                stated += 1;
                replaced += u64::try_from(v.pairs.len()).expect("count");
                // a generic Material of some pair is one a mesh of the part names
                let mut ok = false;
                for &m in &part.meshes {
                    let at = archive.find(m).expect("mesh");
                    let stored = StoredMesh::parse(&archive.resource(at).expect("mesh").body).expect("mesh");
                    let names = anvil_mesh(&stored, at.entry, m, SourceFrame::WORLD)
                        .expect("anvil mesh")
                        .materials;
                    ok |= v.pairs.iter().any(|(g, _)| names.contains(g));
                }
                matched += u64::from(ok);
            }
            (None, Some(why)) => {
                refused += 1;
                *reasons.entry(why.clone()).or_default() += 1;
            }
            (None, None) => {}
        }
    }
    println!(
        "{} part tables, {described} part rows described, {listed} list variants, {stated} state one, {refused} don't \
         ({reasons:?}), {replaced} replacement pairs, {matched} of the stated variants name a generic Material one of the \
         part's meshes has",
        parts.len()
    );
    assert_eq!(
        (parts.len(), described, listed, stated, refused, replaced, matched),
        (451, 815, 809, 767, 48, 2047, 767)
    );
}

/// A part drawn with a variant leaves no mesh nothing places: every mesh of the world is a prototype of a placement,
/// and placing the same part twice with different variants gives each its own Materials.
#[test]
fn a_variant_leaves_no_unplaced_mesh_and_the_same_part_can_be_placed_again() {
    let Some(mut archive) = open() else {
        return;
    };
    let table = id_of(&archive, BUILD_TABLE, "MAD_CHR_UBODY_001_U_1001");
    let mut scratch = World::new();
    let (stated, other) = {
        let mut placer = Placer::new(
            &mut archive,
            &mut scratch,
            SourceFrame::WORLD,
            InstancedLevels::Selector,
        );
        let part = placer.describe_part(
            RiderPartChoice {
                table,
                variant: Variant::Stated,
            },
            Gender::Male,
        );
        let other = part
            .variants
            .iter()
            .find(|v| v.weight != 1.0)
            .expect("another variant")
            .table;
        (part.variant.expect("the stated variant").table, other)
    };
    let spec = RiderSpec {
        name: "twice".to_owned(),
        skeletons: base(&archive),
        gender: Gender::Male,
        parts: vec![
            RiderPartChoice {
                table,
                variant: Variant::Table(stated),
            },
            RiderPartChoice {
                table,
                variant: Variant::Table(other),
            },
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
    .with_materials()
    .place_rider(&spec, &mut placed);
    assert!(
        rider
            .parts
            .iter()
            .all(|p| p.refused.is_empty() && p.placements.len() == 1),
        "{:?}",
        rider.parts
    );
    let materials = |p: &primase_anvil::read::placements::RiderPart| {
        let i = w.get::<Instance>(p.placements[0]).expect("instance");
        w.get::<AnvilMesh>(i.prototype).expect("mesh").materials.clone()
    };
    assert_ne!(materials(&rider.parts[0]), materials(&rider.parts[1]));
    let unplaced: Vec<_> = w
        .iter::<AnvilMesh>()
        .filter(|(e, _)| !w.iter::<Instance>().any(|(_, i)| i.prototype == *e))
        .collect();
    assert!(unplaced.is_empty(), "{} meshes nothing places", unplaced.len());
}

/// A hair or a beard part with a colour row replaces the generic Material its mesh names with that colour's: the
/// pairs name a Material the part's mesh has, and the LOD generic (no mesh of the part names it) is paired too.
#[test]
fn a_colour_row_replaces_the_generic_material_a_hair_or_beard_mesh_names() {
    let Some(mut archive) = open() else {
        return;
    };
    let hair = colours(&mut archive, HAIR_COLOURS).expect("hair colours");
    let beard = colours(&mut archive, BEARD_COLOURS).expect("beard colours");
    let mut w = World::new();
    for (part, colour, generic, effective) in [
        (
            "MAD_CHR_Hair_005_U_Full",
            &hair[1],
            "MAD_CHR_Hair_000_MF_ALL",
            "MAD_CHR_HairCut_V002_Chatin",
        ),
        (
            "MAD_CHR_Hair_005_U_Cut",
            &hair[0],
            "MAD_CHR_Hair_000_MF_ALL",
            "MAD_CHR_HairCut_V001_Blond",
        ),
        (
            "MAD_CHR_Beard_001_U_H000",
            &beard[1],
            "MAD_CHR_Beard_000_MF_ALL",
            "MAD_CHR_HairCut_V002_Chatin",
        ),
    ] {
        let table = id_of(&archive, BUILD_TABLE, part);
        let described = Placer::new(
            &mut archive,
            &mut w,
            SourceFrame::WORLD,
            InstancedLevels::Selector,
        )
        .describe_part(
            RiderPartChoice {
                table,
                variant: Variant::Row {
                    table: colour.table,
                    condition: colour.condition,
                },
            },
            Gender::Male,
        );
        assert!(
            described.refused.is_empty() && described.variant_refused.is_none(),
            "{part}"
        );
        let pairs = described.variant.expect("a variant").pairs;
        let generic = id_of(&archive, MATERIAL, generic);
        let effective = id_of(&archive, MATERIAL, effective);
        assert!(pairs.contains(&(generic, effective)), "{part}: {pairs:?}");
        // a mesh of the part names the generic Material the pair replaces
        let mut named = false;
        for &m in &described.meshes {
            let at = archive.find(m).expect("mesh");
            let stored = StoredMesh::parse(&archive.resource(at).expect("mesh").body).expect("mesh");
            named |= anvil_mesh(&stored, at.entry, m, SourceFrame::WORLD)
                .expect("anvil mesh")
                .materials
                .contains(&generic);
        }
        assert!(named, "{part}");
    }
}
