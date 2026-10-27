//! Hand-worked SCP blob: a spot camera section (two records) and a locator section, then the end record; and the
//! damage the readers must refuse or report.
use primase_shred::read::scp_tables::{self, Chain};

fn word(b: &mut [u8], at: usize, n: u32) {
    b[at..at + 4].copy_from_slice(&n.to_be_bytes());
}
fn float(b: &mut [u8], at: usize, x: f32) {
    word(b, at, x.to_bits());
}
fn matrix(b: &mut [u8], at: usize, t: [f32; 3]) {
    for i in 0..4 {
        float(b, at + i * 20, 1.);
    }
    for (k, v) in t.into_iter().enumerate() {
        float(b, at + 48 + k * 4, v);
    }
}

const BLOB: usize = 0x48;
/// header 0x10, two 48-byte records, four 64-byte matrices
const CAMERAS_SIZE: usize = 0x10 + 2 * 48 + 4 * 64;
/// header 0x10, one 16-byte record, one 64-byte matrix
const LOCATORS_SIZE: usize = 0x10 + 16 + 64;
const CAMERAS: usize = BLOB;
const LOCATORS: usize = CAMERAS + CAMERAS_SIZE;
const END: usize = LOCATORS + LOCATORS_SIZE;

fn header(b: &mut [u8], at: usize, flags: u32, size: usize, count: u32) {
    word(b, at, flags);
    word(b, at + 4, size as u32);
    word(b, at + 8, count);
    word(b, at + 12, 0x10);
}

fn fixture() -> Vec<u8> {
    let mut b = vec![0u8; END + 16];
    word(&mut b, 0, 0xbabf003c);
    word(&mut b, 0x44, BLOB as u32);
    // the low half of a flags word is junk
    header(&mut b, CAMERAS, 0x0003_0041, CAMERAS_SIZE, 2);
    let first = CAMERAS + 0x10 + 2 * 48;
    for i in 0..2 {
        let r = CAMERAS + 0x10 + i * 48;
        word(&mut b, r, 0x3666_b710);
        word(&mut b, r + 4, 0xdead_0001); // pool leftovers are not read
        word(&mut b, r + 8, 0x1000 + i as u32);
        word(&mut b, r + 16, 0x77);
        float(&mut b, r + 20, 53.638);
        word(&mut b, r + 24, 1);
        word(&mut b, r + 28, (0x10 + 2 * 48 + i * 128) as u32);
        word(&mut b, r + 32, (0x10 + 2 * 48 + i * 128 + 64) as u32);
        matrix(&mut b, first + i * 128, [i as f32, 1., 2.]);
        matrix(&mut b, first + i * 128 + 64, [i as f32, 5., 6.]);
    }
    header(&mut b, LOCATORS, 0x0005_0002, LOCATORS_SIZE, 1);
    let r = LOCATORS + 0x10;
    word(&mut b, r, primase_foundations::hash::sdbm(b"RMLocator"));
    word(&mut b, r + 4, 0xabcd);
    word(&mut b, r + 8, primase_foundations::hash::sdbm(b"Waypoint"));
    word(&mut b, r + 12, 0x20);
    matrix(&mut b, LOCATORS + 0x20, [7., 8., 9.]);
    word(&mut b, END, 0xffff_ffff);
    word(&mut b, END + 4, 8);
    b
}

#[test]
fn cameras_and_locators_as_authored() {
    let (t, unread) = scp_tables::decode(&fixture()).unwrap();
    assert_eq!(t.chain, Chain::Complete);
    assert!(unread.is_empty(), "{unread:?}");
    assert_eq!(t.cameras.len(), 2);
    let c = &t.cameras[1];
    assert_eq!(
        (c.id, c.name, c.parent, c.value, c.flag, c.index),
        (0x3666_b710, 0x1001, 0x77, 53.638, 1, 1)
    );
    assert_eq!(c.camera.cols[3], [1., 1., 2.]);
    assert_eq!(c.target.cols[3], [1., 5., 6.]);
    assert_eq!(t.locators.len(), 1);
    let l = &t.locators[0];
    assert_eq!(
        (l.class, l.name, l.kind),
        (
            primase_foundations::hash::sdbm(b"RMLocator"),
            0xabcd,
            primase_foundations::hash::sdbm(b"Waypoint")
        )
    );
    assert_eq!(l.world.cols[3], [7., 8., 9.]);
}

#[test]
fn damage_is_refused_or_reported() {
    let ok = fixture();
    // sections whose size does not fit their count: reported, nothing read from them
    let mut b = ok.clone();
    word(&mut b, CAMERAS + 8, 3);
    let (t, unread) = scp_tables::decode(&b).unwrap();
    assert!(t.cameras.is_empty());
    assert!(unread.iter().any(|u| u.offset == CAMERAS));
    let mut b = ok.clone();
    word(&mut b, LOCATORS + 8, 2);
    let (t, unread) = scp_tables::decode(&b).unwrap();
    assert!(t.locators.is_empty());
    assert!(unread.iter().any(|u| u.offset == LOCATORS));
    // a matrix pointer into the records or past the section
    for (at, to) in [
        (CAMERAS + 0x10 + 28, 0x20),
        (CAMERAS + 0x10 + 32, CAMERAS_SIZE as u32),
        (CAMERAS + 0x10 + 28, 0x42),
    ] {
        let mut b = ok.clone();
        word(&mut b, at, to);
        assert!(
            scp_tables::decode(&b)
                .unwrap_err()
                .to_string()
                .contains("outside its array"),
            "{at:#x} {to:#x}"
        );
    }
    let mut b = ok.clone();
    word(&mut b, LOCATORS + 0x10 + 12, 0x10);
    assert!(
        scp_tables::decode(&b)
            .unwrap_err()
            .to_string()
            .contains("outside its array")
    );
    // a nonfinite value, a nonaffine matrix
    let mut b = ok.clone();
    word(&mut b, CAMERAS + 0x10 + 20, 0x7fc0_0000);
    assert!(
        scp_tables::decode(&b)
            .unwrap_err()
            .to_string()
            .contains("nonfinite")
    );
    let mut b = ok.clone();
    word(&mut b, LOCATORS + 0x20 + 12, 1f32.to_bits());
    assert!(
        scp_tables::decode(&b)
            .unwrap_err()
            .to_string()
            .contains("nonaffine")
    );
    // every truncation is an error or a reported stop, never a panic
    for n in 0..ok.len() {
        let _ = scp_tables::decode(&ok[..n]);
    }
}
