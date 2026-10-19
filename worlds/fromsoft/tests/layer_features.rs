//! The reader keeps layered parts' Byte4C UV members raw, keyed by member index, not by storage slot.
use primase_fromsoft::{
    flver::{Member, Mesh, Model, Vertex},
    sekiro::source::byte4_uv_members,
};

#[test]
fn semantic_indices_are_not_uv_storage_slots() {
    let member = |kind, index| Member {
        unknown: 0,
        offset: 0,
        kind,
        semantic: 5,
        index,
    };
    // Float4 occupies two UV slots. The next Byte4C is semantic 2 at slot 2.
    let vertex = Vertex {
        layer_bytes: vec![(2, [10, 20, 30, 40]), (3, [50, 60, 70, 80])],
        ..Default::default()
    };
    let model = Model {
        bones: vec![],
        version: 0x2001a,
        materials: vec![],
        layouts: vec![vec![member(3, 0), member(0x13, 2), member(0x13, 3)]],
        meshes: vec![Mesh {
            dynamic: false,
            material: 0,
            layouts: vec![0],
            vertices: vec![vertex],
            faces: vec![],
        }],
    };
    assert_eq!(
        byte4_uv_members(&model, 0, &[0]).unwrap(),
        [(2, vec![[10, 20, 30, 40]]), (3, vec![[50, 60, 70, 80]])]
    );
    let mut broken = model.clone();
    broken.meshes[0].vertices[0].layer_bytes.clear();
    assert!(byte4_uv_members(&broken, 0, &[0]).is_err());
    assert!(byte4_uv_members(&model, 0, &[1]).is_err());
}
