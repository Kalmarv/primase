//! Real Entities placed into a world through an archive of Riders Republic's forges (searched patch first: a stated
//! order), then their meshes and levels lowered under the pass manager with its checks on. Instance counts and scales
//! as the Riders Republic placement notes give them (docs/placement.md §14).
use primase_anvil::components::{
    AnvilCollisionShape, AnvilLods, AnvilMaterialOverrides, AnvilMesh, AnvilMeshShape, ShapeGeometry,
};
use primase_anvil::passes::{LowerAnvilLods, LowerAnvilMesh, Static16Normal};
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::entity::{ComponentBody, ENTITY, ENTITY_GROUP, StoredEntity, StoredEntityGroup};
use primase_anvil::read::placements::{InstancedLevels, Placed, Placer};
use primase_anvil::read::shape::{BOX_SHAPE, CAPSULE_SHAPE, REFERENCE_LIST_SHAPE, SPHERE_SHAPE};
use primase_core::frame::SourceFrame;
use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{Instance, Lods, Mesh, Name, Transform, World};
use primase_testdata::{Game, install};

/// The cube root of a transform's determinant: its uniform scale where it is one.
fn scale(t: &Transform) -> f64 {
    t.determinant().cbrt()
}

#[test]
fn fixtures_place_and_lower() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let order = [
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_patch_01.forge",
        "DataPC.forge",
    ];
    let mut archive = Archive::open(&order.map(|f| dir.join(f))).expect("archive");
    let mut w = World::new();
    let mut placed = Placed::default();
    let mut placer = Placer::new(&mut archive, &mut w, SourceFrame::WORLD, InstancedLevels::Slot);
    // an instanced aspen, 10 instances, slots MMMI-: under Slot, level 0 at each
    placer
        .place(At { forge: 1, entry: 2 }, &mut placed)
        .expect("place");
    assert_eq!(placed.placements.len(), 10);
    let prototype = placer
        .world()
        .get::<Instance>(placed.placements[0])
        .expect("placed")
        .prototype;
    assert!(
        placed
            .placements
            .iter()
            .all(|&p| placer.world().get::<Instance>(p).map(|i| i.prototype) == Some(prototype))
    );
    // level 0's mesh, its slot holding the matrices; levels 1 and 2 (slots holding none) not placed
    assert!(placer.world().has::<AnvilMesh>(prototype) && !placer.world().has::<AnvilLods>(prototype));
    assert_eq!((placed.impostor_levels, placed.levels_unplaced), (1, 2));
    let scales: Vec<f64> = placed.placements[..3]
        .iter()
        .map(|&p| scale(placer.world().get::<Transform>(p).expect("transform")))
        .collect();
    for (found, noted) in scales.iter().zip([1.13, 1.03, 1.15]) {
        assert!((found - noted).abs() < 0.01, "{scales:?}");
    }
    // a plain decal, scale 2.2497, one placement
    let before = placed.placements.len();
    placer
        .place(
            At {
                forge: 1,
                entry: 50_890,
            },
            &mut placed,
        )
        .expect("place");
    assert_eq!(placed.placements.len(), before + 1);
    let decal = scale(
        placer
            .world()
            .get::<Transform>(placed.placements[before])
            .expect("transform"),
    );
    assert!((decal - 2.2497).abs() < 1e-3, "{decal}");
    // a flag, not instanced, slots MMM--, one material-info entry per slot
    let before = placed.placements.len();
    placer
        .place(
            At {
                forge: 1,
                entry: 354_769,
            },
            &mut placed,
        )
        .expect("place");
    assert_eq!(placed.placements.len(), before + 1);
    let flag = placed.placements[before];
    let whole = placer.world().get::<Instance>(flag).expect("placed").prototype;
    let lods = placer.world().get::<AnvilLods>(whole).expect("levels").clone();
    assert_eq!(
        (lods.detail, lods.forms.iter().map(|f| f.0).collect::<Vec<_>>()),
        (0, vec![1, 2])
    );
    let overrides = placer
        .world()
        .get::<AnvilMaterialOverrides>(flag)
        .expect("overrides");
    assert_eq!(
        overrides
            .levels
            .iter()
            .map(|(l, p)| (*l, p.len()))
            .collect::<Vec<_>>(),
        vec![(0, 1), (1, 1), (2, 1)]
    );
    // the meshes lowered, then the levels
    let manager = PassManager::new(PassRegistry::new()).with_pass_checks(true);
    let pipeline = Pipeline {
        passes: vec![
            Box::new(LowerAnvilMesh {
                static16_normal: Static16Normal::P,
            }),
            Box::new(LowerAnvilLods),
        ],
    };
    let mut report = RunReport::default();
    manager.run(&mut w, &pipeline, &mut report).expect("lower");
    assert!(w.has::<Mesh>(prototype) && !w.has::<AnvilMesh>(prototype));
    assert!(w.has::<Mesh>(whole) && !w.has::<AnvilLods>(whole));
    let lods = w.get::<Lods>(whole).expect("core levels");
    assert_eq!((lods.detail, lods.forms.len(), lods.default), (0, 2, None));

    // under Selector, the aspen's every mesh level at each instance
    let mut w = World::new();
    let mut placed = Placed::default();
    let mut placer = Placer::new(
        &mut archive,
        &mut w,
        SourceFrame::WORLD,
        InstancedLevels::Selector,
    );
    placer
        .place(At { forge: 1, entry: 2 }, &mut placed)
        .expect("place");
    assert_eq!((placed.placements.len(), placed.levels_unplaced), (10, 0));
    let prototype = placer
        .world()
        .get::<Instance>(placed.placements[0])
        .expect("placed")
        .prototype;
    let lods = placer.world().get::<AnvilLods>(prototype).expect("levels");
    assert_eq!(lods.forms.iter().map(|f| f.0).collect::<Vec<_>>(), vec![1, 2]);
}

