//! `lower-anvil-mesh`, `lower-anvil-rig` and `lower-anvil-skin` on a world built by hand (a rig of two bones, a mesh of
//! three vertices bound to them, a placement following the rig), and on real Entities of Riders Republic's forges placed
//! through the Placer: the joint and weight streams, the palette, the inverse binds, the occurrences and the placements
//! that follow them, under the pass manager with its checks on.
use primase_anvil::components::{
    AnvilMesh, AnvilRig, AnvilRigOccurrence, AnvilSkin, AnvilSkinned, RigBone, StoredMeshBone, StoredRig,
    StoredSkin, VertexAttributes,
};
use primase_anvil::passes::{
    LowerAnvilLods, LowerAnvilMesh, LowerAnvilRig, LowerAnvilSkin, OwnTransform, Static16Normal,
};
use primase_anvil::read::archive::Archive;
use primase_anvil::read::listing::Kind;
use primase_anvil::read::placements::{InstancedLevels, Placed, Placer};
use primase_core::frame::SourceFrame;
use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::rig::{RigInstance, SkinInstance};
use primase_core::skeleton::{JointId, Skeleton};
use primase_core::skin::{BindingId, Influences, Skin, WeightRule};
use primase_core::{Entity, Instance, Mesh, Semantic, Stream, Transform, World};
use primase_testdata::{Game, install};

fn rig_bone(name_hash: u32, parent: Option<u32>, z: f32) -> RigBone {
    RigBone {
        name_hash,
        parent,
        local_position: [0.0, 0.0, z, 0.0],
        local_rotation: [0.0, 0.0, 0.0, 1.0],
        skeleton: 0,
    }
}

fn mesh_bone(bone_id: u32, translation: [f32; 3]) -> StoredMeshBone {
    StoredMeshBone {
        bone_id,
        initial_matrix_inverse: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [translation[0], translation[1], translation[2], 1.0],
        ],
        used_by_submeshes: true,
    }
}

/// A mesh of three vertices and one triangle; its bones as given and the skin stream of 4 influences a vertex.
fn anvil_mesh(frame: SourceFrame, bones: Vec<StoredMeshBone>, slots: Vec<u8>, weights: Vec<u8>) -> AnvilMesh {
    AnvilMesh {
        entry: 0,
        index_id: 0,
        frame,
        positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        triangles: vec![[0, 1, 2]],
        ranges: vec![0],
        materials: vec![0],
        material_entities: Vec::new(),
        kind: 2,
        stride: 0,
        vertex_records: Vec::new(),
        attributes: VertexAttributes::default(),
        skin: Some(AnvilSkin {
            bones,
            skin: StoredSkin {
                influences: 4,
                slots,
                weights,
            },
        }),
    }
}

/// The slots and weights of the three vertices: vertex 0 wholly on slot 0 (slot 1 stored with no weight), vertex 1
/// half on slot 1 and half on slot 0 (a slot 9 with no weight, past the 2 bones), vertex 2 wholly on slot 0.
const SLOTS: [u8; 12] = [0, 1, 0, 0, 1, 0, 0, 9, 0, 0, 0, 0];
const WEIGHTS: [u8; 12] = [255, 0, 0, 0, 128, 127, 0, 0, 255, 0, 0, 0];

struct Built {
    w: World,
    rig: Entity,
    mesh: Entity,
    occurrence: Entity,
    placement: Entity,
}

/// A rig of bones 0xA and 0xB (B under A, 2 up), a mesh bound to `bones`, and a placement of it following the rig.
fn built(frame: SourceFrame, bones: Vec<StoredMeshBone>, slots: &[u8], weights: &[u8]) -> Built {
    let mut w = World::new();
    let rig = w.spawn();
    w.insert(
        rig,
        AnvilRig {
            frame,
            position: [0.0; 4],
            rotation: [0.0, 0.0, 0.0, 1.0],
            rig: StoredRig {
                bones: vec![rig_bone(0xA, None, 0.0), rig_bone(0xB, Some(0xA), 2.0)],
            },
        },
    );
    let mesh = w.spawn();
    w.insert(mesh, anvil_mesh(frame, bones, slots.to_vec(), weights.to_vec()));
    let occurrence = w.spawn();
    w.insert(occurrence, AnvilRigOccurrence { rig });
    w.insert(occurrence, Transform::IDENTITY);
    let placement = w.spawn();
    w.insert(
        placement,
        Instance {
            prototype: mesh,
            variant: 0,
        },
    );
    w.insert(placement, Transform::IDENTITY);
    w.insert(placement, AnvilSkinned { occurrence });
    Built {
        w,
        rig,
        mesh,
        occurrence,
        placement,
    }
}

