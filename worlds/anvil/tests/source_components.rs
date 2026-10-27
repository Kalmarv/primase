use primase_anvil::components::{AnvilSourceComponents, AnvilSourceOwner, SourceComponent};
use primase_core::{Component, World};

#[test]
fn selecting_a_later_occurrence_keeps_one_source_payload_without_keeping_the_first_occurrence() {
    let mut world = World::new();
    let source = world.spawn();
    let first = world.spawn();
    let later = world.spawn();
    let bytes = vec![0x12, 0x34, 0x56];
    world.insert(
        source,
        AnvilSourceComponents {
            owner: 77,
            components: vec![SourceComponent {
                type_id: 0xD2BF_93A6,
                id: 88,
                status: "length-rule-only".into(),
                bytes: bytes.clone(),
            }],
        },
    );
    for occurrence in [first, later] {
        world.insert(occurrence, AnvilSourceOwner { source });
    }
    let keep = world.closure([later]);
    assert!(keep.contains(&source));
    assert!(!keep.contains(&first));
    let selected = world.subset(&keep);
    let data = selected
        .get::<AnvilSourceComponents>(source)
        .expect("source survives selection");
    assert_eq!(data.components.first().expect("component").bytes, bytes);
    assert_eq!(selected.iter::<AnvilSourceComponents>().count(), 1);
    let json = data.to_json().expect("source JSON").expect("typed source view");
    assert_eq!(json["components"][0]["type_id"], "0xd2bf93a6");
    assert_eq!(data.payload_bytes(), 3);
}
