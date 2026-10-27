//! What a character's meshes bind to, on Riders Republic's forges read with the game's component registry: the
//! SkeletonComponents' rigs against the meshes of their Entities' Visuals.
use std::collections::HashMap;

use anvil_formats::archive::{Archive, At};
use anvil_formats::entity::{ComponentBody, SKELETON_COMPONENT, StoredEntity};
use anvil_formats::listing::Kind;
use anvil_formats::lod::StoredLodSelector;
use anvil_formats::mesh::StoredMesh;
use anvil_formats::rig::StoredRig;
use anvil_formats::skeleton::StoredSkeleton;
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
        let Ok(e) = StoredEntity::parse(
            &body,
            primase_anvil::components::component_types::is_component_type,
        ) else {
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
