use fromsoft_formats::flver::{Bone, FaceSet, Material, Member, Mesh, Model, Vertex};
use primase_core::{
    World,
    pass::{Pass, PassLog},
};
use primase_fromsoft::rigid::{ApplyRigidSkinning, RigidModel, SourceModel, lower_model};
/// One bone and one static mesh whose layout stores a normal with a W (`UByte4`), so NORMAL.w names each vertex's bone.
fn model() -> Model {
    Model {
        dummies: vec![],
        version: 0x2001a,
        speedtree: false,
        layouts: vec![vec![Member {
            unknown: 0,
            stored: true,
            offset: 0,
            kind: 0x13,
            semantic: 3,
            index: 0,
        }]],
        materials: vec![Material {
            name: "test".into(),
            mtd: String::new(),
            textures: vec![],
            gx: vec![],
        }],
        bones: vec![Bone {
            name: "root".into(),
            translation: [1., 2., 3.],
            rotation: [0.; 3],
            scale: [2., 3., 4.],
            parent: -1,
        }],
        meshes: vec![Mesh {
            dynamic: false,
            node: -1,
            palette: vec![],
            material: 0,
            layouts: vec![0],
            vertices: vec![Vertex {
                position: [1., 1., 1.],
                normal: [0., 1., 0.],
                normal_w: 0,
                tangents: vec![[1., 0., 0., 1.]],
                tangent_indices: vec![0],
                ..Default::default()
            }],
            faces: vec![],
        }],
    }
}
#[test]
fn rigid_points_normals_tangents_and_declared_pass() {
    let m = model();
    let mut world = World::default();
    let e = world.spawn();
    world.insert(e, SourceModel(m.clone()));
    let mut log = PassLog::default();
    let pass = ApplyRigidSkinning;
    assert_eq!(pass.requires(), ["fromsoft.flver"]);
    assert_eq!(pass.provides(), ["fromsoft.rigid_model"]);
    pass.run(&mut world, &mut log).unwrap();
    // the raw model stays beside the posed one
    assert_eq!(
        world.get::<SourceModel>(e).unwrap().0.meshes[0].vertices[0].position,
        [1.; 3]
    );
    let v = &world.get::<RigidModel>(e).unwrap().0.meshes[0].vertices[0];
    assert_eq!(v.position, [3., 5., 7.]);
    // the normal by the inverse transpose (0, 1/3, 0) and the tangent by the bone's part (2, 0, 0), each of unit length
    assert_eq!(v.normal, [0., 1., 0.]);
    assert_eq!(v.tangents[0], [1., 0., 0., 1.]);
    assert_eq!(m.meshes[0].vertices[0].position, [1.; 3]);
    assert_eq!(log.counts["rigid vertices transformed"], 1);
    pass.run(&mut world, &mut log).unwrap();
    assert_eq!(
        world.get::<RigidModel>(e).unwrap().0.meshes[0].vertices[0].position,
        [3., 5., 7.]
    );
}
#[test]
fn hierarchy_rotation_reflection_and_invalid_hierarchy() {
    let mut m = model();
    m.bones[0].scale = [-1., 1., 1.];
    m.bones[0].translation = [0.; 3];
    m.bones[0].parent = 1;
    m.bones.push(Bone {
        name: "parent".into(),
        translation: [0., 2., 0.],
        rotation: [0., std::f32::consts::FRAC_PI_2, 0.],
        scale: [1.; 3],
        parent: -1,
    });
    let v = m.meshes[0].vertices[0].clone();
    m.meshes[0].vertices.resize(3, v);
    m.meshes[0].faces.push(FaceSet {
        flags: 0,
        strip: false,
        cull: true,
        indices: vec![0, 1, 2],
    });
    let out = lower_model(&m, &mut PassLog::default()).unwrap();
    let v = &out.meshes[0].vertices[0];
    for (a, b) in v.position.into_iter().zip([1., 3., 1.]) {
        assert!((a - b).abs() < 1e-6);
    }
    assert_eq!(v.tangents[0][3], -1.);
    assert_eq!(out.meshes[0].faces[0].indices, [0, 2, 1]);
    m.bones[1].parent = 0;
    assert!(lower_model(&m, &mut PassLog::default()).is_err());
    m.bones[1].parent = 99;
    assert!(lower_model(&m, &mut PassLog::default()).is_err());
}
#[test]
fn non_rigid_weights_counted_and_world_not_partially_changed() {
    let mut bad = model();
    bad.meshes[0].vertices[0].bone_weights = [0.5, 0.5, 0., 0.];
    let mut world = World::default();
    let a = world.spawn();
    world.insert(a, SourceModel(model()));
    let b = world.spawn();
    world.insert(b, SourceModel(bad.clone()));
    let mut log = PassLog::default();
    assert!(ApplyRigidSkinning.run(&mut world, &mut log).is_err());
    assert_eq!(log.counts["refused non-rigid vertices"], 1);
    assert!(world.get::<SourceModel>(a).is_some());
    assert!(world.get::<RigidModel>(a).is_none());
    bad.meshes[0].vertices[0].bone_weights = [0.999, 0., 0., 0.];
    assert!(lower_model(&bad, &mut PassLog::default()).is_err());
    bad.meshes[0].vertices[0].bone_weights = [1., 0., 0., 0.];
    assert!(lower_model(&bad, &mut PassLog::default()).is_ok());
    bad.bones[0].scale[0] = 0.;
    assert!(lower_model(&bad, &mut PassLog::default()).is_err());
}

#[test]
fn dynamic_model_space_bind_pose_is_not_submitted_for_rigid_lowering() {
    let mut m = model();
    m.meshes[0].dynamic = true;
    m.meshes[0].vertices[0].bone_weights = [0.5, 0.5, 0., 0.];
    let mut log = PassLog::default();
    let out = lower_model(&m, &mut log).unwrap();
    assert_eq!(
        out.meshes[0].vertices[0].position,
        m.meshes[0].vertices[0].position
    );
    assert_eq!(out.meshes[0].vertices[0].normal, m.meshes[0].vertices[0].normal);
    assert_eq!(log.counts["model-space bind vertices"], 1);
    assert_eq!(log.counts["refused non-rigid vertices"], 0);
}
