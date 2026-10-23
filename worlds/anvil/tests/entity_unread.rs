//! Entity bodies built by hand: components whose type has no length rule are delimited by their headers and kept as
//! stored bytes, a header-like run inside a payload isn't a component, the inline Hierarchy and the group's links read.
use primase_anvil::read::entity::{BOUNDING_VOLUME, ComponentBody, StoredEntity, StoredEntityGroup};

/// A type with no length rule here (GameplayCoordinatorComponent), and one with a fixed 9-byte rule.
const UNRULED: u32 = 0x1D56_6A63;
const RULED: u32 = 0xA703_3693;

fn component(tag: u8, id: u64, kind: u32, body: &[u8]) -> Vec<u8> {
    let mut b = vec![tag];
    b.extend(id.to_le_bytes());
    b.extend(kind.to_le_bytes());
    b.extend(body);
    b
}

/// An Entity body: the first bytes (with the inline Hierarchy where asked), the identity matrix, `count` components
/// (`components` are their bytes) and the tail.
fn entity_body(hierarchy: bool, count: u32, components: &[u8]) -> Vec<u8> {
    let mut b = vec![1, if hierarchy { 0 } else { 3 }];
    if hierarchy {
        b.extend(5u64.to_le_bytes());
        b.extend(0x788B_AA0Du32.to_le_bytes());
        b.extend([0u8; 64]);
        b.extend(6u64.to_le_bytes());
        b.extend(0x709F_B9D4u32.to_le_bytes());
        b.extend([0u8; 13]);
    }
    for v in [
        1.0f32, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ] {
        b.extend(v.to_le_bytes());
    }
    b.extend(count.to_le_bytes());
    b.extend(components);
    // the tail: 43 bytes, a u32, the scale, the bounding volume, the EntityDescriptor, the DataLayerFilter (no
    // actions, no links) and `03`
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

#[test]
fn a_component_with_no_rule_is_delimited_by_the_headers_around_it() {
    let mut components = component(4, 0x2_0000_0001, UNRULED, &[0xAA; 10]);
    components.extend(component(4, 0x2_0000_0002, RULED, &[0; 9]));
    let e = StoredEntity::parse(&entity_body(false, 2, &components)).expect("entity");
    assert_eq!(e.components.len(), 2);
    assert_eq!(e.components[0].kind, UNRULED);
    assert_eq!(e.components[0].id, 0x2_0000_0001);
    assert_eq!(e.components[0].body, ComponentBody::Unread(vec![0xAA; 10]));
    assert_eq!(e.components[1].kind, RULED);
    assert_eq!(e.components[1].body, ComponentBody::Raw(vec![0; 9]));
    assert!(e.links.is_empty());
    assert!(e.hierarchy.is_none());
}

#[test]
fn a_header_like_run_with_a_small_id_inside_a_payload_is_not_a_component() {
    // a tag 0, a small u64 and a component class's type, inside the first component's bytes
    let mut payload = vec![0u8; 3];
    payload.extend(component(0, 3, UNRULED, &[0xBB; 2]));
    let mut components = component(4, 0x2_0000_0001, UNRULED, &payload);
    components.extend(component(4, 0x2_0000_0002, RULED, &[0; 9]));
    let e = StoredEntity::parse(&entity_body(false, 2, &components)).expect("entity");
    assert_eq!(e.components.len(), 2);
    assert_eq!(e.components[0].body, ComponentBody::Unread(payload));
}

#[test]
fn the_inline_hierarchy_after_the_first_byte_reads() {
    let e = StoredEntity::parse(&entity_body(true, 0, &[])).expect("entity");
    assert_eq!(e.hierarchy.map(|h| h.len()), Some(8 + 4 + 64 + 8 + 4 + 13));
    assert!(e.components.is_empty());
}

#[test]
fn a_groups_links_follow_its_members() {
    let mut body = entity_body(false, 0, &[]);
    body.extend(0u32.to_le_bytes()); // no members
    body.extend(1u32.to_le_bytes()); // one link: a tag and an id
    body.push(1);
    body.extend(0x2_0000_0003u64.to_le_bytes());
    body.extend(0u32.to_le_bytes());
    let g = StoredEntityGroup::parse(&body).expect("group");
    assert!(g.members.is_empty());
    assert_eq!(g.links, vec![0x2_0000_0003]);
    assert_eq!(g.word, 0);
}
