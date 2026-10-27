//! Hand-worked SCP blob: an occluder section of a rectangle and a box, then the end record; and the damage the
//! reader must refuse or report.
use rme_formats::{
    scp_occluders::{SHAPE_BOX, SHAPE_RECTANGLE},
    scp_tables::{self, Chain},
};

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
/// the rectangle's matrix and outline start after the two records
const FIRST: usize = 0x10 + 2 * 16;
const SECOND: usize = FIRST + 64 + 4 * 32;
const SIZE: usize = SECOND + 64 + 12 * 32;
const END: usize = BLOB + SIZE;

fn fixture() -> Vec<u8> {
    let mut b = vec![0u8; END + 16];
    word(&mut b, 0, 0xbabf003c);
    word(&mut b, 0x44, BLOB as u32);
    word(&mut b, BLOB, 0x000b_4140);
    word(&mut b, BLOB + 4, SIZE as u32);
    word(&mut b, BLOB + 8, 2);
    word(&mut b, BLOB + 12, 0x10);
    for (i, (at, flags, edges)) in [(FIRST, 0x1f04_0800, 4), (SECOND, 0x1801, 12)]
        .into_iter()
        .enumerate()
    {
        let r = BLOB + 0x10 + i * 16;
        word(&mut b, r, 0x61220 + i as u32);
        word(&mut b, r + 4, at as u32);
        word(&mut b, r + 8, at as u32 + 64);
        word(&mut b, r + 12, flags);
        matrix(&mut b, BLOB + at, [i as f32, 2., 3.]);
        for s in 0..edges {
            for end in 0..2 {
                let q = BLOB + at + 64 + s * 32 + end * 16;
                for k in 0..3 {
                    float(&mut b, q + 4 * k, (s * 10 + end * 3 + k) as f32);
                }
                float(&mut b, q + 12, 1.);
            }
        }
    }
    word(&mut b, END, 0xffff_ffff);
    word(&mut b, END + 4, 8);
    b
}

#[test]
fn occluders_as_authored() {
    let (t, unread) = scp_tables::decode(&fixture()).unwrap();
    assert_eq!(t.chain, Chain::Complete);
    assert!(unread.is_empty(), "{unread:?}");
    assert_eq!(t.occluders.len(), 2);
    let (r, x) = (&t.occluders[0], &t.occluders[1]);
    assert_eq!((r.name, r.flags, r.outline.len()), (0x61220, 0x1f04_0800, 4));
    assert_eq!(r.flags & 0xffff, SHAPE_RECTANGLE);
    assert_eq!(
        (x.name, x.flags & 0xffff, x.outline.len()),
        (0x61221, SHAPE_BOX, 12)
    );
    assert_eq!(r.world.cols[3], [0., 2., 3.]);
    assert_eq!(x.world.cols[3], [1., 2., 3.]);
    assert_eq!(r.outline[1], [[10., 11., 12.], [13., 14., 15.]]);
    assert_eq!(x.outline[11][1], [113., 114., 115.]);
}

#[test]
fn damage_is_refused_or_reported() {
    let ok = fixture();
    // a shape that is not decoded, a count the records do not fit, a section that is not tiled: reported
    for (at, to) in [
        (BLOB + 0x10 + 12, 0x1234),
        (BLOB + 8, 1000),
        (BLOB + 4, SIZE as u32 + 16),
    ] {
        let mut b = ok.clone();
        word(&mut b, at, to);
        let (t, unread) = scp_tables::decode(&b).unwrap();
        assert!(t.occluders.is_empty(), "{at:#x}");
        assert!(unread.iter().any(|u| u.offset == BLOB), "{at:#x}");
    }
    // a record whose matrix or outline is not where the tiling puts it
    for at in [BLOB + 0x10 + 4, BLOB + 0x10 + 8, BLOB + 0x20 + 4] {
        let mut b = ok.clone();
        word(&mut b, at, 0x20);
        assert!(
            scp_tables::decode(&b)
                .unwrap_err()
                .to_string()
                .contains("off their place"),
            "{at:#x}"
        );
    }
    // a nonfinite point, an outline end whose w is not 1, a nonaffine matrix
    let mut b = ok.clone();
    word(&mut b, BLOB + FIRST + 64, 0x7fc0_0000);
    assert!(
        scp_tables::decode(&b)
            .unwrap_err()
            .to_string()
            .contains("nonfinite")
    );
    let mut b = ok.clone();
    word(&mut b, BLOB + FIRST + 64 + 12, 2f32.to_bits());
    assert!(
        scp_tables::decode(&b)
            .unwrap_err()
            .to_string()
            .contains("w is not 1")
    );
    let mut b = ok.clone();
    word(&mut b, BLOB + FIRST + 12, 1f32.to_bits());
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
