//! A MeshShape body's section run: bodies laid out here by hand, their refusals, and two real MeshShapes whose
//! prefixes, section runs and tails were hand-checked in a separate read-only decode.
use std::fs::File;

use primase_anvil::read::data::{Resource, read_entry};
use primase_anvil::read::forge::Forge;
use primase_anvil::read::mesh_shape::{
    COUNTS, EDGE_FACES, Error, FACE_EDGES, FACES, MESH_SHAPE, MeshShape, NO_FACE, POSITIONS,
};
use primase_testdata::{Game, install_file};

/// A body: the prefix (a u32 holding its first four bytes, a byte, run length), the sections (type, marker, payload; size = 8 + payload), a tail.
fn body(version: u32, flag: u8, run: Option<u32>, sections: &[(u16, u16, &[u8])], tail: &[u8]) -> Vec<u8> {
    let mut s = Vec::new();
    for (kind, marker, payload) in sections {
        s.extend_from_slice(&kind.to_le_bytes());
        s.extend_from_slice(&marker.to_le_bytes());
        let size = i32::try_from(8 + payload.len()).expect("a test size");
        s.extend_from_slice(&size.to_le_bytes());
        s.extend_from_slice(payload);
    }
    let mut b = version.to_le_bytes().to_vec();
    b.push(flag);
    let run = run.unwrap_or_else(|| u32::try_from(s.len()).expect("a test size"));
    b.extend_from_slice(&run.to_le_bytes());
    b.extend(s);
    b.extend_from_slice(tail);
    b
}

/// The sections come back in order with their types, sizes and bytes; the tail and prefix as stored; a run of no
/// sections reads empty.
#[test]
fn a_body_reads_as_its_sections_and_tail() {
    let b = body(
        1,
        0,
        None,
        &[(0x0201, 0xECD7, b"name\0"), (0x1EF2, 0xECD7, &[1, 2, 3])],
        b"tail",
    );
    let m = MeshShape::parse(&b).unwrap();
    assert_eq!((m.second, m.run_length), (0, 13 + 11));
    let got: Vec<(usize, u16, u32, &[u8])> = m
        .sections
        .iter()
        .map(|s| (s.at, s.kind, s.size, s.bytes.as_slice()))
        .collect();
    assert_eq!(
        got,
        [
            (9, 0x0201, 13, &b"name\0"[..]),
            (22, 0x1EF2, 11, &[1u8, 2, 3][..])
        ]
    );
    assert_eq!(m.tail, b"tail");
    let empty = MeshShape::parse(&body(1, 0, None, &[], b"")).unwrap();
    assert!(empty.sections.is_empty() && empty.tail.is_empty());
}

/// What isn't established is refused, naming it.
#[test]
fn what_is_not_established_is_refused() {
    let one: &[(u16, u16, &[u8])] = &[(0x0201, 0xECD7, b"ab")];
    assert!(matches!(
        MeshShape::parse(&body(2, 0, None, one, b"")),
        Err(Error::Prefix { at: 0, found: 2, .. })
    ));
    // the second byte 0x3F is read, 0x40 isn't
    assert_eq!(
        MeshShape::parse(&body(0x3F01, 0, None, one, b""))
            .expect("read")
            .second,
        0x3F
    );
    assert!(matches!(
        MeshShape::parse(&body(0x4001, 0, None, one, b"")),
        Err(Error::Prefix {
            at: 1,
            found: 0x40,
            ..
        })
    ));
    assert!(matches!(
        MeshShape::parse(&body(1, 1, None, one, b"")),
        Err(Error::Prefix { at: 4, found: 1, .. })
    ));
    // the run longer than the body
    assert!(matches!(
        MeshShape::parse(&body(1, 0, Some(100), one, b"")),
        Err(Error::Run { .. })
    ));
    // a section without the marker
    let bad: &[(u16, u16, &[u8])] = &[(0x0201, 0xECD7, b"ab"), (0x0301, 0x1234, b"cd")];
    assert!(matches!(
        MeshShape::parse(&body(1, 0, None, bad, b"")),
        Err(Error::Marker { index: 1, .. })
    ));
    // the run ends inside a section: the section is past it
    assert!(matches!(
        MeshShape::parse(&body(1, 0, Some(9), one, b"xy")),
        Err(Error::Size { .. })
    ));
    // a size under its header
    let mut under = body(1, 0, None, one, b"");
    under[13..17].copy_from_slice(&4i32.to_le_bytes());
    assert!(matches!(MeshShape::parse(&under), Err(Error::Size { .. })));
    assert!(matches!(MeshShape::parse(&[1, 0, 0]), Err(Error::Short { .. })));
}