fn lower(w: &mut World) -> Result<(), String> {
    let manager = PassManager::new(PassRegistry::new()).with_pass_checks(true);
    let pipeline = Pipeline {
        passes: vec![
            Box::new(LowerAnvilMesh {
                static16_normal: Static16Normal::Omit,
            }),
            Box::new(LowerAnvilRig {
                own_transform: OwnTransform::Refuse,
            }),
            Box::new(LowerAnvilSkin),
        ],
    };
    manager
        .run(w, &pipeline, &mut RunReport::default())
        .map_err(|e| format!("{e:#}"))
}

#[test]
fn a_mesh_is_bound_to_the_joints_its_bones_are_named_for() {
    // the mesh lists bone B then A, so slot 0 is B
    let mut b = built(
        SourceFrame::WORLD,
        vec![mesh_bone(0xB, [0.0, 0.0, -2.0]), mesh_bone(0xA, [0.0, 0.0, 0.0])],
        &SLOTS,
        &WEIGHTS,
    );
    lower(&mut b.w).expect("lower");
    let skeleton = b.w.get::<Skeleton>(b.rig).expect("skeleton");
    assert_eq!(skeleton.joints.len(), 2);
    assert_eq!(
        b.w.get::<RigInstance>(b.occurrence).map(|r| r.skeleton),
        Some(b.rig)
    );
    assert_eq!(
        b.w.get::<SkinInstance>(b.placement).map(|s| (s.rig, s.binding)),
        Some((b.occurrence, BindingId(0)))
    );
    let skin = b.w.get::<Skin>(b.mesh).expect("skin");
    assert_eq!(skin.weights, WeightRule::SumsToOne);
    assert_eq!(skin.influences, vec![Some(Influences::Streams { sets: 1 })]);
    let binding = skin.binding(BindingId(0)).expect("binding");
    assert_eq!(binding.skeleton, b.rig);
    let palette = binding.palettes[0].as_ref().expect("palette");
    assert_eq!(
        palette.iter().map(|s| s.joint).collect::<Vec<_>>(),
        [JointId(1), JointId(0)]
    );
    // the stored rows are the affine map's columns: the translation row is its translation
    assert_eq!(palette[0].inverse_bind.cols[3], [0.0, 0.0, -2.0]);
    assert_eq!(palette[0].inverse_bind.cols[2], [0.0, 0.0, 1.0]);
    // the streams: slots and weights as stored, a slot without a weight written as 0
    let mesh = b.w.get::<Mesh>(b.mesh).expect("mesh");
    let streams = &mesh.primitives()[0].streams;
    assert_eq!(
        streams[&Semantic::Joints(0)],
        Stream::U8x4(vec![[0, 0, 0, 0], [1, 0, 0, 0], [0, 0, 0, 0]])
    );
    assert_eq!(
        streams[&Semantic::Weights(0)],
        Stream::U8x4(vec![[255, 0, 0, 0], [128, 127, 0, 0], [255, 0, 0, 0]])
    );
    assert!(b.w.get::<AnvilSkinned>(b.placement).is_none());
}

#[test]
fn the_reader_frame_converts_the_inverse_binds() {
    // ENU: the source's x east, y north, z up become x, z, -y
    let mut b = built(
        SourceFrame::ENU,
        vec![mesh_bone(0xB, [0.0, 0.0, -2.0]), mesh_bone(0xA, [0.0, 0.0, 0.0])],
        &SLOTS,
        &WEIGHTS,
    );
    lower(&mut b.w).expect("lower");
    let skin = b.w.get::<Skin>(b.mesh).expect("skin");
    let palette = skin.bindings[0].palettes[0].as_ref().expect("palette");
    assert_eq!(palette[0].inverse_bind.cols[3], [0.0, -2.0, 0.0]);
}

