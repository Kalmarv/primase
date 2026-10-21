//! Real Entities placed into a world through an archive of Riders Republic's forges (searched patch first: a stated
//! order), then their meshes and levels lowered under the pass manager with its checks on. Instance counts and scales
//! as the Riders Republic placement notes give them (docs/placement.md §14).
use primase_anvil::components::{
    AnvilCollisionShape, AnvilLods, AnvilMaterialOverrides, AnvilMesh, AnvilMeshShape, ShapeGeometry,
};
use primase_anvil::passes::{
    LowerAnvilCollision, LowerAnvilLods, LowerAnvilMesh, LowerAnvilShapes, MeshNormals, SurfaceBy, UvV,
    UvWords,
};
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::entity::{ComponentBody, ENTITY, ENTITY_GROUP, StoredEntity, StoredEntityGroup};
use primase_anvil::read::placements::{InstancedCollision, InstancedLevels, Placed, Placer};
use primase_anvil::read::shape::{BOX_SHAPE, CAPSULE_SHAPE, REFERENCE_LIST_SHAPE, SPHERE_SHAPE};
use primase_core::frame::SourceFrame;
use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{Collider, Instance, Lods, Mesh, Name, Surfaces, Transform, World};
use primase_testdata::real_data;

/// The cube root of a transform's determinant: its uniform scale where it is one.
fn scale(t: &Transform) -> f64 {
    t.determinant().cbrt()
}