/// Two real MeshShapes of the large forge (Riders Republic build 25394987): the Board's and VansPark's. Same ten
/// section types in the same order, sizes as hand-checked, the tails' lengths.
#[test]
fn two_real_mesh_shapes_read_as_their_sections() {
    let Some(path) = install_file(Game::Riders, "DataPC_MAD_USNAT.forge") else {
        return;
    };
    let f = Forge::open(&path).unwrap();
    let mut file = File::open(&path).unwrap();
    let kinds = [
        0x0201u16, 0x0301, 0x1EEE, 0x1EEF, 0x1EF0, 0x1EF1, 0x1EF2, 0x1EF3, 0x1EFA, 0x1EF9,
    ];
    let cases = [
        (
            53_612usize,
            (1_111_231_778i64, 1_577i32),
            2_250usize,
            2_189u32,
            [78u32, 16, 20, 308, 284, 100, 560, 284, 36, 503],
            52usize,
        ),
        (
            21_435,
            (932_544_208, 15_368),
            25_570,
            24_965,
            [70, 16, 20, 4_664, 3_212, 1_076, 6_416, 3_584, 36, 5_871],
            596,
        ),
    ];
    for (k, (offset, size), body_len, run, sizes, tail) in cases {
        assert_eq!((f.index[k].offset, f.index[k].size), (offset, size), "entry {k}");
        let e = read_entry(&f.payload(&mut file, k).unwrap(), 1 << 20).unwrap();
        let r = Resource::parse(&e.resource.data).unwrap();
        assert_eq!((r.type_id, r.body.len()), (MESH_SHAPE, body_len), "entry {k}");
        let m = MeshShape::parse(&r.body).unwrap();
        assert_eq!((m.second, m.run_length), (0, run), "entry {k}");
        let got: Vec<(u16, u32)> = m.sections.iter().map(|s| (s.kind, s.size)).collect();
        let want: Vec<(u16, u32)> = kinds.iter().copied().zip(sizes).collect();
        assert_eq!(got, want, "entry {k}");
        assert_eq!(m.tail.len(), tail, "entry {k}");
    }
}

