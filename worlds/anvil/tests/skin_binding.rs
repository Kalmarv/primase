//! What a character's meshes bind to: a SkeletonComponent read from an Entity laid out by hand, and the census of
//! Riders Republic's forges for the rest of the chain (a Mesh's `MeshBone` records and skin stream against the bone
//! names of the Skeletons, and the SkeletonComponents' rigs against the meshes of their Entities' Visuals).
use std::collections::{HashMap, HashSet};

use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::entity::{
    BOUNDING_VOLUME, ComponentBody, Refusal, SKELETON_COMPONENT, StoredEntity,
};
use primase_anvil::read::listing::Kind;
use primase_anvil::read::lod::StoredLodSelector;
use primase_anvil::read::mesh::StoredMesh;
use primase_anvil::read::rig::StoredRig;
use primase_anvil::read::skeleton::StoredSkeleton;
use primase_testdata::{Game, install};

fn component(tag: u8, id: u64, kind: u32, body: &[u8]) -> Vec<u8> {
    let mut b = vec![tag];
    b.extend(id.to_le_bytes());
    b.extend(kind.to_le_bytes());
    b.extend(body);
    b
}

/// An Entity body of the components' bytes (as tests/entity_unread.rs lays one out: no hierarchy, an identity matrix,
/// the 43-byte run, the scale, the bounding volume, the descriptor, no data layers, no links, `03`).
fn entity_body(count: u32, components: &[u8]) -> Vec<u8> {
    let mut b = vec![1, 3];
    for v in [
        1.0f32, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ] {
        b.extend(v.to_le_bytes());
    }
    b.extend(count.to_le_bytes());
    b.extend(components);
    b.extend([0u8; 43]);
    b.extend(0u32.to_le_bytes());
    b.extend(1.0f32.to_le_bytes());
    b.extend(7u64.to_le_bytes());
    b.extend(BOUNDING_VOLUME.to_le_bytes());
    b.extend([0u8; 24]);
    b.extend(0u32.to_le_bytes());
    b.extend(8u64.to_le_bytes());
    b.extend(0x6012_1A9Eu32.to_le_bytes());
    b.extend([0u8; 13]);
    b.extend(9u64.to_le_bytes());
    b.extend(0xDB1D_406Eu32.to_le_bytes());
    b.extend(0u32.to_le_bytes());
    b.extend(0u32.to_le_bytes());
    b.push(3);
    b
}

fn reference(mode: u8, id: u64) -> Vec<u8> {
    let mut b = vec![1, mode];
    b.extend(id.to_le_bytes());
    b
}

