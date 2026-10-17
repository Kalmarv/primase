use crate::{Error, Image, Result, Samples, allocate, sample_count};

/// Rebuild positive-hemisphere Z from encoded RG, projecting XY onto the unit
/// disk when necessary. Returns whether XY was projected; alpha is untouched.
/// Decode and geometry use f64; all encoded channels round to nearest, ties-even.
/// This is the reference map writer's blue-channel rounding rule. Exhausting
/// all 65,536 XY pairs gives identical blue bytes with f32 or f64 arithmetic,
/// and with ties-away or ties-even rounding (before radial projection).
pub fn rebuild_normal_z(pixel: &mut [u8; 4]) -> bool {
    let x = f64::from(pixel[0]) / 127.5 - 1.;
    let y = f64::from(pixel[1]) / 127.5 - 1.;
    let radius_squared = x * x + y * y;
    let projected = radius_squared > 1.;
    let encode = |v: f64| ((v * 0.5 + 0.5) * 255.).round_ties_even() as u8;
    let z = if projected {
        let radius = radius_squared.sqrt();
        pixel[0] = encode(x / radius);
        pixel[1] = encode(y / radius);
        0.
    } else {
        (1. - radius_squared).sqrt()
    };
    pixel[2] = encode(z);
    projected
}

/// Scale a tangent-space normal's X and Y as glTF's `normalTexture.scale` does: the decoded vector
/// `(2 * byte / 255 - 1)` times `(scale, scale, 1)`, normalised (a zero vector becomes +Z), re-encoded with nearest,
/// ties-even rounding; alpha untouched. Returns whether the vector was zero.
pub fn scale_normal(pixel: &mut [u8; 4], scale: f32) -> bool {
    let decode = |b: u8| f64::from(b) / 127.5 - 1.;
    let s = f64::from(scale);
    let v = [decode(pixel[0]) * s, decode(pixel[1]) * s, decode(pixel[2])];
    let length = v.iter().map(|c| c * c).sum::<f64>().sqrt();
    let encode = |c: f64| ((c * 0.5 + 0.5) * 255.).round_ties_even() as u8;
    if length == 0. || !length.is_finite() {
        pixel[..3].copy_from_slice(&[encode(0.), encode(0.), encode(1.)]);
        return true;
    }
    for c in 0..3 {
        pixel[c] = encode(v[c] / length);
    }
    false
}

/// Vector-filtered image and the number of ambiguous averages replaced by +Z.
#[derive(Debug)]
pub struct NormalFilter {
    pub image: Image,
    pub near_zero_to_flat: u64,
}

/// Filter RGBA8 tangent-space normals in integer source bins (odd edges included).
/// Decode RGB to [-1,1], normalize each source vector, average, and normalize again.
/// A mean shorter than sqrt(3)/255, the RGB8 vector quantization uncertainty,
/// becomes (0,0,1). Alpha is averaged independently. No sRGB transfer is applied.
pub fn resize_normals(image: &Image, width: u32, height: u32) -> Result<NormalFilter> {
    image.validate()?;
    let mut out = allocate(sample_count(width, height)?)?;
    let Samples::U8(pixels) = &image.samples else {
        return Err(Error::Invalid("normal filtering requires RGBA8".into()));
    };
    let mut near_zero_to_flat = 0;
    for y in 0..height {
        for x in 0..width {
            let xa = u64::from(x) * u64::from(image.width) / u64::from(width);
            let xb = ((u64::from(x) + 1) * u64::from(image.width) / u64::from(width)).max(xa + 1);
            let ya = u64::from(y) * u64::from(image.height) / u64::from(height);
            let yb = ((u64::from(y) + 1) * u64::from(image.height) / u64::from(height)).max(ya + 1);
            let mut sum = [0.; 3];
            let mut alpha = 0.;
            for sy in ya..yb {
                for sx in xa..xb {
                    let p = &pixels[(sy as usize * image.width as usize + sx as usize) * 4..][..4];
                    let v = [p[0], p[1], p[2]].map(|v| f64::from(v) / 127.5 - 1.);
                    let length = v.iter().map(|v| v * v).sum::<f64>().sqrt();
                    for c in 0..3 {
                        sum[c] += v[c] / length;
                    }
                    alpha += f64::from(p[3]);
                }
            }
            let count = ((xb - xa) * (yb - ya)) as f64;
            let mean = sum.map(|v| v / count);
            let length = mean.iter().map(|v| v * v).sum::<f64>().sqrt();
            let normal = if length < 3f64.sqrt() / 255. {
                near_zero_to_flat += 1;
                [0., 0., 1.]
            } else {
                mean.map(|v| v / length)
            };
            let offset = (y as usize * width as usize + x as usize) * 4;
            for c in 0..3 {
                out[offset + c] = ((normal[c] + 1.) * 127.5).round().clamp(0., 255.) as u8;
            }
            out[offset + 3] = (alpha / count).round() as u8;
        }
    }
    Ok(NormalFilter {
        image: Image {
            width,
            height,
            samples: Samples::U8(out),
        },
        near_zero_to_flat,
    })
}

/// Original level followed by vector-filtered levels down to 1x1, with a total fallback count.
pub fn normal_mip_chain(image: &Image) -> Result<(Vec<Image>, u64)> {
    image.validate()?;
    if !matches!(image.samples, Samples::U8(_)) {
        return Err(Error::Invalid("normal filtering requires RGBA8".into()));
    }
    let mut chain = vec![image.clone()];
    let mut count = 0;
    while let Some(last) = chain.last() {
        if last.width == 1 && last.height == 1 {
            break;
        }
        let next = resize_normals(last, (last.width / 2).max(1), (last.height / 2).max(1))?;
        count += next.near_zero_to_flat;
        chain.push(next.image);
    }
    Ok((chain, count))
}
