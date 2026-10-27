//! Hand-worked SCP blob: a rail-candidate spline section, an AI-path section, a guide section and a light section,
//! then the end record; and the damage the readers must refuse or report.
use rme_formats::{
    scp_splines::SplineKind,
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
/// header 0x10, one 56-byte record, a 64-byte matrix, three points, three parameters
const SPLINE: usize = 0x10 + 56 + 64 + 36 + 12;
const LIGHT: usize = 0x10 + 80 + 64;
const RAIL: usize = BLOB;
const AI: usize = RAIL + SPLINE;
const GUIDE: usize = AI + SPLINE;
const NAV: usize = GUIDE + SPLINE;
const SHORT: usize = NAV + SPLINE;
const LIGHTS: usize = SHORT + SPLINE;
const END: usize = LIGHTS + LIGHT;

/// One section holding one spline: id, a surface id (rails only) and three points rising along x.
fn spline_section(b: &mut [u8], at: usize, flags: u32, id: u32, surface: u32) {
    word(b, at, flags);
    word(b, at + 4, SPLINE as u32);
    word(b, at + 8, 1);
    word(b, at + 12, 0x10);
    let r = at + 0x10;
    word(b, r, id);
    word(b, r + 4, 0x77);
    word(b, r + 8, id);
    word(b, r + 16, 1);
    word(b, r + 20, 3);
    word(b, r + 24, 3);
    word(b, r + 28, 2);
    word(b, r + 32, 1);
    word(b, r + 36, surface);
    word(b, r + 40, 0x48);
    word(b, r + 44, 0x88);
    word(b, r + 48, 0xac);
    matrix(b, at + 0x48, [9., 9., 9.]);
    for k in 0..3 {
        float(b, at + 0x88 + 12 * k, k as f32);
        float(b, at + 0x88 + 12 * k + 4, 5.);
        float(b, at + 0x88 + 12 * k + 8, -1.);
        float(b, at + 0xac + 4 * k, 0.5 * k as f32);
    }
}

fn fixture() -> Vec<u8> {
    let mut b = vec![0u8; END + 16];
    word(&mut b, 0, 0xbabf003c);
    word(&mut b, 0x44, BLOB as u32);
    // the low half of the flags is junk: 0x10 here, 0x81 and 0x80 on the disc
    spline_section(&mut b, RAIL, 0x0000_0010, 0xaaaa, 11);
    spline_section(&mut b, AI, 0x0009_0000, 0xbbbb, 0);
    spline_section(&mut b, GUIDE, 0x0001_0080, 0xcccc, 0);
    spline_section(&mut b, NAV, 0x000a_3f80, 0xdddd, 0);
    spline_section(&mut b, SHORT, 0x000c_0000, 0xeeee, 0);
    // a light: colour, name, three parameters, kind 7, the matrix at 0x60
    word(&mut b, LIGHTS, 0x0004_0000);
    word(&mut b, LIGHTS + 4, LIGHT as u32);
    word(&mut b, LIGHTS + 8, 1);
    word(&mut b, LIGHTS + 12, 0x10);
    let r = LIGHTS + 0x10;
    for (k, x) in [1., 0.5, 0.25, 1., 0., 2., 4., 0.75].into_iter().enumerate() {
        float(&mut b, r + 4 * k, x);
    }
    word(&mut b, r + 16, 0x1234);
    word(&mut b, r + 32, 7);
    word(&mut b, r + 36, u32::MAX);
    word(&mut b, r + 48, u32::MAX);
    word(&mut b, r + 52, 0x60);
    b[r + 64..r + 72].copy_from_slice(b"gobo_sta");
    matrix(&mut b, LIGHTS + 0x60, [3., 4., 5.]);
    word(&mut b, END, 0xffff_ffff);
    word(&mut b, END + 4, 8);
    b
}

#[test]
fn splines_and_lights_as_authored() {
    let (t, unread) = scp_tables::decode(&fixture()).unwrap();
    assert_eq!(t.chain, Chain::Complete);
    assert!(unread.is_empty(), "{unread:?}");
    assert_eq!(t.sections.len(), 6);
    let kinds: Vec<_> = t.splines.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        [
            SplineKind::RailCandidate,
            SplineKind::AiPath,
            SplineKind::Guide,
            SplineKind::NavPath,
            SplineKind::ShortPath
        ]
    );
    let s = &t.splines[0];
    assert_eq!(
        (s.id, s.parent, s.counts, s.words, s.surface, s.index),
        (0xaaaa, 0x77, [3, 3], [2, 1], 11, 0)
    );
    assert_eq!(s.points, [[0., 5., -1.], [1., 5., -1.], [2., 5., -1.]]);
    assert_eq!(s.params, [0., 0.5, 1.]);
    assert_eq!(s.matrix[12..15], [9., 9., 9.]);
    assert_eq!(
        t.splines.iter().map(|s| s.id).collect::<Vec<_>>(),
        [0xaaaa, 0xbbbb, 0xcccc, 0xdddd, 0xeeee]
    );
    assert_eq!(t.splines[1].surface, 0);

    assert_eq!(t.lights.len(), 1);
    let l = &t.lights[0];
    assert_eq!((l.kind, l.name, l.rgba), (7, 0x1234, [1., 0.5, 0.25, 1.]));
    assert_eq!(l.params, [2., 4., 0.75]);
    assert_eq!(l.links[0], u32::MAX);
    assert_eq!(&l.buffer[..8], b"gobo_sta");
    assert_eq!(l.world.cols[3], [3., 4., 5.]);
}

