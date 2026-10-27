//! The census of every Entity of Riders Republic's forges, read with the game's component registry: which Animations
//! each AnimComponent lists, and that each one resolves to an Animation resource.
use std::collections::BTreeMap;

use anvil_formats::animation::ANIMATION;
use anvil_formats::archive::{Archive, At};
use anvil_formats::entity::{ANIM_COMPONENT, ComponentBody, StoredEntity};
use anvil_formats::listing::Kind;
use primase_testdata::{Game, install};

/// Every AnimComponent of the Entities.
#[test]
fn every_anim_component_lists_animations_that_resolve() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let forges = [
        "DataPC.forge",
        "DataPC_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_WhiteRoom.forge",
    ];
    let mut archive = Archive::open(&forges.map(|f| dir.join(f))).expect("archive");
    let rows: Vec<At> = archive
        .listing()
        .filter(|(_, l)| l.kind == Some(Kind::Entity))
        .map(|(forge, l)| At {
            forge,
            entry: l.entry,
        })
        .collect();
    let (mut components, mut typed, mut listing, mut references, mut resolved) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut by_count: BTreeMap<usize, u64> = BTreeMap::new();
    for at in rows {
        let body = archive.resource(at).expect("entity").body;
        let Ok(e) = StoredEntity::parse(
            &body,
            primase_anvil::components::component_types::is_component_type,
        ) else {
            continue;
        };
        for c in e.components.iter().filter(|c| c.kind == ANIM_COMPONENT) {
            components += 1;
            let ComponentBody::Anim(a) = &c.body else {
                continue;
            };
            typed += 1;
            listing += u64::from(!a.animations.is_empty());
            *by_count.entry(a.animations.len()).or_default() += 1;
            for r in a.animations.iter().flatten() {
                references += 1;
                let found = archive.find(r.id).is_some_and(|f| {
                    archive.forge(f.forge).expect("forge").names[f.entry].type_id == ANIMATION
                });
                resolved += u64::from(found);
            }
        }
    }
    println!(
        "{components} AnimComponents, {typed} read, {listing} listing Animations ({by_count:?} by count), {references} \
         references of which {resolved} resolve to an Animation"
    );
    assert_eq!(
        (components, typed, listing, references, resolved),
        (70, 70, 19, 68, 68)
    );
    assert_eq!(
        by_count,
        BTreeMap::from([(0, 51), (1, 8), (2, 8), (7, 2), (31, 1)])
    );
}
