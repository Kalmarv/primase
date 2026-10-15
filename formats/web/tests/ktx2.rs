#![cfg(feature = "ktx2")]
#![allow(clippy::unwrap_used)]

use basisu::{DecodeFlags, TargetFormat, Transcoder};
use primase_web::ktx2::encode_rgba8;

#[test]
fn encode_in_child_process() {
    let Ok(path) = std::env::var("PRIMASE_KTX2_TEST_OUTPUT") else {
        return;
    };
    let rgba: Vec<u8> = (0..16 * 16)
        .flat_map(|i| [(i % 256) as u8, 128, 255, ((i * 37) % 256) as u8])
        .collect();
    std::fs::write(path, encode_rgba8(16, 16, &rgba, true).unwrap()).unwrap();
}

#[test]
fn separate_process_determinism() {
    use sha2::{Digest, Sha256};
    let exe = std::env::current_exe().unwrap();
    let mut hashes = Vec::new();
    for run in 0..3 {
        let path = std::env::temp_dir().join(format!("primase-ktx2-{}-{run}.ktx2", std::process::id()));
        let child = std::process::Command::new(&exe)
            .args(["--exact", "encode_in_child_process"])
            .env("PRIMASE_KTX2_TEST_OUTPUT", &path)
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        hashes.push(Sha256::digest(std::fs::read(&path).unwrap()));
        std::fs::remove_file(path).unwrap();
    }
    assert!(hashes.windows(2).all(|h| h[0] == h[1]));
    let hex: String = hashes[0].iter().map(|b| format!("{b:02x}")).collect();
    eprintln!("three separate processes: SHA256={hex}");
}

#[test]
fn independent_decoder_and_repeatability() {
    for srgb in [false, true] {
        let rgba: Vec<u8> = (0..32 * 32)
            .flat_map(|i| [((i % 32) * 8) as u8, ((i / 32) * 8) as u8, 128, 255])
            .collect();
        let encoded = encode_rgba8(32, 32, &rgba, srgb).unwrap();
        assert_eq!(encoded, encode_rgba8(32, 32, &rgba, srgb).unwrap());
        let decoder = Transcoder::new(&encoded).unwrap();
        assert_eq!(decoder.level_count(), 6);
        for level in 0..6 {
            let pixels = decoder
                .transcode(level, TargetFormat::Rgba32, DecodeFlags::NONE)
                .unwrap();
            let dimension = (32usize >> level).max(1);
            assert_eq!(pixels.len(), dimension * dimension * 4);
        }
        let decoded = decoder
            .transcode(0, TargetFormat::Rgba32, DecodeFlags::NONE)
            .unwrap();
        assert_eq!(decoded.len(), rgba.len());
        let max = decoded
            .iter()
            .zip(&rgba)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        eprintln!(
            "KTX2 srgb={srgb}: {} bytes, gradient max channel error={max}",
            encoded.len()
        );
        assert!(max <= 8, "gradient channel error {max} exceeds 8");
    }
}

#[test]
fn reject_invalid_image_sizes() {
    assert!(encode_rgba8(0, 1, &[], false).is_err());
    assert!(encode_rgba8(u32::MAX, 2, &[], false).is_err());
    assert!(encode_rgba8(1, 1, &[0; 3], false).is_err());
}

#[test]
fn normal_angular_error() {
    let rgba: Vec<u8> = (0..32 * 32)
        .flat_map(|i| {
            let x = (i % 32) as f64 / 31. - 0.5;
            let y = (i / 32) as f64 / 31. - 0.5;
            let z = (1. - x * x - y * y).sqrt();
            [
                ((x + 1.) * 127.5).round() as u8,
                ((y + 1.) * 127.5).round() as u8,
                ((z + 1.) * 127.5).round() as u8,
                255,
            ]
        })
        .collect();
    let encoded = encode_rgba8(32, 32, &rgba, false).unwrap();
    let decoder = Transcoder::new(&encoded).unwrap();
    let decoded = decoder
        .transcode(0, TargetFormat::Rgba32, DecodeFlags::NONE)
        .unwrap();
    let vector = |p: &[u8; 4]| {
        let v = [
            p[0] as f64 / 127.5 - 1.,
            p[1] as f64 / 127.5 - 1.,
            p[2] as f64 / 127.5 - 1.,
        ];
        let length = v.iter().map(|v| v * v).sum::<f64>().sqrt();
        v.map(|v| v / length)
    };
    let mut max = 0f64;
    for (a, b) in rgba.as_chunks::<4>().0.iter().zip(decoded.as_chunks::<4>().0) {
        let (a, b) = (vector(a), vector(b));
        let angle = a
            .iter()
            .zip(b)
            .map(|(a, b)| a * b)
            .sum::<f64>()
            .clamp(-1., 1.)
            .acos()
            .to_degrees();
        max = max.max(angle);
    }
    eprintln!("1024 normal samples: maximum angle {max} degrees");
    assert!(max <= 3., "normal angular error exceeds 3 degrees");
}
