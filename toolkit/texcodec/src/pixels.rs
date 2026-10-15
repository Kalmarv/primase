use crate::{Error, Result, buffer};
use primase_core::TexFormat;
fn u16le(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}
fn u32le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}
pub(crate) fn decode(f: TexFormat, data: &[u8], n: usize) -> Result<Vec<f32>> {
    use TexFormat::*;
    let mut out = buffer(n)?;
    for (b, p) in data
        .chunks_exact(f.block_bytes() as usize)
        .zip(out.as_chunks_mut::<4>().0)
    {
        p[3] = 1.;
        match f {
            R8Unorm | Rg8Unorm | Rgba8Unorm => {
                for (c, &v) in b.iter().enumerate() {
                    p[c] = v as f32 / 255.;
                }
            }
            R8Snorm | Rg8Snorm | Rgba8Snorm => {
                for (c, &v) in b.iter().enumerate() {
                    p[c] = (v as i8).max(-127) as f32 / 127.;
                }
            }
            A8Unorm => {
                p[3] = b[0] as f32 / 255.;
            }
            Bgra8Unorm | Bgrx8Unorm => {
                p[0] = b[2] as f32 / 255.;
                p[1] = b[1] as f32 / 255.;
                p[2] = b[0] as f32 / 255.;
                if f == Bgra8Unorm {
                    p[3] = b[3] as f32 / 255.;
                }
            }
            B5g6r5Unorm | B5g5r5a1Unorm | B4g4r4a4Unorm => {
                let v = u16le(b) as u32;
                let (bits, shifts) = match f {
                    B5g6r5Unorm => ([5, 6, 5, 0], [11, 5, 0, 0]),
                    B5g5r5a1Unorm => ([5, 5, 5, 1], [10, 5, 0, 15]),
                    _ => ([4, 4, 4, 4], [8, 4, 0, 12]),
                };
                for c in 0..4 {
                    if bits[c] > 0 {
                        let max = (1u32 << bits[c]) - 1;
                        p[c] = ((v >> shifts[c]) & max) as f32 / max as f32;
                    }
                }
            }
            Rgb10a2Unorm => {
                let v = u32le(b);
                for (c, value) in p.iter_mut().take(3).enumerate() {
                    *value = ((v >> (c * 10)) & 1023) as f32 / 1023.;
                }
                p[3] = (v >> 30) as f32 / 3.;
            }
            R16Unorm | Rg16Unorm | Rgba16Unorm | Rg16Snorm | R16Float | Rg16Float | Rgba16Float => {
                for (c, v) in b.as_chunks::<2>().0.iter().enumerate() {
                    let v = u16le(v);
                    p[c] = match f {
                        R16Float | Rg16Float | Rgba16Float => half::f16::from_bits(v).to_f32(),
                        Rg16Snorm => (v as i16).max(-32767) as f32 / 32767.,
                        _ => v as f32 / 65535.,
                    };
                }
            }
            R32Float | Rg32Float | Rgba32Float => {
                for (c, v) in b.as_chunks::<4>().0.iter().enumerate() {
                    p[c] = f32::from_bits(u32le(v));
                }
            }
            _ => return Err(Error::Unsupported(f)),
        }
    }
    Ok(out)
}
