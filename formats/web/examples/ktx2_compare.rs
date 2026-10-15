//! Compare encoder settings on a normal PNG, or quality 100 on a directory of capped colour PNGs.
#[cfg(feature = "ktx2")]
#[path = "support/quality.rs"]
mod quality_stats;

#[cfg(feature = "ktx2")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use basisu::{DecodeFlags, TargetFormat, Transcoder};
    use basisu_c_sys::{
        BasisTextureFormat, common,
        extra::{
            BasisuEncoder, BasisuEncoderParams, SourceImage, SourceImageFormat, basisu_encoder_init,
            types::Extent3d,
        },
    };
    let path = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("supply a capped normal PNG or colour directory")?,
    );
    let colour = path.is_dir();
    let mut paths = if colour {
        std::fs::read_dir(&path)?
            .map(|p| p.map(|p| p.path()))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        vec![path]
    };
    paths.retain(|p| p.extension().is_some_and(|e| e == "png"));
    paths.sort();
    let settings = if colour {
        vec![(100, 2)]
    } else {
        vec![(75, 2), (100, 2), (100, 5), (100, 10)]
    };
    basisu_encoder_init();
    for (quality, effort) in settings {
        let mut stats = quality_stats::Quality::default();
        let mut total_bytes = 0usize;
        for path in &paths {
            let image = primase_image::read_png(&std::fs::read(path)?)?.image;
            let primase_image::Samples::U8(rgba) = image.samples else {
                return Err("RGBA8 required".into());
            };
            let mut encoder = BasisuEncoder::new();
            encoder.set_image(SourceImage {
                data: &rgba,
                format: SourceImageFormat::Rgba8,
                size: Extent3d {
                    width: image.width,
                    height: image.height,
                    depth_or_array_layers: 1,
                },
            })?;
            let mut params = (if colour {
                BasisuEncoderParams::new_with_srgb_defaults(BasisTextureFormat::UastcLdr4x4)
            } else {
                BasisuEncoderParams::new_with_linear_defaults(BasisTextureFormat::UastcLdr4x4)
            })
            .with_removed_flags(common::BU_COMP_FLAGS_THREADED)
            .with_flags(common::BU_COMP_FLAGS_GEN_MIPS_WRAP);
            params.quality_level = quality;
            params.effort_level = effort;
            let encoded = encoder.compress(params)?;
            let decoder = Transcoder::new(&encoded).map_err(|e| format!("{e:?}"))?;
            let decoded = decoder
                .transcode(0, TargetFormat::Rgba32, DecodeFlags::NONE)
                .map_err(|e| format!("{e:?}"))?;
            stats.add(&rgba, &decoded, !colour);
            total_bytes += encoded.len();
        }
        println!(
            "{}",
            serde_json::json!({"quality":quality,"effort":effort,"bytes":total_bytes,"statistics":stats.report()})
        );
    }
    Ok(())
}

#[cfg(not(feature = "ktx2"))]
fn main() {
    eprintln!("enable the ktx2 feature to compare encoder settings");
}
