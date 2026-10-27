//! Grass components built by hand, in the layout Riders Republic's 46 GrassVisuals and its GrassComponent have: a
//! GrassVisual reads by its rule (the Visual head, a GrassVisualInstance with a MeshInstanceData's fields, its u16
//! array, the imposter pointer, the Visual tail and the model's number), and a GrassComponent is read over its extent
//! (its settings have no length rule): the library reference, the settings as stored and the ManagedEntities' ids.
use anvil_formats::entity::{BOUNDING_VOLUME, ComponentBody, GRASS_COMPONENT, GRASS_VISUAL, StoredEntity};

/// A reference to a Mesh as the forge stores one (a tag, a flag byte, a 5-byte id and zeros).
const REFERENCE: [u8; 10] = [1, 1, 0x66, 0xB5, 0xAD, 0x34, 0xD0, 1, 0, 0];

/// The component registry the bodies here are read with: the two types they hold.
fn is_component_type(kind: u32) -> bool {
    [GRASS_VISUAL, GRASS_COMPONENT].contains(&kind)
}

fn component(kind: u32, id: u64, body: &[u8]) -> Vec<u8> {
    let mut b = vec![4];
    b.extend(id.to_le_bytes());
    b.extend(kind.to_le_bytes());
    b.extend(body);
    b
}

/// An Entity body holding `count` components (`components` are their bytes).
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

/// A MeshInstanceData's fields with no instance, no material info and no instance bounds: the matrix count, the
/// bounding volume, the 16 reserved bytes, the CompiledMeshInstance (type and 17 bytes) and two zero counts.
fn mesh_instance() -> Vec<u8> {
    let mut b = 0u32.to_le_bytes().to_vec();
    b.extend(5u64.to_le_bytes());
    b.extend(BOUNDING_VOLUME.to_le_bytes());
    b.extend([0u8; 24]);
    b.extend(0u32.to_le_bytes());
    b.push(3);
    b.extend([0u8; 15]);
    b.extend(0x4368_101Bu32.to_le_bytes());
    b.extend([0u8; 17]);
    b.extend(0u32.to_le_bytes());
    b.extend(0u32.to_le_bytes());
    b
}

/// A Visual's 125-byte tail: two u32, `03` and 116 bytes.
fn visual_tail() -> Vec<u8> {
    let mut b = 1u32.to_le_bytes().to_vec();
    b.extend(0u32.to_le_bytes());
    b.push(3);
    b.extend([0u8; 116]);
    b
}

/// The start of a grass Visual: the head, the reference, the instance tag, its id and type.
fn grass_head(instance_type: u32) -> Vec<u8> {
    let mut b = vec![0, 1, 0, 0];
    b.extend(REFERENCE);
    b.push(0);
    b.extend(6u64.to_le_bytes());
    b.extend(instance_type.to_le_bytes());
    b
}

#[test]
fn a_grass_visual_reads_by_its_rule() {
    let mut body = grass_head(0xD456_9683);
    body.extend(mesh_instance());
    body.extend(3u32.to_le_bytes());
    for range in [4u16, 5, 6] {
        body.extend(range.to_le_bytes());
    }
    body.push(3); // GrassImposterProperties: null
    body.extend(visual_tail());
    body.push(7); // GrassModelID
    let components = component(GRASS_VISUAL, 0x2_0000_0001, &body);
    let e = StoredEntity::parse(&entity_body(1, &components), is_component_type).expect("entity");
    assert_eq!(e.components.len(), 1);
    let ComponentBody::GrassVisual(g) = &e.components[0].body else {
        panic!("not a GrassVisual: {:?}", e.components[0].body);
    };
    assert_eq!(g.model, 7);
    assert_eq!(g.ranges, vec![4, 5, 6]);
    assert_eq!(g.id, 6);
    assert!(g.instance.instances.is_empty());
    assert_eq!(g.tail.len(), 125);
}

#[test]
fn a_grass_component_reads_over_its_extent() {
    let mut body = grass_head(0x9C95_4A88);
    body.extend(mesh_instance());
    body.extend(visual_tail());
    body.extend(REFERENCE); // the library
    body.extend(11u64.to_le_bytes());
    body.extend(0x3BE0_5B93u32.to_le_bytes());
    let settings = [0u8; 300];
    body.extend(settings);
    body.extend(2u32.to_le_bytes());
    body.push(0);
    body.extend(0x2_0000_0010u64.to_le_bytes());
    body.push(1);
    body.extend(0x2_0000_0011u64.to_le_bytes());
    let components = component(GRASS_COMPONENT, 0x2_0000_0001, &body);
    let e = StoredEntity::parse(&entity_body(1, &components), is_component_type).expect("entity");
    assert_eq!(e.components.len(), 1);
    let ComponentBody::GrassSetup(g) = &e.components[0].body else {
        panic!("not a GrassSetup: {:?}", e.components[0].body);
    };
    assert_eq!(g.id, 6);
    assert_eq!(g.settings_id, 11);
    assert_eq!(g.settings, settings.to_vec());
    assert_eq!(g.entities, vec![0x2_0000_0010, 0x2_0000_0011]);
    assert!(g.library.is_some());
    assert!(g.instance_extra.is_empty());
}
