//! Hostile structure in the container readers is a named refusal, never an abort or a mis-read: regression inputs of the
//! robustness census (nested native-DB containers, a Latin-1 name that does not round-trip, a LOD count past the header).

#![allow(clippy::unwrap_used)]

use frostbite_formats::db::{self, MAX_DEPTH, Node};
use frostbite_formats::meshset;

/// `levels` unnamed objects, one inside the next, the innermost empty: a node tag (2, unnamed) and the varint size of
/// what it holds.
fn nested(levels: usize) -> Vec<u8> {
    let mut inner: Vec<u8> = Vec::new();
    for _ in 0..levels {
        let mut n = inner.len();
        let mut size = Vec::new();
        loop {
            let b = (n & 127) as u8;
            n >>= 7;
            if n == 0 {
                size.push(b);
                break;
            }
            size.push(b | 128);
        }
        let mut node = vec![0x82];
        node.extend(size);
        node.extend(&inner);
        inner = node;
    }
    inner
}

/// 100,000 nested lists (about 300 KB) overflowed an 8 MiB stack and aborted the process; 10,000 read. The bound is
/// [`MAX_DEPTH`] levels, refused by name.
#[test]
fn containers_nested_past_the_bound_are_refused() {
    assert!(db::read_node(&nested(MAX_DEPTH), 0).is_ok());
    let e = db::read_node(&nested(MAX_DEPTH + 1), 0).unwrap_err().to_string();
    assert!(e.contains("nest deeper than"), "{e}");
    let e = db::read_node(&nested(100_000), 0).unwrap_err().to_string();
    assert!(e.contains("nest deeper than"), "{e}");
}

/// A name with a byte over 0x7f (Latin-1: one byte a character) advanced the reader by its UTF-8 length, so the
/// writer's own output failed to read back ("container length mismatch").
#[test]
fn a_latin1_name_round_trips() {
    let leaf = |name: &str, value: u32| Node {
        ty: 8,
        named: true,
        name: name.into(),
        payload: value.to_le_bytes().to_vec(),
        ..Node::default()
    };
    let root = Node {
        ty: 2,
        terminated: true,
        children: vec![leaf("café", 3), leaf("naïve ünïcode ÿ", 4), leaf("b", 5)],
        ..Node::default()
    };
    let mut bytes = Vec::new();
    db::write_node(&root, &mut bytes).unwrap();
    let (back, end) = db::read_node(&bytes, 0).unwrap();
    assert_eq!(back, root);
    assert_eq!(end, bytes.len());
    assert_eq!(back.field("café").and_then(Node::u32), Some(3));
}

/// The LOD record pointers lie between 0x30 and the name at 0x68: a count of more would read "pointers" out of the
/// header's other fields. A header of an unknown size is refused before anything else.
#[test]
fn a_lod_count_past_the_header_is_refused() {
    let mut d = vec![0u8; 0x200];
    for (at, v) in [(0, 0xc0u32), (4, 0xb0), (8, 0x180)] {
        d[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    // the name, at 0x100 (pointers are stored relative to byte 16)
    for at in [0x68, 0x70] {
        d[at..at + 8].copy_from_slice(&0xf0u64.to_le_bytes());
    }
    d[0x100] = b'x';
    d[0xb4..0xb6].copy_from_slice(&((meshset::MAX_LODS + 1) as u16).to_le_bytes());
    let e = meshset::read(&d).unwrap_err().to_string();
    assert!(e.contains("8 LODs"), "{e}");
    d[0xb4..0xb6].copy_from_slice(&0u16.to_le_bytes());
    let m = meshset::read(&d).unwrap();
    assert!(m.lods.is_empty() && m.name == "x");
    d[0] = 0xb0;
    let e = meshset::read(&d).unwrap_err().to_string();
    assert!(e.contains("header of 0xc0 bytes"), "{e}");
}
