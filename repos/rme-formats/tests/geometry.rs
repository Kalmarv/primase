//! Hand-worked quartets decoded by `scene::decode`: refusals, processed skins and every packed vertex layout.
use rme_formats::scene;

fn word(b: &mut [u8], at: usize, n: u32) {
    b[at..at + 4].copy_from_slice(&n.to_be_bytes());
}
fn float(b: &mut [u8], at: usize, n: f32) {
    word(b, at, n.to_bits());
}
fn matrix(b: &mut [u8], at: usize, mirror: bool, translation: f32) {
    for i in 0..4 {
        float(b, at + i * 20, if i == 0 && mirror { -1. } else { 1. });
    }
    float(b, at + 48, translation);
}

struct Fixture {
    scp: Vec<u8>,
    mlp: Vec<u8>,
    mmp: Vec<u8>,
    mgp: Vec<u8>,
}
impl Fixture {
    /// Literal two-palette processed stream, with a separate rigid sibling and named source skeleton.
    fn processed() -> Self {
        let mut f = Self::new();
        let positions: Vec<_> = f.mmp[..96]
            .as_chunks::<16>()
            .0
            .iter()
            .map(|v| v[..12].to_vec())
            .collect();
        let ep = 432;
        let stream_size = 640;
        let metadata_size = ep + stream_size + 16;
        f.mlp.splice(84 + ep..84 + ep, vec![0; stream_size + 16]);
        word(&mut f.mlp, 28 + 16, metadata_size as u32);
        word(&mut f.mlp, 28 + 20, 12);
        word(&mut f.mlp, 56 + 4, metadata_size as u32);
        word(&mut f.mlp, 56 + 8, 12);
        word(&mut f.mlp, 84 + 0x8c, 0x8007f);
        word(&mut f.mlp, 84 + 0x9c, ep as u32);
        word(&mut f.mlp, 84 + 0xb4, 1);
        word(&mut f.mlp, 84 + 384 + 8, 36);
        word(&mut f.mlp, 84 + 384 + 20, 6 * 36);
        word(&mut f.mlp, 84 + 408 + 8, 0);
        word(&mut f.mlp, 84 + 192 + 56, 0x0100ffff);
        word(&mut f.mlp, 84 + 192 + 96 + 56, 0xffffffff);
        f.mmp.drain(..96);
        let start = 84 + ep;
        word(&mut f.mlp, start, stream_size as u32);
        for i in 0..15 {
            word(&mut f.mlp, start + 4 + i * 4, 0xaabbccdd);
        }
        for batch in 0..2 {
            let header = start + 64 + batch * 256;
            f.mlp[header..header + 4].copy_from_slice(if batch == 0 {
                &[0, 1, 255, 255]
            } else {
                &[2, 255, 255, 255]
            });
            f.mlp[header + 4..header + 6].copy_from_slice(&3u16.to_be_bytes());
            f.mlp[header + 6..header + 8].copy_from_slice(&(if batch == 0 { 2u16 } else { 1 }).to_be_bytes());
            for i in 0..14 {
                word(&mut f.mlp, header + 8 + i * 4, 0xddccbbaa);
            }
            for j in 0..3 {
                let p = header + 64 + j * 64;
                f.mlp[p..p + 12].copy_from_slice(&positions[batch * 3 + j]);
                let weights = if batch == 0 {
                    [[128, 127, 0, 0], [64, 191, 0, 0], [0, 255, 0, 0]][j]
                } else {
                    [255, 0, 0, 0]
                };
                f.mlp[p + 12..p + 16].copy_from_slice(&weights);
                float(&mut f.mlp, p + 24, 1.);
                word(&mut f.mlp, p + 28, 0x11223344);
                float(&mut f.mlp, p + 32, 1.);
                float(&mut f.mlp, p + 44, -1.);
                for (k, value) in [0x3800u16, 0xbc00, 0x4000, 0x3400].into_iter().enumerate() {
                    f.mlp[p + 48 + k * 2..p + 50 + k * 2].copy_from_slice(&value.to_be_bytes());
                }
                f.mlp[p + 56..p + 64].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
            }
        }
        // Zero-count terminator occupies the last 64 stream bytes; the 16-byte footer stays opaque.
        let table = f.scp.len();
        f.scp.resize(table + 3 * 16 + 3 * 64, 0);
        word(&mut f.scp, 0x20, 3);
        word(&mut f.scp, 0x24, table as u32);
        for (i, name) in [b"root".as_slice(), b"joint", b"toe"].into_iter().enumerate() {
            let text = f.scp.len();
            f.scp.extend_from_slice(name);
            f.scp.push(0);
            let transform = table + 48 + i * 64;
            matrix(&mut f.scp, transform, false, 10. + i as f32);
            word(
                &mut f.scp,
                table + i * 16,
                if i == 0 { 254 } else { 96 + i as u32 },
            );
            word(
                &mut f.scp,
                table + i * 16 + 4,
                if i == 0 { u32::MAX } else { i as u32 - 1 },
            );
            word(&mut f.scp, table + i * 16 + 8, text as u32);
            word(&mut f.scp, table + i * 16 + 12, transform as u32);
        }
        f
    }
    /// Hand-written native packed declaration families. The six vertices each receive the same known lanes.
    fn attributes(mask: u32, stride: usize) -> Self {
        let mut f = Self::new();
        f.mlp.splice(84 + 432..84 + 432, [0; 24]);
        word(&mut f.mlp, 28 + 16, 456);
        word(&mut f.mlp, 56 + 4, 456);
        word(&mut f.mlp, 84 + 0x8c, mask);
        word(&mut f.mlp, 84 + 0x94, 432);
        let mut row = vec![];
        if mask & 2 != 0 {
            row.extend([255, 128, 0, 64]);
        }
        if mask & 8 != 0 {
            row.extend(0x000003ffu32.to_be_bytes());
        }
        if mask & 16 != 0 {
            row.extend(0x001ff800u32.to_be_bytes());
        }
        // half values: UV0(.5,-1), UV1(2,.25), UV2(-2,.75), UV3(1,0).
        for (bit, halves) in [
            (5, [0x3800u16, 0xbc00]),
            (6, [0x4000, 0x3400]),
            (7, [0xc000, 0x3a00]),
            (8, [0x3c00, 0]),
        ] {
            if mask & (1 << bit) != 0 {
                for h in halves {
                    row.extend(h.to_be_bytes());
                }
            }
        }
        if mask & 0x600 != 0 {
            for h in [0x3c00u16, 0xc000, 0x3800, 0, 0x4000, 0x4200, 0x4400, 0] {
                row.extend(h.to_be_bytes());
            }
        }
        assert_eq!(row.len(), stride, "hand-worked family {mask:x}");
        for _ in 0..6 {
            f.mgp.extend(&row);
        }
        word(&mut f.mlp, 28 + 24, f.mgp.len() as u32);
        word(&mut f.mlp, 56 + 12, f.mgp.len() as u32);
        for (k, v) in [6, 6, stride as u32, 0, 0, f.mgp.len() as u32]
            .into_iter()
            .enumerate()
        {
            word(&mut f.mlp, 84 + 432 + k * 4, v);
        }
        f
    }
    fn new() -> Self {
        let mut mlp = vec![0; 84 + 432 + 336];
        word(&mut mlp, 0, 0xfafa0003);
        word(&mut mlp, 16, 2);
        word(&mut mlp, 20, 84);
        word(&mut mlp, 24, 28);
        let mut mmp = vec![0; 108 + 54];
        for (i, (base, nv, ns, offset)) in [(84, 6, 2, 0), (84 + 432, 3, 1, 108)].into_iter().enumerate() {
            let meta = if i == 0 { 432 } else { 336 };
            for (k, v) in [
                100 + i as u32,
                (base - 84) as u32,
                offset as u32,
                0,
                meta,
                nv * 18,
                0,
            ]
            .into_iter()
            .enumerate()
            {
                word(&mut mlp, 28 + i * 28 + k * 4, v);
            }
            word(&mut mlp, base, 0xbaba0031);
            matrix(&mut mlp, base + 16, false, 0.);
            for k in 0..3 {
                float(&mut mlp, base + 0x60 + k * 4, -100.);
                float(&mut mlp, base + 0x70 + k * 4, 100.);
            }
            word(&mut mlp, base + 0x80, 100 + i as u32);
            word(&mut mlp, base + 0x8c, 0x80005);
            let vp = 192 + ns * 96;
            let ip = vp + 24;
            for (k, v) in [vp, u32::MAX as usize, ip, u32::MAX as usize, 192, ns]
                .into_iter()
                .enumerate()
            {
                word(&mut mlp, base + 0x90 + k * 4, v as u32);
            }
            for (k, v) in [nv, nv, 16, 0, 0, nv * 16].into_iter().enumerate() {
                word(&mut mlp, base + vp + k * 4, v);
            }
            for (k, v) in [nv, 5, nv * 16, nv * 2, 1, 0].into_iter().enumerate() {
                word(&mut mlp, base + ip + k * 4, v);
            }
            for j in 0..ns {
                word(&mut mlp, base + 192 + j * 96 + 32, j as u32 * 3);
                word(&mut mlp, base + 192 + j * 96 + 36, 3);
                word(&mut mlp, base + 192 + j * 96 + 52, 7 + j as u32);
            }
            let points = [
                [1., 2., 3.],
                [4., 2., 3.],
                [1., 5., 3.],
                [10., 2., 3.],
                [11., 2., 3.],
                [10., 3., 3.],
            ];
            for (j, point) in points.iter().take(nv as usize).enumerate() {
                for (k, &value) in point.iter().enumerate() {
                    float(&mut mmp, offset + j * 16 + k * 4, value);
                }
                word(&mut mmp, offset + j * 16 + 12, 0x7fc00000); // +Z =511*64/32767, the RSX10-bit lane.
                mmp[offset + nv as usize * 16 + j * 2..offset + nv as usize * 16 + j * 2 + 2]
                    // Source CW winding, matching the disc's render convention.
                    .copy_from_slice(&([0u16, 2, 1, 3, 5, 4][j]).to_be_bytes());
            }
        }
        let mut scp = vec![0; 72 + 3 * 48 + 3 * 64];
        word(&mut scp, 0, 0xbabf003c);
        word(&mut scp, 16, 3);
        word(&mut scp, 20, 72);
        for i in 0..3 {
            word(&mut scp, 72 + i * 48, (216 + i * 64) as u32);
            word(&mut scp, 72 + i * 48 + 8, if i == 2 { 101 } else { 100 });
            matrix(&mut scp, 216 + i * 64, i == 1, if i == 1 { 20. } else { 0. });
        }
        Self {
            scp,
            mlp,
            mmp,
            mgp: vec![],
        }
    }
    fn decode(&self) -> rme_formats::Result<scene::Scene> {
        scene::decode(&self.scp, &self.mlp, &self.mmp, &self.mgp)
    }
}