/// The entry named `name` in the archive's forge `forge`.
fn named(archive: &Archive, forge: usize, name: &str) -> At {
    let names = &archive.forge(forge).expect("forge").names;
    let entry = names.iter().position(|n| n.name == name).expect(name);
    At { forge, entry }
}

#[test]
fn groups_place_their_own_visuals_and_report_members() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    // a USNAT group of instanced signs: Sign_NoEntry x 2 and Sign_Parkingtime_B x 1, no members (docs/world.md §7)
    let order = ["DataPC_MAD_USNAT.forge", "DataPC.forge"];
    let mut archive = Archive::open(&order.map(|f| dir.join(f))).expect("archive");
    let at = named(&archive, 0, "MAD_GRP_CITYSCHOOL_Parking_Campus_A_016_Short");
    let mut w = World::new();
    let mut placed = Placed::default();
    let mut placer = Placer::new(
        &mut archive,
        &mut w,
        SourceFrame::WORLD,
        InstancedLevels::Selector,
    );
    placer.place(at, &mut placed).expect("place");
    assert_eq!((placed.placements.len(), placed.members.len()), (3, 0));
    // WhiteRoom's light group: no components, three member Entities listed by no cell (docs/placement.md §5)
    let mut archive = Archive::open(&[dir.join("DataPC_MAD_WhiteRoom.forge")]).expect("archive");
    let at = named(&archive, 0, "Light_Spot_CharaGroup");
    let mut placed = Placed::default();
    let mut placer = Placer::new(
        &mut archive,
        &mut w,
        SourceFrame::WORLD,
        InstancedLevels::Selector,
    );
    placer.place(at, &mut placed).expect("place");
    assert_eq!((placed.placements.len(), placed.members.len()), (0, 3));
}

/// `t` applied to `p`, in f64.
fn apply(t: &Transform, p: [f32; 3]) -> [f64; 3] {
    let c = t.cols.map(|c| c.map(f64::from));
    let p = p.map(f64::from);
    [0, 1, 2].map(|r| c[0][r] * p[0] + c[1][r] * p[1] + c[2][r] * p[2] + c[3][r])
}

