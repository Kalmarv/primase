//! An AnimComponent read from an Entity laid out by hand: which Animations it lists, and the rest kept as stored.
use anvil_formats::entity::{ANIM_COMPONENT, BOUNDING_VOLUME, ComponentBody, StoredEntity};

/// The component registry the body here is read with: the one type it holds.
fn is_component_type(kind: u32) -> bool {
    kind == ANIM_COMPONENT
}

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
    let e = StoredEntity::parse(&entity, is_component_type).expect("entity");
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