#[test]
fn section_flags_say_the_kind() {
    // the type is `flags >> 16`; the low half never matters
    for low in [0x0000, 0x0010, 0x0080, 0x0081, 0x3f80, 0xffff] {
        assert_eq!(SplineKind::of_section(low), Some(SplineKind::RailCandidate));
        assert_eq!(SplineKind::of_section(0x0001_0000 | low), Some(SplineKind::Guide));
        assert_eq!(
            SplineKind::of_section(0x0009_0000 | low),
            Some(SplineKind::AiPath)
        );
        assert_eq!(
            SplineKind::of_section(0x000a_0000 | low),
            Some(SplineKind::NavPath)
        );
        assert_eq!(
            SplineKind::of_section(0x000c_0000 | low),
            Some(SplineKind::ShortPath)
        );
    }
    // the class indices run 0 to 4 in the order of ALL
    assert_eq!(SplineKind::ALL.map(SplineKind::class), [0, 1, 2, 3, 4]);
    // every other type is no spline section
    for t in [2u32, 3, 4, 5, 6, 7, 8, 0xb, 0xd, 0xe, 0xf, 0x10] {
        assert_eq!(SplineKind::of_section(t << 16 | 0x81), None, "{t:#x}");
    }
}

#[test]
fn damage_is_refused_or_reported() {
    let ok = fixture();
    // a point pointer past the section
    let mut b = ok.clone();
    word(&mut b, RAIL + 0x10 + 44, SPLINE as u32 - 8);
    assert!(
        scp_tables::decode(&b)
            .unwrap_err()
            .to_string()
            .contains("outside their section")
    );
    // a point count that runs the arrays off the section
    let mut b = ok.clone();
    word(&mut b, AI + 0x10 + 20, 100);
    assert!(scp_tables::decode(&b).is_err());
    // a nonfinite point
    let mut b = ok.clone();
    word(&mut b, GUIDE + 0x88, 0x7fc0_0000);
    assert!(
        scp_tables::decode(&b)
            .unwrap_err()
            .to_string()
            .contains("nonfinite")
    );
    // records that do not fit the section: reported, no splines from it
    let mut b = ok.clone();
    word(&mut b, RAIL + 8, 9);
    let (t, unread) = scp_tables::decode(&b).unwrap();
    assert_eq!(t.splines.len(), 4);
    assert!(unread.iter().any(|u| u.offset == RAIL));
    // a light section whose size is not data + count * 144: reported, no lights
    let mut b = ok.clone();
    word(&mut b, LIGHTS + 4, LIGHT as u32 + 4);
    let (t, unread) = scp_tables::decode(&b).unwrap();
    assert!(t.lights.is_empty());
    assert!(unread.iter().any(|u| u.offset == LIGHTS));
    // a light matrix pointer into the records, then one that is not affine
    let mut b = ok.clone();
    word(&mut b, LIGHTS + 0x10 + 52, 0x20);
    assert!(
        scp_tables::decode(&b)
            .unwrap_err()
            .to_string()
            .contains("outside its array")
    );
    let mut b = ok.clone();
    word(&mut b, LIGHTS + 0x60 + 12, 1f32.to_bits());
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