/// The Entities and groups of DataPC_MAD_USNAT whose RigidBody names a Box, Sphere, Capsule or ReferenceList shape
/// (the first 3,000 with scale 1), placed: each collision placement's shape, under its transform, lies within its
/// owner's stored bounds (translation + box) within 1e-2 m. Measured: 1,649 of 1,654 (31 boxes, 436 capsules, 531
/// spheres, 656 meshes of reference lists); the 5 others all belong to MAD_VEG_SECONDARY_PinyonPine_2M_A, the
/// reference list the Riders Republic notes find stored with a box that ignores its children's matrices.
#[test]
fn collision_lies_within_its_owners_bounds() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let order = [
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_patch_01.forge",
        "DataPC.forge",
    ];
    let mut archive = Archive::open(&order.map(|f| dir.join(f))).expect("archive");
    let mut owners = Vec::new();
    let n = archive.forge(1).expect("forge").names.len();
    for entry in 0..n {
        let kind = archive.forge(1).expect("forge").names[entry].type_id;
        if kind != ENTITY && kind != ENTITY_GROUP {
            continue;
        }
        let Ok(r) = archive.resource(At { forge: 1, entry }) else {
            continue;
        };
        let e = if kind == ENTITY_GROUP {
            StoredEntityGroup::parse(&r.body).ok().map(|g| g.entity)
        } else {
            StoredEntity::parse(&r.body).ok()
        };
        let Some(e) = e.filter(|e| e.scale == 1.0) else {
            continue;
        };
        let primitive = e.components.iter().any(|c| match &c.body {
            ComponentBody::RigidBody { shape: Some(s), .. } => archive
                .find(s.id)
                .and_then(|at| archive.forge(at.forge)?.names.get(at.entry))
                .is_some_and(|nm| {
                    [BOX_SHAPE, SPHERE_SHAPE, CAPSULE_SHAPE, REFERENCE_LIST_SHAPE].contains(&nm.type_id)
                }),
            _ => false,
        });
        if primitive {
            let t = [e.matrix[12], e.matrix[13], e.matrix[14]];
            owners.push((At { forge: 1, entry }, t, e.bounds.min, e.bounds.max));
        }
        if owners.len() == 3000 {
            break;
        }
    }
    let mut w = World::new();
    let mut placed = Placed::default();
    let mut placer = Placer::new(
        &mut archive,
        &mut w,
        SourceFrame::WORLD,
        InstancedLevels::Selector,
    );
    let mut outside = Vec::new();
    for (at, t, min, max) in owners {
        let before = placed.collisions.len();
        if placer.place(at, &mut placed).is_err() {
            continue;
        }
        let w = placer.world();
        for &c in &placed.collisions[before..] {
            let tr = w.get::<Transform>(c).expect("transform");
            let prototype = w.get::<Instance>(c).expect("placed").prototype;
            let (mut points, mut grow) = (Vec::new(), 0.0);
            if let Some(m) = w.get::<AnvilMeshShape>(prototype) {
                points.extend(m.positions.iter().map(|&p| apply(tr, p)));
            } else {
                match w.get::<AnvilCollisionShape>(prototype).expect("a shape").geometry {
                    ShapeGeometry::Sphere { centre, radius } => {
                        points.push(apply(tr, centre));
                        grow = f64::from(radius);
                    }
                    ShapeGeometry::Capsule { a, b, radius } => {
                        points.extend([apply(tr, a), apply(tr, b)]);
                        grow = f64::from(radius);
                    }
                    ShapeGeometry::Box { half_extents: e } => {
                        points.extend((0..8).map(|i| {
                            apply(tr, [0, 1, 2].map(|a| if i >> a & 1 == 0 { -e[a] } else { e[a] }))
                        }));
                    }
                }
            }
            let past = points
                .iter()
                .flat_map(|p| {
                    (0..3).map(move |a| {
                        let lo = f64::from(t[a]) + f64::from(min[a]);
                        let hi = f64::from(t[a]) + f64::from(max[a]);
                        (lo - (p[a] - grow)).max(p[a] + grow - hi)
                    })
                })
                .fold(0.0f64, f64::max);
            if past > 1e-2 {
                outside.push(w.get::<Name>(c).map(|n| n.0.clone()).unwrap_or_default());
            }
        }
    }
    assert!(
        outside.iter().all(|n| n.contains("PinyonPine_2M_A")),
        "{outside:?}"
    );
}

/// A StaticDecalComponent placed as a Visual (docs/entity-grammar.md §9.1): MAD_MOD_AREA52_Radar_Dish_A_014's decal,
/// its LODSelector (the game's name for it: the owner's, then "StaticDecalComponent LodSelector") at the dish's own
/// matrix, beside the dish's Visual.
#[test]
fn a_static_decal_is_placed_with_its_owner() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let order = ["DataPC_MAD_USNAT.forge", "DataPC.forge"];
    let mut archive = Archive::open(&order.map(|f| dir.join(f))).expect("archive");
    let at = named(&archive, 0, "MAD_MOD_AREA52_Radar_Dish_A_014");
    // the selector the decal shows, by its name
    named(
        &archive,
        0,
        "MAD_MOD_AREA52_Radar_Dish_A_014 StaticDecalComponent LodSelector",
    );
    let mut w = World::new();
    let mut placed = Placed::default();
    let mut placer = Placer::new(
        &mut archive,
        &mut w,
        SourceFrame::WORLD,
        InstancedLevels::Selector,
    );
    placer.place(at, &mut placed).expect("place");
    assert_eq!((placed.placements.len(), placed.static_decals), (2, 1));
    let world = placer.world();
    let [dish, decal] = [placed.placements[0], placed.placements[1]];
    assert_eq!(world.get::<Transform>(dish), world.get::<Transform>(decal));
    let prototype = |p| world.get::<Instance>(p).expect("placed").prototype;
    assert_ne!(prototype(dish), prototype(decal));
    let geometry = |e| world.has::<AnvilMesh>(e) || world.has::<AnvilLods>(e);
    assert!(
        geometry(prototype(decal)),
        "the decal's prototype holds its geometry"
    );
}

