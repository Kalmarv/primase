//! Hand-worked SCP: two nodes with a child and a placement link, world matrices, a spot section of two chunks, a
//! type-0xE section whose data starts 0x20 in, and the end record.
use rme_formats::Affine;
use rme_formats::scp_tables::{self, Chain, class_name, shape_name};

fn word(b: &mut [u8], at: usize, n: u32) {
    b[at..at + 4].copy_from_slice(&n.to_be_bytes());
}
fn matrix(b: &mut [u8], at: usize, t: [f32; 3]) {
    for i in 0..4 {
        word(b, at + i * 20, 1f32.to_bits());
    }
    for (k, v) in t.into_iter().enumerate() {
        word(b, at + 48 + k * 4, v.to_bits());
    }
}

const PLACEMENT: usize = 0x48;
const NODES: usize = 0x78;
const XF: usize = NODES + 2 * 0x70;
const MATRICES: usize = XF + 16;
const LINKS: usize = MATRICES + 128;
const BLOB: usize = LINKS + 8;
const SPOTS: usize = BLOB;
const SPOTS_SIZE: usize = 0x10 + 2 * 576;
const OTHER: usize = SPOTS + SPOTS_SIZE;
const OTHER_SIZE: usize = 0x30;
const END: usize = OTHER + OTHER_SIZE;

fn fixture() -> Vec<u8> {
    let mut b = vec![0u8; END + 16];
    word(&mut b, 0, 0xbabf003c);
    word(&mut b, 0x10, 1);
    word(&mut b, 0x14, PLACEMENT as u32);
    word(&mut b, 0x18, 2);
    word(&mut b, 0x1c, NODES as u32);
    word(&mut b, 0x38, 2);
    word(&mut b, 0x3c, XF as u32);
    word(&mut b, 0x44, BLOB as u32);
    for i in 0..2 {
        let n = NODES + i * 0x70;
        word(&mut b, n + 0x10, 0x1000 + i as u32);
        for k in 0..5 {
            word(&mut b, n + 0x34 + 4 * k, u32::MAX);
            word(&mut b, n + 0x5c + 4 * k, u32::MAX);
        }
        word(&mut b, XF + 8 * i, 0x2000 + i as u32);
        word(&mut b, XF + 8 * i + 4, (MATRICES + 64 * i) as u32);
        matrix(&mut b, MATRICES + 64 * i, [i as f32, 2., 3.]);
    }
    // node 0: sphere words, one child (node 1), one placement (placement 0)
    for (k, v) in [1f32, 2., 3., 4.].into_iter().enumerate() {
        word(&mut b, NODES + 4 * k, v.to_bits());
    }
    word(&mut b, NODES + 0x20, 1);
    word(&mut b, NODES + 0x34, LINKS as u32);
    word(&mut b, NODES + 0x48, 1);
    word(&mut b, NODES + 0x5c, (LINKS + 4) as u32);
    word(&mut b, LINKS, (NODES + 0x70) as u32);
    word(&mut b, LINKS + 4, PLACEMENT as u32);
    // spot section
    word(&mut b, SPOTS, 0x20000);
    word(&mut b, SPOTS + 4, SPOTS_SIZE as u32);
    word(&mut b, SPOTS + 8, 2);
    word(&mut b, SPOTS + 12, 0x10);
    for (k, (class, id)) in [("Teleport", 0x1001u32), ("?", 0x9999)].into_iter().enumerate() {
        let c = SPOTS + 0x10 + k * 0x200;
        word(
            &mut b,
            c + 0x10,
            primase_foundations::hash::sdbm(format!("spot{k}").as_bytes()),
        );
        word(&mut b, c + 0x14, id);
        word(&mut b, c + 0x18, 0x1000);
        word(
            &mut b,
            c + 0x1c,
            if k == 0 {
                primase_foundations::hash::sdbm(class.as_bytes())
            } else {
                0xdead_beef
            },
        );
        word(&mut b, c + 0x20, primase_foundations::hash::sdbm(b"Rectangle"));
        word(&mut b, c + 0x24, 0x41);
        word(&mut b, c + 0x28, 0x42);
        matrix(
            &mut b,
            SPOTS + 0x10 + 2 * 0x200 + 64 * k,
            [10. * (k + 1) as f32, 0., 5.],
        );
    }
    // a type-0xE section with its data 0x20 bytes in
    word(&mut b, OTHER, 0x000e_0012);
    word(&mut b, OTHER + 4, OTHER_SIZE as u32);
    word(&mut b, OTHER + 8, 1);
    word(&mut b, OTHER + 12, 0x20);
    // the end record
    word(&mut b, END, 0xffff_ffff);
    word(&mut b, END + 4, 8);
    b
}