/// A SkeletonComponent's body: head, main, the addons, the ExpressionPack (none), the tracks and the WindForce.
fn skeleton_component(main: u64, addons: &[u64], tracks: &[(u64, [u8; 8])]) -> Vec<u8> {
    let mut b = vec![0, 1, 0, 0];
    b.extend(reference(1, main));
    b.extend((addons.len() as u32).to_le_bytes());
    for a in addons {
        b.extend(reference(4, *a));
    }
    b.extend([3, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    b.extend((tracks.len() as u32).to_le_bytes());
    for (id, rest) in tracks {
        b.extend(id.to_le_bytes());
        b.extend(0xA803_7D53u32.to_le_bytes());
        b.extend(rest);
    }
    b.extend(0x018B_2FEB_0682u64.to_le_bytes());
    b.extend(0xE7F9_A244u32.to_le_bytes());
    b.extend([1, 1, 1, 0, 0, 0, 0x80, 0x3F, 0, 0, 0, 0]);
    b
}

#[test]
fn a_skeleton_component_reads_its_main_addons_tracks_and_wind_force() {
    let body = skeleton_component(
        0x0177_B6B7_756A,
        &[0x01CC_5902_684C, 0x01C3_9E65_8FDC],
        &[(0x01D8_B79A_F6B5, [0xED, 0x30, 0xC7, 0x14, 1, 0, 0, 0])],
    );
    assert_eq!(body.len(), 56 + 2 * 10 + 20);
    let entity = entity_body(1, &component(4, 0x2_0000_0001, SKELETON_COMPONENT, &body));
    let e = StoredEntity::parse(&entity).expect("entity");
    let ComponentBody::Skeleton(s) = &e.components[0].body else {
        panic!("{:?}", e.components[0].body)
    };
    assert_eq!(s.head, [0, 1, 0, 0]);
    assert_eq!(s.main.map(|r| (r.mode, r.id)), Some((1, 0x0177_B6B7_756A)));
    assert_eq!(
        s.addons.iter().map(|r| (r.mode, r.id)).collect::<Vec<_>>(),
        [(4, 0x01CC_5902_684C), (4, 0x01C3_9E65_8FDC)]
    );
    assert_eq!(s.expression_pack, None);
    assert_eq!(
        s.tracks,
        [(0x01D8_B79A_F6B5, [0xED, 0x30, 0xC7, 0x14, 1, 0, 0, 0])]
    );
    assert_eq!(s.wind_force.0, 0x018B_2FEB_0682);
    assert_eq!(&s.wind_force.1[..4], [1, 1, 1, 0]);
}

#[test]
fn a_skeleton_component_that_does_not_end_where_its_headers_say_is_refused_with_why() {
    let mut body = skeleton_component(0x0177_B6B7_756A, &[], &[]);
    body.push(0xAA);
    let entity = entity_body(1, &component(4, 0x2_0000_0001, SKELETON_COMPONENT, &body));
    let e = StoredEntity::parse(&entity).expect("entity");
    assert!(
        matches!(&e.components[0].body, ComponentBody::Refused { bytes, why: Refusal::Ends { at, end } } if *bytes == body && at + 1 == *end)
    );
}

#[test]
fn refusals_alike_but_for_their_positions_have_one_reason() {
    let refused = |extra: usize| {
        let mut body = skeleton_component(0x0177_B6B7_756A, &[], &[]);
        body.extend(std::iter::repeat_n(0xAA, extra));
        let entity = entity_body(1, &component(4, 0x2_0000_0001, SKELETON_COMPONENT, &body));
        match StoredEntity::parse(&entity)
            .expect("entity")
            .components
            .remove(0)
            .body
        {
            ComponentBody::Refused { why, .. } => why,
            other => panic!("not refused: {other:?}"),
        }
    };
    let (one, two) = (refused(1), refused(2));
    assert_ne!(one.to_string(), two.to_string());
    assert_eq!(one.reason(), two.reason());
}

fn open() -> Option<Archive> {
    let dir = install(Game::Riders)?;
    let forges = [
        "DataPC.forge",
        "DataPC_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_WhiteRoom.forge",
    ];
    let paths: Vec<_> = forges.iter().map(|f| dir.join(f)).collect();
    Some(Archive::open(&paths).expect("archive"))
}

fn rows(archive: &Archive, kind: Kind) -> Vec<(At, String)> {
    archive
        .listing()
        .filter(|(_, l)| l.kind == Some(kind))
        .map(|(forge, l)| {
            (
                At {
                    forge,
                    entry: l.entry,
                },
                l.name.to_owned(),
            )
        })
        .collect()
}

/// The bone records and skin streams of every Mesh with bones, against the bone names of every Skeleton.
#[test]
fn every_mesh_bone_list_and_skin_stream_reads_and_its_ids_are_bone_names() {
    let Some(mut archive) = open() else {
        return;
    };
    let mut names: HashSet<u32> = HashSet::new();
    for (at, name) in rows(&archive, Kind::Skeleton) {
        let s = StoredSkeleton::parse(&archive.resource(at).expect("skeleton").body)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        names.extend(s.bones.iter().map(|b| b.name_hash));
    }
    let (mut meshes, mut ids, mut named, mut vertices) = (0u64, 0u64, 0u64, 0u64);
    let mut third_streams = 0u64;
    let mut unnamed: HashSet<u32> = HashSet::new();
    let mut by_influences: HashMap<usize, u64> = HashMap::new();
    for (at, name) in rows(&archive, Kind::Mesh) {
        let mesh = StoredMesh::parse(&archive.resource(at).expect("mesh").body)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let bones = mesh.bones().unwrap_or_else(|e| panic!("{name}: {e}"));
        let skin = mesh.skin().unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(bones.is_empty(), skin.is_none(), "{name}");
        let Some(skin) = skin else {
            if !mesh.skin_stream.is_empty() {
                third_streams += 1;
            }
            continue;
        };
        meshes += 1;
        let distinct: HashSet<u32> = bones.iter().map(|b| b.bone_id).collect();
        assert_eq!(distinct.len(), bones.len(), "{name}: a bone twice");
        for b in &bones {
            ids += 1;
            if names.contains(&b.bone_id) {
                named += 1;
            } else {
                unnamed.insert(b.bone_id);
            }
        }
        vertices += u64::try_from(skin.vertices().count()).expect("count");
        *by_influences.entry(skin.influences).or_default() += 1;
    }
    println!(
        "{meshes} meshes with bones, {ids} bone ids of which {named} are the name of a bone of some Skeleton ({} ids \
         are not), {vertices} skinned vertices, meshes by influences {by_influences:?}; {third_streams} meshes with a          third stream and no bones",
        unnamed.len()
    );
    assert_eq!(meshes, 29_174);
    assert_eq!(ids, 497_194);
    assert_eq!(named, 497_168);
    assert_eq!(unnamed.len(), 6);
    assert_eq!(vertices, 39_795_423);
    assert_eq!(by_influences, HashMap::from([(4, 29_116), (8, 58)]));
}

/// Every SkeletonComponent of the Entities reads, its Skeletons join into a rig, and the meshes of its Entity's
/// Visuals name bones of that rig.
#[test]
fn every_skeleton_component_reads_and_its_rig_holds_the_bones_of_its_meshes() {
    let Some(mut archive) = open() else {
        return;
    };
    let (mut components, mut rigs, mut covered, mut short) = (0u64, 0u64, 0u64, 0u64);
    let (mut addons, mut shared, mut added) = (0u64, 0u64, 0u64);
    let mut shortfalls: HashMap<u32, u64> = HashMap::new();
    for (at, name) in rows(&archive, Kind::Entity) {
        let body = archive.resource(at).expect("entity").body;
        let Ok(e) = StoredEntity::parse(&body) else {
            continue;
        };
        for c in &e.components {
            if c.kind != SKELETON_COMPONENT {
                continue;
            }
            let ComponentBody::Skeleton(sc) = &c.body else {
                panic!("{name}: a SkeletonComponent read as {:?}", c.body)
            };
            components += 1;
            let mut skeletons = Vec::new();
            for r in sc.main.iter().chain(&sc.addons) {
                let at = archive
                    .find(r.id)
                    .unwrap_or_else(|| panic!("{name}: no resource {:#x}", r.id));
                let s = StoredSkeleton::parse(&archive.resource(at).expect("skeleton").body)
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
                skeletons.push(s);
            }
            let refs: Vec<&StoredSkeleton> = skeletons.iter().collect();
            let rig = StoredRig::assemble(&refs).unwrap_or_else(|e| panic!("{name}: {e}"));
            rigs += 1;
            let main_bones = skeletons.first().map_or(0, |s| s.bones.len());
            let total: usize = skeletons.iter().map(|s| s.bones.len()).sum();
            addons += u64::try_from(skeletons.len().saturating_sub(1)).expect("count");
            added += u64::try_from(rig.bones.len().saturating_sub(main_bones)).expect("count");
            shared += u64::try_from(total.saturating_sub(rig.bones.len())).expect("count");
            let mut meshes = Vec::new();
            for v in e.visuals() {
                let Some(object) = v.object else { continue };
                let Some(at) = archive.find(object.id) else {
                    continue;
                };
                let body = archive.resource(at).expect("object").body;
                match StoredLodSelector::parse(&body) {
                    Ok(l) => meshes.extend(l.level_ids().expect("levels").into_iter().flatten()),
                    Err(_) => meshes.push(object.id),
                }
            }
            for id in meshes {
                let Some(at) = archive.find(id) else { continue };
                let Ok(mesh) = StoredMesh::parse(&archive.resource(at).expect("mesh").body) else {
                    continue;
                };
                let bones = mesh.bones().expect("bones");
                if bones.is_empty() {
                    continue;
                }
                let missing = rig.palette(&bones).iter().filter(|p| p.is_none()).count();
                if missing == 0 {
                    covered += 1;
                } else {
                    short += 1;
                    *shortfalls
                        .entry(u32::try_from(missing).expect("count"))
                        .or_default() += 1;
                }
            }
        }
    }
    println!(
        "{components} SkeletonComponents, {rigs} rigs of {addons} addon Skeletons ({shared} bones shared with an earlier \
         Skeleton, {added} added to the main one's); bone-bearing meshes of their Visuals: {covered} wholly in the rig, \
         {short} not (bones missing -> meshes {shortfalls:?})"
    );
    assert_eq!(components, 146);
    assert_eq!(rigs, 146);
    assert_eq!(covered, 523);
    assert_eq!(short, 48);
    assert_eq!(shortfalls, HashMap::from([(1, 48)]));
}