#[test]
fn a_slot_without_a_bone_is_the_first_joint_if_no_vertex_weights_it_and_an_error_if_one_does() {
    // bone 0xC isn't in the rig: slot 1, weighted by none of the vertices' influences
    let mut b = built(
        SourceFrame::WORLD,
        vec![mesh_bone(0xA, [0.0; 3]), mesh_bone(0xC, [1.0, 2.0, 3.0])],
        &[0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        &[255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0],
    );
    lower(&mut b.w).expect("lower");
    let skin = b.w.get::<Skin>(b.mesh).expect("skin");
    let palette = skin.bindings[0].palettes[0].as_ref().expect("palette");
    assert_eq!(palette[1].joint, JointId(0));
    assert_eq!(palette[1].inverse_bind.cols[3], [1.0, 2.0, 3.0]);
    // the same with a vertex weighting slot 1
    let mut b = built(
        SourceFrame::WORLD,
        vec![mesh_bone(0xA, [0.0; 3]), mesh_bone(0xC, [0.0; 3])],
        &SLOTS,
        &WEIGHTS,
    );
    let e = lower(&mut b.w).expect_err("refused");
    assert!(e.contains("slot 1") && e.contains("0000000c"), "{e}");
}

#[test]
fn a_mesh_that_follows_no_rig_is_drawn_without_its_skin_streams() {
    let mut b = built(
        SourceFrame::WORLD,
        vec![mesh_bone(0xA, [0.0; 3]), mesh_bone(0xB, [0.0; 3])],
        &SLOTS,
        &WEIGHTS,
    );
    // a second skinned mesh no placement follows a rig for
    let loose = b.w.spawn();
    b.w.insert(
        loose,
        anvil_mesh(
            SourceFrame::WORLD,
            vec![mesh_bone(0xA, [0.0; 3]), mesh_bone(0xB, [0.0; 3])],
            SLOTS.to_vec(),
            WEIGHTS.to_vec(),
        ),
    );
    lower(&mut b.w).expect("lower");
    assert!(b.w.get::<Skin>(b.mesh).is_some());
    assert!(b.w.get::<Skin>(loose).is_none());
    let streams = &b.w.get::<Mesh>(loose).expect("mesh").primitives()[0].streams;
    assert!(
        streams
            .keys()
            .all(|s| !matches!(s, Semantic::Joints(_) | Semantic::Weights(_)))
    );
}

/// The Entity named `name`.
fn entity(archive: &Archive, name: &str) -> primase_anvil::read::archive::At {
    archive
        .listing()
        .find(|(_, l)| l.kind == Some(Kind::Entity) && l.name == name)
        .map(|(forge, l)| primase_anvil::read::archive::At {
            forge,
            entry: l.entry,
        })
        .unwrap_or_else(|| panic!("no Entity {name}"))
}

/// Real Entities that own a SkeletonComponent, placed and lowered with the checks on: an animal (its main Skeleton
/// alone) and a bike cinematic (a main Skeleton and six addons).
#[test]
fn real_skeleton_components_place_bind_and_lower() {
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
    let mut report = Vec::new();
    for name in ["MAD_ANM_Lizard", "MAD_CIN_DH_Bike_A_001"] {
        let at = entity(&archive, name);
        let mut w = World::new();
        let mut placed = Placed::default();
        let mut placer = Placer::new(
            &mut archive,
            &mut w,
            SourceFrame::WORLD,
            InstancedLevels::Selector,
        );
        placer.place(at, &mut placed).expect("place");
        assert!(placed.refused.is_empty(), "{name}: {:?}", placed.refused);
        let occurrences = w.iter::<AnvilRigOccurrence>().count();
        let follows = w.iter::<AnvilSkinned>().count();
        assert_eq!(occurrences, 1, "{name}");
        assert!(follows > 0, "{name}");
        let manager = PassManager::new(PassRegistry::new()).with_pass_checks(true);
        let pipeline = Pipeline {
            passes: vec![
                Box::new(LowerAnvilMesh {
                    static16_normal: Static16Normal::Omit,
                }),
                Box::new(LowerAnvilLods),
                Box::new(LowerAnvilRig {
                    own_transform: OwnTransform::Compose,
                }),
                Box::new(LowerAnvilSkin),
            ],
        };
        manager
            .run(&mut w, &pipeline, &mut RunReport::default())
            .unwrap_or_else(|e| panic!("{name}: {e:#}"));
        let skeletons = w.iter::<Skeleton>().count();
        let skins = w.iter::<Skin>().count();
        let instances = w.iter::<SkinInstance>().count();
        assert_eq!(skeletons, 1, "{name}");
        assert!(skins > 0 && instances > 0, "{name}");
        // every binding's palette has a slot for each bone of its mesh
        for (e, skin) in w.iter::<Skin>() {
            let mesh = w.get::<Mesh>(e).expect("a skin on a mesh");
            for b in &skin.bindings {
                assert_eq!(b.palettes.len(), mesh.primitives().len(), "{name}");
            }
        }
        let joints = w.iter::<Skeleton>().next().map_or(0, |(_, s)| s.joints.len());
        report.push(format!(
            "{name}: {follows} placements follow the rig, {skins} skinned meshes, {instances} skin instances, {joints} joints"
        ));
    }
    println!("{}", report.join("\n"));
}
