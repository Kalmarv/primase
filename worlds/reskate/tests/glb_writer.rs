//! The map's GLB writer builds on `primase_glb` (container, buffer views, float accessors): its bytes are what they were
//! before it did (pinned by hash), the toolkit's strict reader reads them, and a non-finite position is an error.

#![allow(clippy::unwrap_used)]

use primase_core::{Semantic, Stream};
use primase_glb::Gltf;
use primase_reskate::glb::{GlbPrim, write};

fn cases() -> Vec<Vec<GlbPrim>> {
    vec![
        vec![
            GlbPrim {
                material: Some("a".into()),
                pos: vec![0., 0., 0., 1., 0., 0., 0., 1., -0.0],
                nrm: Some(vec![0., 0., 1., 0., 0., 1., 0., 0., 1.]),
                uv: Some(vec![0., 0., 1., 0., 0., 1.]),
                idx: vec![0, 1, 2],
                blend: Some(vec![0.1, 0.5, 1.0]),
            },
            GlbPrim {
                material: None,
                pos: vec![-1.5, 2.25, 3.125, 4., 5., 6., 0.1, 0.2, 0.3],
                nrm: None,
                uv: None,
                idx: vec![2, 1, 0],
                blend: None,
            },
        ],
        vec![GlbPrim {
            material: Some("b".into()),
            pos: vec![1., 2., 3., 4., 5., 6., 7., 8., 9.],
            nrm: None,
            uv: Some(vec![0.3; 6]),
            idx: vec![0, 1, 2, 2, 1, 0],
            blend: None,
        }],
    ]
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    })
}

/// Hashes taken of the writer's output before it was moved onto the toolkit's container and accessor encoding (FNV-1a of
/// the whole file, and its length): the move changes no byte.
#[test]
fn the_bytes_are_those_the_writer_always_wrote() {
    let got: Vec<(usize, u64)> = cases()
        .iter()
        .map(|c| {
            let b = write("m", c).unwrap();
            (b.len(), fnv(&b))
        })
        .collect();
    assert_eq!(got, [(1548, 0x78ac_a41a_b6d3_785b), (844, 0xdb00_82ba_e519_40ba)]);
}

/// The toolkit's strict reader reads the file back: the frame is y up (x, z, -y of the writer's), the blend weights a custom
/// attribute.
#[test]
fn the_toolkits_reader_reads_it() {
    let glb = write("m", &cases()[0]).unwrap();
    let g = Gltf::from_glb(&glb).unwrap();
    let mesh = g.read_mesh(0).unwrap();
    assert_eq!(mesh.primitives.len(), 2);
    let first = &mesh.primitives[0];
    // (x, y, z) of the writer's frame is (x, z, -y)
    assert_eq!(
        first.primitive.streams[&Semantic::Position],
        Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., -0.0, -1.]])
    );
    assert_eq!(first.material_name.as_deref(), Some("a"));
    assert_eq!(first.custom["_RESKATE_BLEND"].count, 3);
    assert_eq!(mesh.primitives[1].material, None);
    assert_eq!(first.primitive.indices, Some(vec![0, 1, 2]));
}

#[test]
fn a_non_finite_position_is_an_error() {
    let mut c = cases();
    c[1][0].pos[4] = f32::NAN;
    let e = write("m", &c[1]).unwrap_err().to_string();
    assert!(e.contains("position value 4 is not finite"), "{e}");
}