/// Two triangles sharing an edge, as the five geometry sections store them: 4 positions, 2 faces, 5 edges (one
/// shared, four on the boundary). Other sections are left as they are.
fn two_triangles() -> Vec<(u16, Vec<u8>)> {
    let le = |v: &[u32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let le16 = |v: &[u16]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let positions: Vec<u8> = [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0]
        .iter()
        .flat_map(|x| x.to_le_bytes())
        .collect();
    vec![
        (0x0201, b"name\0".to_vec()),
        (COUNTS, le(&[4, 2, 5])),
        (POSITIONS, positions),
        (FACES, le16(&[0, 1, 2, 2, 1, 3])),
        (FACE_EDGES, le(&[0, 1, 2, 1, 3, 4])),
        (
            EDGE_FACES,
            le16(&[0, NO_FACE, 0, 1, 0, NO_FACE, 1, NO_FACE, 1, NO_FACE]),
        ),
    ]
}

fn shape(sections: &[(u16, Vec<u8>)]) -> MeshShape {
    let s: Vec<(u16, u16, &[u8])> = sections.iter().map(|(k, b)| (*k, 0xECD7, b.as_slice())).collect();
    MeshShape::parse(&body(1, 0, None, &s, b"")).expect("a laid-out body")
}

/// The geometry reads as stored: positions, faces, each face's edges, each edge's faces, the boundary counted.
#[test]
fn the_geometry_reads_as_its_sections_store_it() {
    let g = shape(&two_triangles()).geometry().unwrap();
    assert_eq!(g.positions[3], [1.0, 1.0, 0.0]);
    assert_eq!(g.faces, [[0, 1, 2], [2, 1, 3]]);
    assert_eq!(g.face_edges, [[0, 1, 2], [1, 3, 4]]);
    assert_eq!(g.edge_faces[1], [0, 1]);
    assert_eq!(g.boundary_edges(), 4);
}

/// A section missing, repeated or of the wrong length, an index out of range, or an edge whose stored faces aren't
/// the faces naming it, is refused.
#[test]
fn inconsistent_geometry_is_refused() {
    let with = |kind: u16, bytes: Vec<u8>| {
        let mut s = two_triangles();
        for (k, b) in &mut s {
            if *k == kind {
                *b = bytes.clone();
            }
        }
        shape(&s).geometry()
    };
    let le16 = |v: &[u16]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    // a position index past the 4
    assert!(with(FACES, le16(&[0, 1, 4, 2, 1, 3])).is_err());
    // positions one short
    assert!(with(POSITIONS, vec![0; 36]).is_err());
    // edge 0 claims face 1, which doesn't name it
    assert!(
        with(
            EDGE_FACES,
            le16(&[0, 1, 0, 1, 0, NO_FACE, 1, NO_FACE, 1, NO_FACE])
        )
        .is_err()
    );
    // a face id past the 2
    assert!(
        with(
            EDGE_FACES,
            le16(&[0, 2, 0, 1, 0, NO_FACE, 1, NO_FACE, 1, NO_FACE])
        )
        .is_err()
    );
    // counts the sections cannot hold are refused before room is made for them (each of the three allocated its
    // count in records before the sections were looked at)
    let le = |v: &[u32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    assert!(with(COUNTS, le(&[4, 0x7FFF_FFFF, 5])).is_err());
    assert!(with(COUNTS, le(&[4, 2, 0x7FFF_FFFF])).is_err());
    assert!(with(COUNTS, le(&[0x7FFF_FFFF, 2, 5])).is_err());
    let mut missing = two_triangles();
    missing.retain(|(k, _)| *k != FACE_EDGES);
    assert!(shape(&missing).geometry().is_err());
    let mut twice = two_triangles();
    twice.push((COUNTS, vec![0; 12]));
    assert!(shape(&twice).geometry().is_err());
}

/// The two real shapes' geometry (Riders Republic build 25394987; counts and boundary hand-checked in a separate
/// read-only decode, games/riders-republic docs/recon/collision-geometry.md, 39c645b).
#[test]
fn two_real_shapes_read_their_geometry() {
    let Some(path) = install_file(Game::Riders, "DataPC_MAD_USNAT.forge") else {
        return;
    };
    let f = Forge::open(&path).unwrap();
    let mut file = File::open(&path).unwrap();
    for (k, (v, faces, x, boundary)) in [
        (53_612usize, (25usize, 46usize, 69usize, 0usize)),
        (21_435, (388, 534, 894, 186)),
    ] {
        let e = read_entry(&f.payload(&mut file, k).unwrap(), 1 << 20).unwrap();
        let r = Resource::parse(&e.resource.data).unwrap();
        let g = MeshShape::parse(&r.body).unwrap().geometry().unwrap();
        assert_eq!(
            (
                g.positions.len(),
                g.faces.len(),
                g.edge_faces.len(),
                g.boundary_edges()
            ),
            (v, faces, x, boundary),
            "entry {k}"
        );
        assert!(g.positions.iter().flatten().all(|c| c.is_finite()), "entry {k}");
    }
}
