//! Independent Pillow channel-statistics oracle for the source DDS census.
//! BC interpolation conventions may differ by one UNORM8 step. BC4's Pillow
//! grayscale expansion is compared on R only, not its replicated G/B channels.
mod inputs;

use inputs::{Game, OracleSet, oracle};
use primase_core::TexFormat;
use primase_texcodec::TexDecoder;
use serde::Deserialize;
#[derive(Deserialize)]
struct Record {
    name: String,
    width: u32,
    height: u32,
    min: [u8; 4],
    max: [u8; 4],
    mean: [f64; 4],
}
#[test]
fn source_channel_statistics() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = oracle(OracleSet::Scripts, Game::Sekiro, "pbr") else {
        return Ok(());
    };
    let records: Vec<Record> = serde_json::from_slice(&std::fs::read(root.join("texture-statistics.json"))?)?;
    let mut pixels = 0u64;
    for r in &records {
        let t = primase_dds::read(&std::fs::read(
            root.join("census/dds").join(format!("{}.dds", r.name)),
        )?)?;
        assert_eq!([t.width, t.height], [r.width, r.height], "{} dimensions", r.name);
        let rgba =
            primase_texcodec::Decoder::default().decode_rgba8(t.format, t.width, t.height, &t.levels[0])?;
        let mut min = [255u8; 4];
        let mut max = [0u8; 4];
        let mut sums = [0u64; 4];
        for p in rgba.as_chunks::<4>().0 {
            for c in 0..4 {
                min[c] = min[c].min(p[c]);
                max[c] = max[c].max(p[c]);
                sums[c] += p[c] as u64;
            }
        }
        let n = u64::from(t.width) * u64::from(t.height);
        pixels += n;
        for c in 0..4 {
            if t.format == TexFormat::Bc4Unorm && (c == 1 || c == 2) {
                continue;
            }
            assert!(min[c].abs_diff(r.min[c]) <= 1, "{} channel {c} minimum", r.name);
            assert!(max[c].abs_diff(r.max[c]) <= 1, "{} channel {c} maximum", r.name);
            assert!(
                (sums[c] as f64 / n as f64 - r.mean[c]).abs() <= 1.,
                "{} channel {c} mean",
                r.name
            );
        }
    }
    assert!(!records.is_empty(), "empty oracle is not coverage");
    println!(
        "{} source DDS, {pixels} base-mip texels; extrema and means within one UNORM8 step",
        records.len()
    );
    Ok(())
}