#[test]
fn nodes_links_matrices_and_spot_chunks() {
    let (t, unread) = scp_tables::decode(&fixture()).unwrap();
    assert_eq!(t.nodes.len(), 2);
    assert_eq!(t.nodes[0].children, [1]);
    assert_eq!(t.nodes[0].placements, [0]);
    assert_eq!(t.nodes[0].sphere, Some([1., 2., 3., 4.]));
    assert_eq!(t.nodes[1].sphere, None);
    assert_eq!(t.nodes[1].id, 0x1001);
    assert_eq!(t.nodes[1].matrix_hash, 0x2001);
    assert_eq!(t.nodes[1].world.cols[3], [1., 2., 3.]);
    assert_eq!(t.chain, Chain::Complete);
    assert_eq!(t.sections.len(), 2);
    assert_eq!((t.sections[1].kind(), t.sections[1].data), (0xe, 0x20));
    assert_eq!(t.spots.len(), 2);
    let s = &t.spots[0];
    assert_eq!(class_name(s.class), Some("Teleport"));
    assert_eq!(shape_name(s.shape), Some("Rectangle"));
    assert_eq!(
        (s.name, s.id, s.parent, s.pair),
        (
            primase_foundations::hash::sdbm(b"spot0"),
            0x1001,
            0x1000,
            [0x41, 0x42]
        )
    );
    // a chunk whose own id is a node's id knows the node; the other is only a chunk
    assert_eq!(s.node, Some(1));
    assert_eq!(t.spots[1].node, None);
    assert_eq!(class_name(t.spots[1].class), None);
    assert_eq!(
        s.world,
        Affine {
            cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [10., 0., 5.]]
        }
    );
    // the undecoded section is reported with its offset and size, the spot section is not
    assert_eq!(unread.len(), 1);
    assert_eq!((unread[0].offset, unread[0].bytes), (OTHER, OTHER_SIZE));
}

#[test]
fn damage_is_refused_or_reported() {
    let ok = fixture();
    // a child link into the middle of a record
    let mut b = ok.clone();
    word(&mut b, LINKS, (NODES + 0x71) as u32);
    assert!(
        scp_tables::decode(&b)
            .unwrap_err()
            .to_string()
            .contains("not a record start")
    );
    // a link kind other than 0
    let mut b = ok.clone();
    word(&mut b, NODES + 0x24, 1);
    assert!(
        scp_tables::decode(&b)
            .unwrap_err()
            .to_string()
            .contains("kind other than 0")
    );
    // a matrix count that differs from the node count
    let mut b = ok.clone();
    word(&mut b, 0x38, 1);
    assert!(scp_tables::decode(&b).is_err());
    // a nonaffine spot matrix
    let mut b = ok.clone();
    word(&mut b, SPOTS + 0x10 + 2 * 0x200 + 12, 1f32.to_bits());
    assert!(
        scp_tables::decode(&b)
            .unwrap_err()
            .to_string()
            .contains("nonaffine")
    );
    // an end record that is not one: the chain stops there and the rest is reported
    let mut b = ok.clone();
    word(&mut b, END + 4, 9);
    let (t, unread) = scp_tables::decode(&b).unwrap();
    assert_eq!(t.chain, Chain::Stopped { offset: END });
    assert_eq!(t.sections.len(), 2);
    assert!(unread.iter().any(|u| u.offset == END && u.bytes == 16));
    // a spot section whose size is not 0x10 + count * 576 is reported, not read
    let mut b = ok.clone();
    word(&mut b, SPOTS + 8, 1);
    let (t, unread) = scp_tables::decode(&b).unwrap();
    assert!(t.spots.is_empty());
    assert!(unread.iter().any(|u| u.offset == SPOTS));
    // no blob
    let mut b = ok.clone();
    word(&mut b, 0x44, u32::MAX);
    let (t, unread) = scp_tables::decode(&b).unwrap();
    assert_eq!((t.chain, t.sections.len(), unread.len()), (Chain::None, 0, 0));
    // every truncation is an error or a reported stop, never a panic
    for n in 0..ok.len() {
        let _ = scp_tables::decode(&ok[..n]);
    }
}