#[test]
fn precise_refusals_and_eof() {
    let mut f = Fixture::new();
    word(&mut f.mlp, 0, 0);
    assert!(f.decode().unwrap_err().to_string().contains("mlp: bad magic"));
    let mut f = Fixture::new();
    f.mmp[96..98].copy_from_slice(&6u16.to_be_bytes());
    assert!(
        f.decode()
            .unwrap_err()
            .to_string()
            .contains("mmp: index out of range")
    );
    let mut f = Fixture::new();
    word(&mut f.mlp, 84 + 192 + 96 + 32, 6);
    assert!(f.decode().is_err());
    let mut f = Fixture::new();
    word(&mut f.mlp, 84 + 192 + 36, 0);
    assert!(f.decode().unwrap_err().to_string().contains("partition gap"));
    let mut f = Fixture::new();
    word(&mut f.scp, 72 + 8, 999);
    assert!(f.decode().unwrap_err().to_string().contains("missing mesh"));
    let mut f = Fixture::new();
    f.mmp.push(1);
    assert!(f.decode().unwrap_err().to_string().contains("uncovered bytes"));
    let f = Fixture::new();
    for n in 0..f.mlp.len() {
        assert!(
            scene::decode(&f.scp, &f.mlp[..n], &f.mmp, &f.mgp).is_err(),
            "truncated MLP {n}"
        );
    }
}

