//! Real Entities read from byte 0 to their last byte (Riders Republic build 25394987): the placement fixtures of the
//! Riders Republic notes (docs/placement.md §14), each's scale, slots, instances and material infos as found there.
use std::fs::File;

use primase_anvil::read::data::{Resource, read_entry};
use primase_anvil::read::entity::{ENTITY, Instance, Slot, StoredEntity};
use primase_anvil::read::forge::Forge;
use primase_anvil::read::shapes::MAX_RECORD;
use primase_testdata::real_data;

/// Each LODSelectorInstance slot as a letter: M a MeshInstanceData, I an ImpostorInstanceData, - none.
fn slots(e: &StoredEntity) -> String {
    e.visuals()
        .filter_map(|v| match &v.instance {
            Instance::Lod { slots, .. } => Some(slots),
            _ => None,
        })
        .flatten()
        .map(|s| match s {
            Slot::None => '-',
            Slot::Mesh(_) => 'M',
            Slot::Impostor(_) => 'I',
        })
        .collect()
}

/// The first MeshInstanceData slot's instance count and material-info count.
fn first_mesh(e: &StoredEntity) -> (usize, usize) {
    e.visuals()
        .find_map(|v| match &v.instance {
            Instance::Lod { slots, .. } => slots.iter().find_map(|s| match s {
                Slot::Mesh(m) => Some((m.instances.len(), m.material_infos.len())),
                _ => None,
            }),
            Instance::Mesh(m) => Some((m.instances.len(), m.material_infos.len())),
            Instance::Raw { .. } | Instance::None => None,
        })
        .unwrap_or_default()
}

#[test]
fn placement_fixtures_read_whole() {
    let Some(dir) = real_data("PRIMASE_ANVIL_DIR") else {
        return;
    };
    // forge, entry, body bytes, scale, slots, (instances, material infos) of the first mesh slot
    let fixtures = [
        ("DataPC_MAD_USNAT.forge", 50_890, 532, 2.2497, "M----", (0, 0)),
        (
            "DataPC_MAD_USNAT_patch_01.forge",
            88_495,
            532,
            13.65,
            "M----",
            (0, 0),
        ),
        ("DataPC_MAD_USNAT.forge", 118_201, 532, 1.0, "--M--", (0, 0)),
        ("DataPC_MAD_USNAT.forge", 2, 3_135, 1.0, "MMMI-", (10, 0)),
        ("DataPC_MAD_USNAT.forge", 260_362, 806, 1.0, "MI---", (2, 0)),
        ("DataPC_MAD_USNAT.forge", 66_575, 2_927, 1.0, "MMMI-", (8, 0)),
        ("DataPC_MAD_USNAT.forge", 354_769, 919, 1.0, "MMM--", (0, 1)),
        ("DataPC_MAD_USNAT.forge", 47_242, 1_685, 1.0, "--M--", (0, 0)),
    ];
    let mut forges: Vec<(String, Forge, File)> = Vec::new();
    for (name, entry, length, scale, pattern, first) in fixtures {
        if !forges.iter().any(|(n, _, _)| n == name) {
            let path = dir.join(name);
            forges.push((
                name.to_owned(),
                Forge::open(&path).expect("forge"),
                File::open(&path).expect("open"),
            ));
        }
        let (_, forge, file) = forges.iter_mut().find(|(n, _, _)| n == name).expect("opened");
        let payload = forge.payload(file, entry).expect("payload");
        let block = read_entry(&payload, MAX_RECORD).expect("entry");
        let r = Resource::parse(&block.resource.data).expect("resource");
        assert_eq!((r.type_id, r.body.len()), (ENTITY, length), "{name}:{entry}");
        let e = StoredEntity::parse(&r.body).expect("entity");
        // to the precision the notes give it (13.65 to two places, 2.2497 to four)
        assert!(
            (e.scale - scale).abs() <= 5e-3,
            "{name}:{entry}: scale {}",
            e.scale
        );
        assert_eq!(slots(&e), pattern, "{name}:{entry}");
        assert_eq!(first_mesh(&e), first, "{name}:{entry}");
        // the matrix's last row is 0 0 0 1 (column-major: elements 3, 7, 11, 15)
        assert_eq!(
            [e.matrix[3], e.matrix[7], e.matrix[11]],
            [0.0; 3],
            "{name}:{entry}"
        );
    }
}
