//! The Entity flags the Placer reads, counted over every Entity and EntityGroup body of Riders Republic's base and
//! USNAT forges with their patches (build 25394987), against the notes' census by the game's registry
//! (registry-named-fields §9, §10; placement.md §19): each RigidBody's `QueryFlags` slots, its `CollisionLayerBits`,
//! `IsGround`; the run's `IsPhantom` and `IsHidden` by Entity name; MeshInstanceData's `IsFake` with `FakeMeshIndex`
//! the FarFake cell's number. The census reads bodies this reader refuses (46,477 Entities, UI widgets most of them);
//! its counts bound this reader's while it refuses them, and every body reads now (the UI widgets too: the hidden
//! names are the 3D ones and the UI's).
use std::collections::{BTreeMap, BTreeSet};

use anvil_formats::archive::{Archive, At};
use anvil_formats::entity::{
    CollisionFilter, ComponentBody, ENTITY, ENTITY_GROUP, Instance, StoredEntity, StoredEntityGroup, query,
};
use primase_testdata::{Game, install};

#[test]
fn rigid_body_filters_and_entity_flags_count_as_the_registry_census() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let forges = [
        "DataPC.forge",
        "DataPC_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_MAD_USNAT_patch_01.forge",
    ];
    let (mut bodies, mut query_set, mut layer_set) = (0u64, [0u64; 34], [[0u64; 24]; 2]);
    let (mut ground, mut water_not_static) = (0u64, 0u64);
    let (mut phantom, mut hidden): (BTreeSet<String>, BTreeSet<String>) = Default::default();
    let mut fakes: BTreeMap<String, u32> = BTreeMap::new();
    let (mut refused, mut refused_bodies) = (BTreeMap::<String, u64>::new(), 0u64);
    let (mut fake_instances, mut fake_index_is_cell) = (0u64, 0u64);
    for file in forges {
        let mut archive = Archive::open(&[dir.join(file)]).expect("archive");
        let names = archive.forge(0).expect("forge").names.clone();
        for (entry, n) in names.iter().enumerate() {
            let e = match n.type_id {
                ENTITY => {
                    let body = archive.resource(At { forge: 0, entry }).expect("entity").body;
                    let Ok(e) = StoredEntity::parse(
                        &body,
                        primase_anvil::components::component_types::is_component_type,
                    ) else {
                        refused_bodies += body
                            .windows(4)
                            .filter(|w| *w == 0xC177_D702u32.to_le_bytes())
                            .count() as u64;
                        *refused
                            .entry(n.name.split('_').next().unwrap_or("").to_owned())
                            .or_default() += 1;
                        continue;
                    };
                    e
                }
                ENTITY_GROUP => {
                    let body = archive.resource(At { forge: 0, entry }).expect("group").body;
                    let Ok(g) = StoredEntityGroup::parse(
                        &body,
                        primase_anvil::components::component_types::is_component_type,
                    ) else {
                        refused_bodies += body
                            .windows(4)
                            .filter(|w| *w == 0xC177_D702u32.to_le_bytes())
                            .count() as u64;
                        *refused
                            .entry(n.name.split('_').next().unwrap_or("").to_owned())
                            .or_default() += 1;
                        continue;
                    };
                    g.entity
                }
                _ => continue,
            };
            if e.is_phantom() {
                phantom.insert(n.name.clone());
            }
            if e.is_hidden() {
                hidden.insert(n.name.clone());
            }
            for c in &e.components {
                let ComponentBody::RigidBody { bytes, .. } = &c.body else {
                    continue;
                };
                let f = CollisionFilter::parse(bytes).expect("every body's filter reads");
                bodies += 1;
                for (k, q) in query_set.iter_mut().enumerate() {
                    *q += u64::from(f.queries(k as u32));
                }
                for (set, &mask) in layer_set.iter_mut().zip(&f.layers) {
                    for (k, l) in set.iter_mut().enumerate() {
                        *l += u64::from(mask >> k & 1 == 1);
                    }
                }
                ground += u64::from(f.ground);
                water_not_static += u64::from(f.queries(query::WATER_LEVEL) && !f.queries(query::STATIC));
            }
            for v in e.visuals() {
                let Instance::Mesh(m) = &v.instance else {
                    continue;
                };
                if !m.is_fake() {
                    continue;
                }
                fake_instances += 1;
                fakes.insert(n.name.clone(), m.fake_mesh_index());
                // MAD_USNAT_FarFake_<n>_Cell...
                let cell = n
                    .name
                    .strip_prefix("MAD_USNAT_FarFake_")
                    .and_then(|r| r.split('_').next())
                    .and_then(|k| k.parse::<u32>().ok());
                fake_index_is_cell += u64::from(cell == Some(m.fake_mesh_index()) && !m.is_near_fake());
            }
        }
    }
    let mut top: Vec<_> = refused.iter().collect();
    top.sort_by_key(|(_, k)| std::cmp::Reverse(**k));
    println!(
        "refused {} entities, RigidBody kind words in them {refused_bodies}: {:?}",
        refused.values().sum::<u64>(),
        &top[..top.len().min(15)]
    );
    println!("hidden {hidden:?}");
    println!(
        "bodies {bodies}, query {query_set:?}, layers {layer_set:?}, ground {ground}, water not static \
         {water_not_static}; phantom {}, hidden {}; fake names {}, fake instances {fake_instances}, index = cell \
         {fake_index_is_cell}",
        phantom.len(),
        hidden.len(),
        fakes.len()
    );
    // The registry census (registry-named-fields §9, §10) reads 491,972 bodies; this reader's are those of the Entities
    // it parses, the rest in the bodies it refuses (their RigidBodyComponent kind words, counted raw)
    assert_eq!(bodies + refused_bodies, 491_972);
    // slots the census finds on bodies of parsed Entities alone match it exactly; every other at most its count
    assert_eq!(
        [
            query_set[10],
            query_set[15],
            query_set[16],
            layer_set[0][13],
            layer_set[1][3]
        ],
        [5_372, 1_182, 5_372, 5_372, 1_168]
    );
    let census: [u64; 34] = [
        486_510, 485_294, 49_168, 212_818, 0, 212_818, 16_167, 0, 350_870, 0, 5_372, 0, 0, 0, 20_336, 1_182,
        5_372, 11, 0, 491_926, 0, 0, 54, 0, 0, 0, 0, 0, 0, 0, 26, 22_798, 0, 0,
    ];
    for (k, (&ours, &theirs)) in query_set.iter().zip(&census).enumerate() {
        assert!(
            ours <= theirs,
            "query slot {k}: {ours} over the census's {theirs}"
        );
    }
    assert!(ground <= 33_960);
    // the run's IsHidden and IsPhantom (bytes 4 and 3), each name checked against the census's read of its body
    // (placement.md §19.1: these three are phantom and hidden in their base-forge copies). Every body reads now, the UI
    // widgets too (the census's "mostly UI"): of the hidden names the 3D ones are these three, and the names are at
    // least the 200 the census counts (one body per name; across the four forges' copies the names here are 438)
    assert_eq!(
        hidden
            .iter()
            .filter(|n| ["MAD_FX", "MAD_GRP", "MAD_PRP"].iter().any(|p| n.starts_with(p)))
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "MAD_FX_Mesh_UFOBeamSmall",
            "MAD_GRP_RRGENEVENT_CheckpointAerial_Flying15_A_001",
            "MAD_GRP_RRGENEVENT_CheckpointAerial_Flying15_A_002",
        ]
    );
    assert!(hidden.len() >= 200, "{} hidden names", hidden.len());
    assert!(hidden.is_subset(&phantom));
    for name in [
        "FX_MAD_CIN_SnowPuff_001",
        "AmbientEffect_TemplateEntity",
        "CONTAINER",
        "FAKE_Platform_Container_001",
    ] {
        assert!(phantom.contains(name), "{name} phantom");
    }
    assert_eq!(
        water_not_static, 5_372,
        "the water surfaces: WaterLevel without Static"
    );
    // placement.md §19.4: IsFake on the FarFake cells alone, FakeMeshIndex the cell's number on every one, never near
    assert!(fakes.keys().all(|k| k.starts_with("MAD_USNAT_FarFake_")));
    assert!(fakes.len() <= 1_031);
    assert_eq!(fake_index_is_cell, fake_instances);
}