#[test]
fn processed_refusals_and_malformed_data() {
    let mut fixture = Fixture::processed();
    word(&mut fixture.mlp, 84 + 0xb4, 0);
    let scene = fixture.decode().unwrap();
    assert!(
        scene.models[0]
            .refusal
            .unwrap()
            .1
            .contains("processed skin format")
    );
    assert!(scene.models[1].refusal.is_none());
    for (offset, bytes, reason) in [
        (84 + 432 + 64, vec![3], "bone palette"),
        (84 + 432 + 64 + 4, vec![0, 0], "batch vertex count"),
        (84 + 432 + 128 + 12, vec![127], "weights must sum"),
        (84 + 432 + 128 + 48, vec![0x7c, 0], "processed UV"),
        (84 + 432 + 640 - 64 + 4, vec![0, 1], "terminal/size"),
    ] {
        let mut fixture = Fixture::processed();
        fixture.mlp[offset..offset + bytes.len()].copy_from_slice(&bytes);
        assert!(
            fixture.decode().unwrap_err().to_string().contains(reason),
            "{reason}"
        );
    }
    let mut fixture = Fixture::processed();
    word(&mut fixture.scp, 0x20, 0);
    assert!(
        fixture
            .decode()
            .unwrap_err()
            .to_string()
            .contains("no source skeleton")
    );
}

