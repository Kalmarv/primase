//! A MeshShape body's section run: bodies laid out here by hand, their refusals, and two real MeshShapes whose
//! prefixes, section runs and tails were hand-checked in a separate read-only decode.
use std::fs::File;

use primase_anvil::read::data::{Resource, read_entry};
use primase_anvil::read::forge::Forge;
use primase_anvil::read::mesh_shape::{Error, MESH_SHAPE, MeshShape};
use primase_testdata::real_data;

/// A body: the prefix (version, byte, run length), the sections (type, marker, payload; size = 8 + payload), a tail.
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
    assert_eq!((m.version, m.flag, m.run_length), (1, 0, 13 + 11));
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
        Err(Error::Version(2))
    ));
    assert!(matches!(
        MeshShape::parse(&body(1, 1, None, one, b"")),
        Err(Error::Flag(1))
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
    let Some(path) = real_data("PRIMASE_ANVIL_FORGE_USNAT") else {
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
        assert_eq!((m.version, m.flag, m.run_length), (1, 0, run), "entry {k}");
        let got: Vec<(u16, u32)> = m.sections.iter().map(|s| (s.kind, s.size)).collect();
        let want: Vec<(u16, u32)> = kinds.iter().copied().zip(sizes).collect();
        assert_eq!(got, want, "entry {k}");
        assert_eq!(m.tail.len(), tail, "entry {k}");
    }
}
