//! The reader keeps layered parts' Byte4C UV members raw, keyed by member index, not by storage slot.
use fromsoft_formats::flver::{Member, Mesh, Model, Vertex};
use primase_core::{Semantic, Stream};
use primase_fromsoft::sekiro::{source::SECOND_COLOUR, source::byte4_uv_members, source::layer_streams};

#[test]
fn semantic_indices_are_not_uv_storage_slots() {
    let member = |kind, index| Member {
        unknown: 0,
        stored: true,
        offset: 0,
        kind,
        semantic: 5,
        index,
    };
    // Float4 occupies two UV slots. The next Byte4C is semantic 2 at slot 2.
    let vertex = Vertex {
        layer_bytes: [(2, [10, 20, 30, 40]), (3, [50, 60, 70, 80])].into(),
        ..Default::default()
    };
    let model = Model {
        bones: vec![],
        dummies: vec![],
        version: 0x2001a,
        speedtree: false,
        materials: vec![],
        layouts: vec![vec![member(3, 0), member(0x13, 2), member(0x13, 3)]],
        meshes: vec![Mesh {
            dynamic: false,
            node: -1,
            palette: vec![],
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

/// The second UV set and the second colour of a layered mesh are kept as streams, found by the element layout (UV
/// storage slot 1, the colour element indexed 1), never by position among the decoded values.
#[test]
fn a_layered_mesh_keeps_its_second_uv_set_and_second_colour() {
    let member = |kind, semantic, index| Member {
        unknown: 0,
        stored: true,
        offset: 0,
        kind,
        semantic,
        index,
    };
    // slots 0 and 1: the packed pair; slot 2: Byte4C member 2; colours: element 0, then element 1
    let layouts = vec![vec![
        member(0x16, 5, 0),
        member(0x13, 5, 2),
        member(0x13, 10, 0),
        member(3, 10, 1),
    ]];
    let vertex = |u: f32, c: f32| Vertex {
        uvs: [[u, 0., 0., 0.], [u + 0.5, 1., 0., 0.], [9., 9., 0., 0.]].into(),
        layer_bytes: [(0, [5; 4]), (1, [6; 4]), (2, [1, 2, 3, 4])].into(),
        colors: [[0.; 4], [c, 0.25, 0.5, 1.]].into(),
        ..Default::default()
    };
    let model = |layouts| Model {
        bones: vec![],
        dummies: vec![],
        version: 0x2001a,
        speedtree: false,
        materials: vec![],
        layouts,
        meshes: vec![Mesh {
            dynamic: false,
            node: -1,
            palette: vec![],
            material: 0,
            layouts: vec![0],
            vertices: vec![vertex(0.25, 0.75), vertex(0.5, 0.125)],
            faces: vec![],
        }],
    };
    let streams = layer_streams(&model(layouts.clone()), 0, &[1, 0]).unwrap();
    assert_eq!(
        streams,
        [
            (Semantic::Color(2), Stream::U8x4(vec![[1, 2, 3, 4]; 2])),
            (Semantic::TexCoord(1), Stream::F32x2(vec![[1., 1.], [0.75, 1.]])),
            (
                SECOND_COLOUR,
                Stream::F32x4(vec![[0.125, 0.25, 0.5, 1.], [0.75, 0.25, 0.5, 1.]])
            ),
        ]
    );
    // no colour element indexed 1: no second colour stream is made up
    let mut one = layouts.clone();
    one[0][3].index = 0;
    let streams = layer_streams(&model(one), 0, &[0]).unwrap();
    assert!(streams.iter().all(|(s, _)| *s != SECOND_COLOUR));
    // a Byte4C member in the second UV slot is not a UV set
    let mut packed = layouts.clone();
    packed[0] = vec![member(0x13, 5, 1), member(0x13, 5, 2), member(0x13, 10, 0)];
    let streams = layer_streams(&model(packed), 0, &[0]).unwrap();
    assert!(streams.iter().all(|(s, _)| *s != Semantic::TexCoord(1)));
    // a Byte4C member that would take the second colour's stream is refused
    let mut clash = layouts;
    clash[0][1].index = 9;
    assert!(layer_streams(&model(clash), 0, &[0]).is_err());
}