/// Oracle: literal signed CMP words, IEEE half values and native declaration offsets worked by hand.
#[test]
fn every_packed_layout_family_and_source_lanes() {
    for (mask, stride) in [
        (0x80005, 0),
        (0x80007, 4),
        (0x80025, 4),
        (0x80027, 8),
        (0x8003d, 12),
        (0x8003f, 16),
        (0x80065, 8),
        (0x80067, 12),
        (0x8007d, 16),
        (0x800e5, 12),
        (0x800fd, 20),
        (0x800ff, 24),
        (0x801e5, 16),
        (0x801fd, 24),
        (0x801ff, 28),
        (0x807fd, 40),
    ] {
        let f = Fixture::attributes(mask, stride);
        let scene = f.decode().unwrap();
        let a = &scene.models[0].attributes;
        assert_eq!(a.normals, vec![[0., 0., 32704. / 32767.]; 6]);
        assert_eq!(a.zero_normals, 0);
        assert_eq!(
            a.colors.as_ref().map(|c| c[0]),
            (mask & 2 != 0).then_some([1., 128. / 255., 0., 64. / 255.])
        );
        for (i, uv) in [[0.5, -1.], [2., 0.25], [-2., 0.75], [1., 0.]]
            .into_iter()
            .enumerate()
        {
            assert_eq!(
                a.uvs[i].as_ref().map(|v| v[0]),
                (mask & (1 << (i + 5)) != 0).then_some(uv)
            );
        }
        if mask & 8 != 0 {
            assert_eq!(a.raw["attr15"].values[0], [32736. / 32767., 0., 0., 0.]);
        }
        if mask & 16 != 0 {
            assert_eq!(a.raw["attr14"].values[0], [0., 32736. / 32767., 0., 0.]);
        }
        if mask & 0x600 != 0 {
            assert_eq!(a.raw["attr12"].values[0], [1., -2., 0.5, 0.]);
            assert_eq!(a.raw["attr13"].values[0], [2., 3., 4., 0.]);
        }
    }
    let n = rme_formats::attributes::cmp(0x80000400); // x=-1024, y=0, z=-512.
    assert_eq!(n, [-32768. / 32767., 0., -32768. / 32767.]);
}
