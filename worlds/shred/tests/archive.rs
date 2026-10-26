//! Hand-worked framing and raw RFC 1951 fixtures; no encoder of ours makes the expected bytes.
use primase_shred::read::{cstr, pak::Pak};

fn u16be(b: &mut Vec<u8>, n: u16) {
    b.extend(n.to_be_bytes());
}
fn u32be(b: &mut Vec<u8>, n: u32) {
    b.extend(n.to_be_bytes());
}
fn word(b: &mut [u8], at: usize, n: u32) {
    b[at..at + 4].copy_from_slice(&n.to_be_bytes());
}

fn stream() -> Vec<u8> {
    let mut b = b"CSTR".to_vec();
    for n in [0xfeed, 1, 2, 0xf32f] {
        u16be(&mut b, n);
    }
    u32be(&mut b, 6);
    // Non-final stored DEFLATE block, LEN=3, NLEN=~3, 'abc'; final empty stored block.
    let deflate = [0, 3, 0, 0xfc, 0xff, b'a', b'b', b'c', 1, 0, 0, 0xff, 0xff];
    u16be(&mut b, deflate.len() as u16);
    u16be(&mut b, 3);
    b.extend(deflate);
    u16be(&mut b, 3);
    u16be(&mut b, 3);
    b.extend(b"XYZ");
    b
}

fn archive() -> Vec<u8> {
    let compressed = stream();
    let names = b"MAIN\0a\0LOCAL\0b\0";
    let start = 16 + 4 * 32 + names.len() as u32;
    let mut b = b"PAK\0".to_vec();
    u16be(&mut b, 0xfeeb);
    u16be(&mut b, 4);
    u32be(&mut b, 4);
    u32be(&mut b, names.len() as u32);
    for row in [
        [0x1ed7_cb79, 0, 1, 0, start, 0, 0, 0],
        [97, 5, 0, 16, start, 2, 0, 0],
        [0xa705_be4b, 7, 1, 0, start + 2, 0, 0, 0],
        [98, 13, 2, 128, start + 2, 6, compressed.len() as u32, 0],
    ] {
        for n in row {
            u32be(&mut b, n);
        }
    }
    b.extend(names);
    b.extend(b"hi");
    b.extend(compressed);
    b
}

#[test]
fn two_sections_stored_and_compressed() {
    let p = Pak::from_bytes(archive()).unwrap();
    assert_eq!(p.sections().len(), 2);
    assert_eq!(p.sections()[0].name, b"MAIN");
    assert_eq!(p.sections()[0].entries, 0..2);
    assert_eq!(p.sections()[1].entries, 2..4);
    assert_eq!(p.entries()[3].section, Some(1));
    assert_eq!(p.entries()[3].alignment, 128);
    assert_eq!(p.payload(0).unwrap().as_ref(), b"");
    assert_eq!(p.payload(1).unwrap().as_ref(), b"hi");
    assert_eq!(p.payload(3).unwrap().as_ref(), b"abcXYZ");
    assert!(p.payload(4).is_err());
    let d = cstr::decode(&stream(), 6).unwrap();
    assert_eq!((d.blocks, d.stored_blocks), (2, 1));
}

#[test]
fn pak_refusals_and_offsets() {
    let base = archive();
    for (at, n) in [
        (0, 0),
        (4, 0),
        (6, 0),
        (8, u32::MAX),
        (12, u32::MAX),
        (16, 0),
        (20, u32::MAX),
        (24, 3),
        (28, 32),
        (28, 16),
        (32, 0),
        (36, 1),
        (40, 1),
        (44, 1),
        (64, 158),
        (64, 160),
        (136, 0),
        (136, u32::MAX),
    ] {
        let mut b = base.clone();
        word(&mut b, at, n);
        let e = Pak::from_bytes(b).unwrap_err();
        assert!(e.to_string().contains("0x"), "{at}: {e}");
    }
    let mut b = base.clone();
    b.push(0);
    assert!(Pak::from_bytes(b).is_err());
    let mut b = base.clone();
    b[158] = b'x';
    assert!(Pak::from_bytes(b).is_err()); // final name terminator
    for end in 0..base.len() {
        assert!(Pak::from_bytes(base[..end].to_vec()).is_err(), "{end}");
    }
}

#[test]
fn cstr_refusals_and_lazy_decode() {
    let base = stream();
    for (at, value) in [
        (0, 0),
        (4, 0),
        (6, 2),
        (8, 1),
        (9, 3),
        (10, 0),
        (12, 1),
        (16, 0xff),
        (18, 0xff),
        (19, 2),
        (20, 7),
    ] {
        let mut b = base.clone();
        b[at] = value;
        assert!(cstr::decode(&b, 6).is_err(), "{at}");
    }
    assert!(cstr::decode(&base, 7).is_err());
    // Both block sizes changed to 2: output sum 5, not 6 (framing stays exact).
    let mut b = base.clone();
    b[34] = 2;
    b[36] = 2;
    b.pop();
    assert!(cstr::decode(&b, 6).is_err());
    let mut b = base.clone();
    b.push(0);
    assert!(cstr::decode(&b, 6).is_err());
    for end in 0..base.len() {
        assert!(cstr::decode(&base[..end], 6).is_err(), "{end}");
    }
    let mut b = archive();
    b[161] = 0;
    let p = Pak::from_bytes(b).unwrap(); // metadata does not inflate payloads
    assert!(p.payload(3).is_err());
}