/// An instanced EntityGroup Visual whose LODSelector's dependencies fill its first two levels:
/// MAD_HWIGRP_AREA52_VansPark_TowerBuildingD_Part2_01's MAD_MOD_AREA52_ResearchFacilityKit_Railings90d_A (levels
/// D D R R -, its five instance matrices in slot 2, slots 0 and 1 none). Under [`InstancedLevels::Selector`] each
/// instance places from level 2 on (its `_LOD2`, 204 triangles), the two filled levels counted unplaced; under
/// [`InstancedLevels::Every`] from level 0 (its `_LOD0`, 1,288 triangles), the others its forms. The meshes' ClassIDs
/// are the forge index's for `_LOD0` and `_LOD2`, and their triangle counts RRAssetStudio v3's (the
/// `oracles.rrassetstudio.riders/meshes` summary rows of those ClassIDs).
#[test]
fn instances_place_the_levels_dependencies_fill_under_every() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    const SELECTOR: u64 = 0x1E2_1A3D_63DF;
    const LOD0: u64 = 0x1E2_1A3D_63EC;
    const LOD2: u64 = 0x1E3_F07A_26E5;
    let order = ["DataPC_MAD_USNAT_patch_01.forge", "DataPC_MAD_USNAT.forge"];
    let mut found = Vec::new();
    for choice in [InstancedLevels::Selector, InstancedLevels::Every] {
        let mut archive = Archive::open(&order.map(|f| dir.join(f))).expect("archive");
        let at = named(&archive, 1, "MAD_HWIGRP_AREA52_VansPark_TowerBuildingD_Part2_01");
        let r = archive.resource(at).expect("resource");
        let g = StoredEntityGroup::parse(&r.body).expect("group");
        assert!(
            g.entity
                .visuals()
                .any(|v| v.object.map(|o| o.id) == Some(SELECTOR))
        );
        let mut w = World::new();
        let mut placed = Placed::default();
        let mut placer = Placer::new(&mut archive, &mut w, SourceFrame::WORLD, choice);
        placer.place(at, &mut placed).expect("place");
        let world = placer.world();
        let railings: Vec<_> = placed
            .placements
            .iter()
            .map(|&p| world.get::<Instance>(p).expect("placed").prototype)
            .filter(|&p| {
                world
                    .get::<Name>(p)
                    .is_some_and(|n| n.0 == "MAD_MOD_AREA52_ResearchFacilityKit_Railings90d_A")
            })
            .collect();
        assert_eq!(railings.len(), 5, "{choice:?}: one placement per instance");
        let p = railings[0];
        assert!(railings.iter().all(|&q| q == p), "one prototype");
        let mesh = world.get::<AnvilMesh>(p).expect("the finest level's mesh");
        let lods = world.get::<AnvilLods>(p).expect("its other levels");
        let forms: Vec<u8> = lods.forms.iter().map(|(l, _)| *l).collect();
        found.push((
            mesh.index_id,
            mesh.triangles.len(),
            lods.detail,
            forms,
            placed.dependency_levels_unplaced,
            placed.placements.len(),
        ));
    }
    let [selector, every] = [&found[0], &found[1]];
    assert_eq!(
        (selector.0, selector.1, selector.2, &selector.3),
        (LOD2, 204, 2, &vec![3])
    );
    assert_eq!(
        (every.0, every.1, every.2, &every.3),
        (LOD0, 1288, 0, &vec![1, 2, 3])
    );
    // the filled levels counted unplaced under Selector only; the same placements under both
    assert!(selector.4 >= 2);
    assert_eq!(every.4, 0);
    assert_eq!(selector.5, every.5);
}
