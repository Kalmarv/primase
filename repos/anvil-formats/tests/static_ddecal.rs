//! A StaticDDecalComponent (0x5B413085), which HoudiniDecalInstancerComponent derives from without storing more, reads by
//! the same rule: an Entity built by hand holds one with an empty library (no model, no item), as the two of Riders
//! Republic do, and reads its 278 bytes as stored.
use anvil_formats::ddecal::{STATIC_DDECAL, StoredDeferredDecals};
use anvil_formats::entity::{BOUNDING_VOLUME, ComponentBody, StoredEntity};

/// The component registry the body here is read with: the one type it holds.
fn is_component_type(kind: u32) -> bool {
    [STATIC_DDECAL].contains(&kind)
}

/// An Entity body holding one component of type `kind` with `body`.
fn entity_with(kind: u32, body: &[u8]) -> Vec<u8> {
    let mut b = vec![1, 3];
    for v in [
        1.0f32, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ] {
        b.extend(v.to_le_bytes());
    }
    b.extend(1u32.to_le_bytes());
    b.push(4);
    b.extend(0x2_0000_0001u64.to_le_bytes());
    b.extend(kind.to_le_bytes());
    b.extend(body);
    // the tail: 43 bytes, a u32, the scale, the bounding volume, the EntityDescriptor, the DataLayerFilter, `03`
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

/// The body of a component with an empty library: 212 bytes, the StaticDDecalLibrary object, no model, no item, 46 bytes.
fn empty_library() -> Vec<u8> {
    let mut body = vec![0u8; 212];
    body.extend(0x2_0000_0002u64.to_le_bytes());
    body.extend(0xB43F_ECF1u32.to_le_bytes());
    body.extend(0u32.to_le_bytes());
    body.extend(0u32.to_le_bytes());
    body.extend([0u8; 46]);
    body
}

#[test]
fn a_static_ddecal_component_reads_by_the_instancers_rule() {
    let body = empty_library();
    assert_eq!(body.len(), 278);
    let e = StoredEntity::parse(&entity_with(STATIC_DDECAL, &body), is_component_type).expect("entity");
    assert_eq!(e.components.len(), 1);
    assert_eq!(e.components[0].kind, STATIC_DDECAL);
    assert_eq!(e.components[0].body, ComponentBody::Raw(body.clone()));
    // and its records read as the instancer's do: none
    let decals = StoredDeferredDecals::parse(&body).expect("decals");
    assert!(decals.items.is_empty());
}
