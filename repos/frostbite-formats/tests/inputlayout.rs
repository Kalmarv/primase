//! Input layouts: a hand-laid layout read to its elements and usages, the refusals, and the rule that sends a vertex
//! program's inputs to its elements; and, on the game's data (the skate. install and its Oodle 9 library), every input
//! layout of the game configuration, level root and customization TOCs read, and the vertex programs of the game
//! configuration's raster permutations compared with their layouts: each input of the program must be the one element
//! its semantic name and index name.

#![allow(clippy::unwrap_used)]

use frostbite_formats::inputlayout::{self, Found, semantic_name};

/// A layout of the elements `(first, last, offset, dxgi, usage)`, laid as the game's are, and its metadata.
fn layout(id: u64, elements: &[(u32, u32, u32, u8, u8)]) -> (Vec<u8>, [u8; 16]) {
    let n = elements.len();
    let usages_at = 0x20 + (20 * n).next_multiple_of(16);
    let size = usages_at + n.next_multiple_of(16);
    let mut d = vec![0u8; size];
    d[0..4].copy_from_slice(&(n as u32).to_le_bytes());
    d[4..12].copy_from_slice(&0x20u64.to_le_bytes());
    d[12..20].copy_from_slice(&(usages_at as u64).to_le_bytes());
    for (i, &(first, last, offset, dxgi, usage)) in elements.iter().enumerate() {
        let at = 0x20 + 20 * i;
        d[at..at + 4].copy_from_slice(&first.to_le_bytes());
        d[at + 4..at + 8].copy_from_slice(&last.to_le_bytes());
        d[at + 8..at + 12].copy_from_slice(&offset.to_le_bytes());
        d[at + 16..at + 20].copy_from_slice(&(u32::from(dxgi) << 8 | 6).to_le_bytes());
        d[usages_at + i] = usage;
    }
    d.extend(4u32.to_le_bytes());
    d.extend(0xcu32.to_le_bytes());
    let mut m = [0u8; 16];
    m[0..4].copy_from_slice(&(size as u32).to_le_bytes());
    m[4..8].copy_from_slice(&8u32.to_le_bytes());
    m[8..16].copy_from_slice(&id.to_le_bytes());
    (d, m)
}

#[test]
fn a_layout_reads_to_its_elements_and_usages() {
    // position in slot 0; two texture coordinates and a tangent space in slots of their own
    let (d, m) = layout(
        77,
        &[
            (0, 0, 0, 6, 1),
            (2, 3, 0, 34, 33),
            (3, 4, 0, 34, 34),
            (4, 5, 0, 28, 52),
        ],
    );
    let l = inputlayout::read(&d, &m).unwrap();
    assert_eq!(l.id, 77);
    assert_eq!(l.usages, [1, 33, 34, 52]);
    assert_eq!(
        l.elements
            .iter()
            .map(|e| (e.first, e.last, e.dxgi()))
            .collect::<Vec<_>>(),
        [(0, 0, 6), (2, 3, 34), (3, 4, 34), (4, 5, 28)]
    );
    // the inputs of a program: POSITION0 is usage 1; TEXCOORD2 and TEXCOORD3 are the usages 33 and 34 by their indices
    assert_eq!(
        l.resolve(&[
            ("POSITION", 0),
            ("TEXCOORD", 2),
            ("TEXCOORD", 3),
            ("TEXCOORD", 0),
            ("NORMAL", 0)
        ]),
        [
            Some(Found::ByName(0)),
            Some(Found::ByName(1)),
            Some(Found::ByName(2)),
            None,
            None
        ]
    );
}

#[test]
fn an_input_finds_its_element_by_name_then_by_elimination() {
    assert_eq!(
        [1, 6, 151, 30, 33, 52].map(semantic_name),
        ["POSITION", "NORMAL", "COLOR", "TEXCOORD", "TEXCOORD", "TEXCOORD"]
    );
    // a normal and a texture coordinate may both be index 0, each found by its name
    let (d, m) = layout(1, &[(0, 0, 0, 6, 1), (0, 0, 12, 10, 6), (0, 0, 20, 34, 33)]);
    let l = inputlayout::read(&d, &m).unwrap();
    assert_eq!(
        l.resolve(&[("NORMAL", 0), ("TEXCOORD", 0)]),
        [Some(Found::ByName(1)), Some(Found::ByName(2))]
    );
    // a program that reads a position as TEXCOORD0 and usage 30 as a TEXCOORD: the one element left of the index
    let (d, m) = layout(2, &[(0, 0, 0, 6, 1), (2, 3, 0, 28, 30)]);
    let l = inputlayout::read(&d, &m).unwrap();
    assert_eq!(
        l.resolve(&[("TEXCOORD", 0), ("TEXCOORD", 2)]),
        [Some(Found::ByElimination(0)), Some(Found::ByName(1))]
    );
    // two inputs of one index and two elements that name neither: not told apart; and an element taken twice is not
    let (d, m) = layout(3, &[(1, 0, 0, 34, 7), (1, 0, 4, 34, 8), (2, 0, 8, 34, 9)]);
    let l = inputlayout::read(&d, &m).unwrap();
    assert_eq!(l.resolve(&[("COLOR", 1), ("NORMAL", 1)]), [None, None]);
    assert_eq!(
        l.resolve(&[("COLOR", 2), ("NORMAL", 2)]),
        [Some(Found::ByElimination(2)), None]
    );
}

#[test]
fn a_malformed_layout_is_refused() {
    let (d, m) = layout(1, &[(0, 0, 0, 6, 1), (2, 3, 0, 34, 33)]);
    assert!(inputlayout::read(&d, &m).is_ok());
    // a size that is not the metadata's
    let mut short = m;
    short[0..4].copy_from_slice(&((d.len() - 8 - 16) as u32).to_le_bytes());
    assert!(inputlayout::read(&d, &short).is_err());
    // an element count the payload cannot hold, and none
    for n in [0u32, 3, 33] {
        let mut bad = d.clone();
        bad[0..4].copy_from_slice(&n.to_le_bytes());
        assert!(inputlayout::read(&bad, &m).is_err(), "{n} elements");
    }
    // the pointers and the relocations are the ones every layout has
    let mut bad = d.clone();
    bad[4] = 0x24;
    assert!(inputlayout::read(&bad, &m).is_err());
    let mut bad = d.clone();
    let n = bad.len();
    bad[n - 8] = 8;
    assert!(inputlayout::read(&bad, &m).is_err());
}
