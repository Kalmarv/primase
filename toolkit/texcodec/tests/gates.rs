use primase_core::{
    TexFormat,
    gate::{GateReport, Tolerance},
};
use primase_texcodec::{Bcdec, Decoder, TexDecoder};
/// Independent bcdec_rs 0.2.0 oracle; every random block includes all selector values
/// statistically, with endpoint orders and equality exercised explicitly below.
#[test]
fn seeded_bc1_to_bc5() {
    let mut state = 0x12345678u32;
    for f in [
        TexFormat::Bc1,
        TexFormat::Bc2,
        TexFormat::Bc3,
        TexFormat::Bc4Unorm,
        TexFormat::Bc5Unorm,
    ] {
        let mut gate = GateReport::new(format!("{f:?} vs bcdec"), Tolerance::Exact);
        for i in 0..10000 {
            let mut b = vec![0; f.block_bytes() as usize];
            for v in &mut b {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *v = state as u8;
            }
            let ours = Decoder::default().decode_rgba8(f, 4, 4, &b).expect("ours");
            let reference = Bcdec.decode_rgba8(f, 4, 4, &b).expect("oracle");
            gate.compare_bytes(i.to_string(), &ours, &reference);
        }
        println!("{gate}");
        gate.assert_passed();
    }
}
#[test]
fn punch_through_and_four_color() {
    for decoder in [&Decoder::default() as &dyn TexDecoder, &Bcdec] {
        for (a, b, alpha) in [(0u16, 65535u16, 0), (42, 42, 0), (65535, 0, 255)] {
            let mut block = [255; 8];
            block[..2].copy_from_slice(&a.to_le_bytes());
            block[2..4].copy_from_slice(&b.to_le_bytes());
            let out = decoder
                .decode_rgba8(TexFormat::Bc1, 4, 4, &block)
                .expect("decode");
            for p in out.as_chunks::<4>().0.iter() {
                assert_eq!(p[3], alpha);
                if alpha == 0 {
                    assert_eq!(p, &[0; 4]);
                }
            }
        }
    }
}
#[test]
fn signed_float_oracle() {
    let mut gate = GateReport::new("SNORM vs bcdec float", Tolerance::Abs(0.0000002));
    for a in 0..=255 {
        for b in 0..=255 {
            let block = [a, b, 0x88, 0xc6, 0xfa, 0x88, 0xc6, 0xfa];
            let ours = Decoder::default()
                .decode_rgba32f(TexFormat::Bc4Snorm, 4, 4, &block)
                .expect("ours");
            let reference = Bcdec
                .decode_rgba32f(TexFormat::Bc4Snorm, 4, 4, &block)
                .expect("ref");
            let diff = ours
                .iter()
                .zip(&reference)
                .map(|(a, b)| (a - b).abs())
                .fold(0., f32::max);
            gate.record(format!("{a}/{b}"), diff as f64);
        }
    }
    println!("{gate}");
    gate.assert_passed();
}
#[test]
fn invalid_inputs_and_edge_blocks() {
    let d = Decoder::default();
    assert!(d.decode_rgba8(TexFormat::Bc1, 0, 4, &[]).is_err());
    assert!(d.decode_rgba8(TexFormat::Bc1, 4, 4, &[0; 7]).is_err());
    assert!(d.decode_rgba8(TexFormat::Bc1, 4, 4, &[0; 9]).is_err());
    assert!(d.decode_rgba8(TexFormat::Etc1, 4, 4, &[0; 8]).is_err());
    assert_eq!(
        d.decode_rgba8(TexFormat::Bc1, 5, 3, &[0; 16]).expect("odd").len(),
        60
    );
    assert_eq!(
        d.decode_rgba8(TexFormat::Bc4Snorm, 1, 1, &[0; 8])
            .expect("signed zero"),
        [128, 0, 0, 255]
    );
    assert_eq!(
        d.decode_rgba32f(TexFormat::R32Float, 1, 1, &2.5f32.to_le_bytes())
            .expect("hdr"),
        [2.5, 0., 0., 1.]
    );
}
/// Independent reference C# decoder, generated synthetic fixtures.
#[test]
fn csharp_fixtures() {
    let decoder = Decoder::default();
    let mut gate = GateReport::new("D3D vs C# BlockDecode", Tolerance::Abs(1.));
    for (name, f) in [
        ("bc1", TexFormat::Bc1),
        ("bc2", TexFormat::Bc2),
        ("bc3", TexFormat::Bc3),
        ("bc5", TexFormat::Bc5Unorm),
    ] {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let input = std::fs::read(root.join(format!("{name}.blocks"))).expect("fixture");
        let expected = std::fs::read(root.join(format!("{name}.csharp.rgba"))).expect("oracle");
        gate.compare_bytes(
            name,
            &decoder.decode_rgba8(f, 32, 32, &input).expect("decode"),
            &expected,
        );
    }
    println!("{gate}");
    gate.assert_passed();
}
#[test]
fn pillow_measurement() {
    for (name, f) in [
        ("bc1", TexFormat::Bc1),
        ("bc2", TexFormat::Bc2),
        ("bc3", TexFormat::Bc3),
        ("bc4", TexFormat::Bc4Unorm),
        ("bc5", TexFormat::Bc5Unorm),
        ("bc7", TexFormat::Bc7),
    ] {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let input = std::fs::read(root.join(format!("{name}.blocks"))).expect("fixture");
        let mut expected = std::fs::read(root.join(format!("{name}.pillow.rgba"))).expect("oracle");
        // Pillow presents BC4 as grayscale; our texture-channel contract is R001.
        if f == TexFormat::Bc4Unorm {
            for p in expected.as_chunks_mut::<4>().0 {
                p[1] = 0;
                p[2] = 0;
            }
        }
        let got = Decoder::default()
            .decode_rgba8(f, 32, 32, &input)
            .expect("decode");
        let mut gate = GateReport::new(format!("D3D vs Pillow {f:?}"), Tolerance::Abs(1.));
        gate.compare_bytes(name, &got, &expected);
        let mut hist = [0u64; 256];
        for (a, b) in got.iter().zip(&expected) {
            hist[a.abs_diff(*b) as usize] += 1;
        }
        println!(
            "{gate}; histogram={:?}",
            hist.iter()
                .enumerate()
                .filter(|(_, n)| **n > 0)
                .collect::<Vec<_>>()
        );
        gate.assert_passed();
    }
}
#[test]
fn fh6_minus127_minus128_compares_clamped_endpoints() {
    // Signed mode selection compares endpoints after clamping.
    let block = [129, 128, 255, 255, 255, 255, 255, 255];
    for d in [&Decoder::default() as &dyn TexDecoder, &Bcdec] {
        assert_eq!(
            d.decode_rgba32f(TexFormat::Bc4Snorm, 1, 1, &block)
                .expect("decode"),
            [1., 0., 0., 1.]
        );
    }
}
/// Hand-derived channel values cover every supported uncompressed layout.
#[test]
fn unpacked_formats() {
    use TexFormat::*;
    let cases: Vec<(TexFormat, Vec<u8>, [f32; 4])> = vec![
        (R8Unorm, vec![255], [1., 0., 0., 1.]),
        (Rg8Unorm, vec![255, 0], [1., 0., 0., 1.]),
        (Rgba8Unorm, vec![255, 0, 255, 0], [1., 0., 1., 0.]),
        (A8Unorm, vec![128], [0., 0., 0., 128. / 255.]),
        (R8Snorm, vec![128], [-1., 0., 0., 1.]),
        (Rg8Snorm, vec![128, 127], [-1., 1., 0., 1.]),
        (Rgba8Snorm, vec![128, 127, 0, 127], [-1., 1., 0., 1.]),
        (Bgra8Unorm, vec![0, 255, 128, 0], [128. / 255., 1., 0., 0.]),
        (Bgrx8Unorm, vec![0, 255, 128, 0], [128. / 255., 1., 0., 1.]),
        (B5g6r5Unorm, 0xf800u16.to_le_bytes().to_vec(), [1., 0., 0., 1.]),
        (B5g5r5a1Unorm, 0x7c00u16.to_le_bytes().to_vec(), [1., 0., 0., 0.]),
        (B4g4r4a4Unorm, 0x0f00u16.to_le_bytes().to_vec(), [1., 0., 0., 0.]),
        (
            Rgb10a2Unorm,
            0x400003ffu32.to_le_bytes().to_vec(),
            [1., 0., 0., 1. / 3.],
        ),
        (R16Unorm, vec![255, 255], [1., 0., 0., 1.]),
        (Rg16Unorm, vec![255, 255, 0, 0], [1., 0., 0., 1.]),
        (Rg16Snorm, vec![0, 128, 255, 127], [-1., 1., 0., 1.]),
        (
            Rgba16Unorm,
            vec![255, 255, 0, 0, 255, 255, 0, 0],
            [1., 0., 1., 0.],
        ),
        (R16Float, vec![0, 0x3e], [1.5, 0., 0., 1.]),
        (Rg16Float, vec![0, 0x3e, 0, 0xc0], [1.5, -2., 0., 1.]),
        (
            Rgba16Float,
            vec![0, 0x3e, 0, 0xc0, 0, 0, 0, 0x3c],
            [1.5, -2., 0., 1.],
        ),
        (R32Float, 1.5f32.to_le_bytes().to_vec(), [1.5, 0., 0., 1.]),
        (
            Rg32Float,
            [1.5f32, -2.].iter().flat_map(|x| x.to_le_bytes()).collect(),
            [1.5, -2., 0., 1.],
        ),
        (
            Rgba32Float,
            [1.5f32, -2., 0., 1.]
                .iter()
                .flat_map(|x| x.to_le_bytes())
                .collect(),
            [1.5, -2., 0., 1.],
        ),
    ];
    let mut gate = GateReport::new("uncompressed hand vectors", Tolerance::Exact);
    for (f, b, want) in cases {
        let got = Decoder::default().decode_rgba32f(f, 1, 1, &b).expect("decode");
        gate.record(
            format!("{f:?}"),
            got.iter()
                .zip(want)
                .map(|(a, b)| (a - b).abs())
                .fold(0., f32::max) as f64,
        );
    }
    println!("{gate}");
    gate.assert_passed();
}
/// Native bcdec.h oracle, 14 valid and 4 reserved mode tags, 16 blocks/tag,
/// signed and unsigned. Compare float bit patterns without HDR clipping.
#[test]
fn native_bc6_all_modes() {
    let blocks = include_bytes!("fixtures/bc6.blocks");
    for (f, expected) in [
        (
            TexFormat::Bc6hUfloat,
            include_bytes!("fixtures/bc6.0.rgb32f").as_slice(),
        ),
        (
            TexFormat::Bc6hSfloat,
            include_bytes!("fixtures/bc6.1.rgb32f").as_slice(),
        ),
    ] {
        let mut gate = GateReport::new(format!("{f:?} native bcdec"), Tolerance::Exact);
        for (i, b) in blocks.as_chunks::<16>().0.iter().enumerate() {
            let rgba = Decoder::default().decode_rgba32f(f, 4, 4, b).expect("decode");
            let rgb: Vec<u8> = rgba
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| p[..3].iter().flat_map(|x| x.to_le_bytes()))
                .collect();
            gate.compare_bytes(i.to_string(), &rgb, &expected[i * 192..(i + 1) * 192]);
        }
        println!("{gate}");
        gate.assert_passed();
    }
}
#[test]
fn reserved_bc7_is_transparent_black() {
    // Pillow 11 emits opaque black here; bcdec and D3D require transparent black.
    for d in [&Decoder::default() as &dyn TexDecoder, &Bcdec] {
        assert_eq!(
            d.decode_rgba8(TexFormat::Bc7, 4, 4, &[0; 16]).expect("reserved"),
            vec![0; 64]
        );
    }
}
