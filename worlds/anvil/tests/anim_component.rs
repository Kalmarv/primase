//! An AnimComponent read from an Entity laid out by hand, and the census of every Entity of Riders Republic's forges:
//! which Animations each lists, and that each one resolves to an Animation resource.
use std::collections::BTreeMap;

use primase_anvil::read::animation::ANIMATION;
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::entity::{ANIM_COMPONENT, BOUNDING_VOLUME, ComponentBody, StoredEntity};
use primase_anvil::read::listing::Kind;
use primase_testdata::{Game, install};

fn component(tag: u8, id: u64, kind: u32, body: &[u8]) -> Vec<u8> {
    let mut b = vec![tag];
    b.extend(id.to_le_bytes());
    b.extend(kind.to_le_bytes());
    b.extend(body);
    b
}

/// An Entity body of the components' bytes (as tests/entity_unread.rs lays one out).
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

fn reference(id: u64) -> Vec<u8> {
    let mut b = vec![1, 1];
    b.extend(id.to_le_bytes());
    b
}

#[test]
fn an_anim_component_reads_its_animations_and_keeps_the_rest() {
    let mut body = vec![0, 1, 0, 0];
    body.extend(2u32.to_le_bytes());
    body.extend(reference(0x1F0_FB14_9009));
    body.extend(reference(0x1F0_FB16_1303));
    let tail = [vec![1, 1], 0.2f32.to_le_bytes().to_vec(), vec![0; 6]].concat();
    body.extend(&tail);
    let entity = entity_body(1, &component(4, 0x2_0000_0001, ANIM_COMPONENT, &body));
    let e = StoredEntity::parse(&entity).expect("entity");
    let ComponentBody::Anim(a) = &e.components[0].body else {
        panic!("{:?}", e.components[0].body)
    };
    assert_eq!(a.head, [0, 1, 0, 0]);
    assert_eq!(
        a.animations.iter().map(|r| r.map(|r| r.id)).collect::<Vec<_>>(),
        [Some(0x1F0_FB14_9009), Some(0x1F0_FB16_1303)]
    );
    assert_eq!(a.tail, tail);
}

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
        let Ok(e) = StoredEntity::parse(&body) else {
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