#[test]
fn fixtures_place_and_lower() {
    let Some(dir) = real_data("PRIMASE_ANVIL_DIR") else {
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
    // the aspen's RigidBody: an instanced Entity's collision isn't placed
    assert_eq!((placed.collisions.len(), placed.instanced_collision), (0, 1));
    // plastic cups: a RigidBody whose shape is a MeshShape, placed once at the Entity's transform
    let before = placed.placements.len();
    placer
        .place(
            At {
                forge: 1,
                entry: 47_242,
            },
            &mut placed,
        )
        .expect("place");
    let cups = placed.placements[before];
    let collision = *placed.collisions.last().expect("collision");
    let shape = placer
        .world()
        .get::<Instance>(collision)
        .expect("placed")
        .prototype;
    assert!(placer.world().has::<AnvilMeshShape>(shape));
    assert_eq!(
        placer.world().get::<Transform>(collision),
        placer.world().get::<Transform>(cups)
    );
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
                uv_words: UvWords::Signed,
                uv_divisor: 2048.0,
                uv_v: UvV::AsStored,
                normals: MeshNormals::Packed127,
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
    let Some(dir) = real_data("PRIMASE_ANVIL_DIR") else {
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
    // its RigidBody's shape, a ReferenceListShape: each child placed
    assert!(placed.shapes_unread.is_empty() && !placed.collisions.is_empty());
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
    let Some(dir) = real_data("PRIMASE_ANVIL_DIR") else {
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
    let mut kinds = std::collections::BTreeMap::<&str, u32>::new();
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
                *kinds.entry("mesh").or_default() += 1;
            } else {
                match w.get::<AnvilCollisionShape>(prototype).expect("a shape").geometry {
                    ShapeGeometry::Sphere { centre, radius } => {
                        points.push(apply(tr, centre));
                        grow = f64::from(radius);
                        *kinds.entry("sphere").or_default() += 1;
                    }
                    ShapeGeometry::Capsule { a, b, radius } => {
                        points.extend([apply(tr, a), apply(tr, b)]);
                        grow = f64::from(radius);
                        *kinds.entry("capsule").or_default() += 1;
                    }
                    ShapeGeometry::Box { half_extents: e } => {
                        points.extend((0..8).map(|i| {
                            apply(tr, [0, 1, 2].map(|a| if i >> a & 1 == 0 { -e[a] } else { e[a] }))
                        }));
                        *kinds.entry("box").or_default() += 1;
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
        ["mesh", "sphere", "capsule", "box"]
            .iter()
            .all(|k| kinds.contains_key(k)),
        "{kinds:?}"
    );
    assert!(
        outside.iter().all(|n| n.contains("PinyonPine_2M_A")),
        "{outside:?}"
    );
    // the shapes lowered to core collision: a collider per primitive prototype, the meshes' collision
    let primitives: u32 = ["sphere", "capsule", "box"].iter().map(|k| kinds[k]).sum();
    let prototypes = w.iter::<AnvilCollisionShape>().count();
    assert!(prototypes > 0 && prototypes <= primitives as usize);
    let surface = "rr-diagnostic-unknown".to_owned();
    let pipeline = Pipeline {
        passes: vec![
            Box::new(LowerAnvilShapes {
                by: SurfaceBy::CollisionMaterial,
                surface: surface.clone(),
            }),
            Box::new(LowerAnvilCollision {
                by: SurfaceBy::CollisionMaterial,
                surface,
            }),
        ],
    };
    let manager = PassManager::new(PassRegistry::new()).with_pass_checks(true);
    let mut report = RunReport::default();
    manager.run(&mut w, &pipeline, &mut report).expect("lower");
    assert_eq!(w.iter::<Collider>().count(), prototypes);
    // their surfaces: the game's CollisionMaterials by name (CM_...), every shape and face of these resolving to one
    let surfaces = w.resource::<Surfaces>().expect("surfaces");
    let names: Vec<&str> = surfaces.0.iter().map(|s| s.name.as_str()).collect();
    assert!(
        names
            .iter()
            .all(|n| n.starts_with("CM_") || *n == "rr-diagnostic-unknown"),
        "{names:?}"
    );
    assert!(names.len() > 3, "{names:?}");
    assert_eq!(
        w.iter::<AnvilCollisionShape>().count() + w.iter::<AnvilMeshShape>().count(),
        0
    );
}

/// The aspen's RigidBody, its collision placed at each of its 10 instances where the caller states so.
#[test]
fn instanced_collision_is_placed_per_instance_as_stated() {
    let Some(dir) = real_data("PRIMASE_ANVIL_DIR") else {
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
    let mut placer = Placer::new(
        &mut archive,
        &mut w,
        SourceFrame::WORLD,
        InstancedLevels::Selector,
    )
    .with_instanced_collision(InstancedCollision::PerInstance);
    placer
        .place(At { forge: 1, entry: 2 }, &mut placed)
        .expect("place");
    let n = placed.collisions.len();
    assert!(
        n >= 10 && n % 10 == 0 && placed.instanced_collision == 0,
        "{n} collisions"
    );
    // each instance's shapes at that instance's transform: the first part's translation differs instance by instance
    let per = n / 10;
    let at = |k: usize| {
        placer
            .world()
            .get::<Transform>(placed.collisions[k * per])
            .expect("transform")
            .cols[3]
    };
    assert!((1..10).all(|k| at(k) != at(0)));
}

/// A group some of whose meshes can't be read (MAD_GRP_CITYSCHOOL_Building_Dormitory_A_015: Meshes whose tail states
/// a range's vertex count other than the range's, 441 against 401 and 72 against 24): they are recorded and left
/// out, the group's other Visuals and its collision placed.
#[test]
fn a_refused_mesh_leaves_out_only_what_needs_it() {
    let Some(dir) = real_data("PRIMASE_ANVIL_DIR") else {
        return;
    };
    let order = [
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_patch_01.forge",
        "DataPC.forge",
    ];
    let mut archive = Archive::open(&order.map(|f| dir.join(f))).expect("archive");
    let name = "MAD_GRP_CITYSCHOOL_Building_Dormitory_A_015";
    let at = (0..2)
        .find_map(|forge| {
            let names = &archive.forge(forge)?.names;
            names
                .iter()
                .position(|n| n.name == name)
                .map(|entry| At { forge, entry })
        })
        .expect(name);
    let mut w = World::new();
    let mut placed = Placed::default();
    let mut placer = Placer::new(
        &mut archive,
        &mut w,
        SourceFrame::WORLD,
        InstancedLevels::Selector,
    );
    placer.place(at, &mut placed).expect("place");
    // two of its meshes (entries 97266 and 97267 of the patch), both refused for a tail range's vertex count
    assert!(
        !placed.refused.is_empty() && placed.refused.iter().all(|(_, why)| why.contains("tail ranges")),
        "{:?}",
        placed.refused
    );
    assert!(
        placed.placements.len() > 8,
        "{} placements",
        placed.placements.len()
    );
    assert!(!placed.collisions.is_empty());
}
