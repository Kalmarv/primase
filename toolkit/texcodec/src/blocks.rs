use crate::{BcArithmetic, Error, Result};
use primase_core::TexFormat;
fn word(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}
fn indices(b: &[u8]) -> u64 {
    b.iter()
        .enumerate()
        .fold(0, |v, (i, &b)| v | ((b as u64) << (8 * i)))
}
fn expand(v: u32, bits: u32, mode: BcArithmetic) -> u32 {
    match mode {
        BcArithmetic::Pillow => (v << (8 - bits)) | (v >> (2 * bits - 8)),
        BcArithmetic::CodeWalker => {
            let n = v * 255 + (1 << (bits - 1));
            (n / (1 << bits) + n) / (1 << bits)
        }
        BcArithmetic::Bcdec => {
            if bits == 5 {
                (v * 527 + 23) >> 6
            } else {
                (v * 259 + 33) >> 6
            }
        }
    }
}
fn palette(b: &[u8], opaque: bool, mode: BcArithmetic) -> [[u8; 4]; 4] {
    let c0 = word(b);
    let c1 = word(&b[2..]);
    let a = [(c0 >> 11) as u32, ((c0 >> 5) & 63) as u32, (c0 & 31) as u32];
    let z = [(c1 >> 11) as u32, ((c1 >> 5) & 63) as u32, (c1 & 31) as u32];
    let four = opaque || c0 > c1;
    let mut p = [[0, 0, 0, 255]; 4];
    for c in 0..3 {
        let bits = if c == 1 { 6 } else { 5 };
        let x = expand(a[c], bits, mode);
        let y = expand(z[c], bits, mode);
        p[0][c] = x as u8;
        p[1][c] = y as u8;
        if mode != BcArithmetic::Bcdec {
            p[2][c] = if four { (2 * x + y) / 3 } else { (x + y) / 2 } as u8;
            p[3][c] = if four { (x + 2 * y) / 3 } else { 0 } as u8;
        } else if four {
            let interp = |v| {
                if bits == 5 {
                    (v * 351 + 61) >> 7
                } else {
                    (v * 2763 + 1039) >> 11
                }
            };
            p[2][c] = interp(2 * a[c] + z[c]) as u8;
            p[3][c] = interp(a[c] + 2 * z[c]) as u8;
        } else {
            p[2][c] = if bits == 5 {
                ((a[c] + z[c]) * 1053 + 125) >> 8
            } else {
                ((a[c] + z[c]) * 4145 + 1019) >> 11
            } as u8;
        }
    }
    if !four {
        p[3] = [0; 4];
    }
    p
}
fn unorm(b: &[u8], rounded: bool, bias: u32) -> [u8; 16] {
    let a = b[0] as u32;
    let z = b[1] as u32;
    let mut p = [0u8; 8];
    p[0] = b[0];
    p[1] = b[1];
    let count = if a > z { 7 } else { 5 };
    for i in 1..count {
        let n = (count - i) * a + i * z;
        // bcdec BC4/5 use 16.16 weights, unlike its BC3 alpha implementation.
        p[(i + 1) as usize] = if rounded {
            let weights: &[u32] = if count == 7 {
                &[9363, 18724, 28086, 37450, 46812, 56173]
            } else {
                &[13107, 26215, 39321, 52429]
            };
            ((weights[(count - i - 1) as usize] * a + weights[(i - 1) as usize] * z + 32768) >> 16) as u8
        } else {
            ((n + bias) / count) as u8
        };
    }
    if count == 5 {
        p[6] = 0;
        p[7] = 255;
    }
    let bits = indices(&b[2..8]);
    std::array::from_fn(|i| p[((bits >> (i * 3)) & 7) as usize])
}
pub(crate) fn snorm(b: &[u8]) -> [f32; 16] {
    let a = (b[0] as i8).max(-127) as f64 / 127.;
    let z = (b[1] as i8).max(-127) as f64 / 127.;
    let mut p = [0.; 8];
    p[0] = a;
    p[1] = z;
    let count = if a > z { 7 } else { 5 };
    for i in 1..count {
        p[i + 1] = ((count - i) as f64 * a + i as f64 * z) / count as f64;
    }
    if count == 5 {
        p[6] = -1.;
        p[7] = 1.;
    }
    let bits = indices(&b[2..8]);
    std::array::from_fn(|i| p[((bits >> (i * 3)) & 7) as usize] as f32)
}
pub(crate) fn decode(f: TexFormat, b: &[u8], mode: BcArithmetic, oracle: bool) -> Result<[u8; 64]> {
    use TexFormat::*;
    let mut out = [0; 64];
    for p in out.as_chunks_mut::<4>().0 {
        p[3] = 255;
    }
    if oracle || f == Bc7 {
        match f {
            Bc1 => bcdec_rs::bc1(b, &mut out, 16),
            Bc2 => bcdec_rs::bc2(b, &mut out, 16),
            Bc3 => bcdec_rs::bc3(b, &mut out, 16),
            Bc7 => bcdec_rs::bc7(b, &mut out, 16),
            Bc4Unorm | Bc5Unorm => {
                let channels = if f == Bc4Unorm { 1 } else { 2 };
                for c in 0..channels {
                    let mut v = [0; 16];
                    bcdec_rs::bc4(&b[c * 8..], &mut v, 4, false);
                    for i in 0..16 {
                        out[i * 4 + c] = v[i];
                    }
                }
            }
            _ => return Err(Error::Unsupported(f)),
        }
        return Ok(out);
    }
    match f {
        Bc1 | Bc2 | Bc3 => {
            let offset = if f == Bc1 { 0 } else { 8 };
            let p = palette(&b[offset..], f != Bc1, mode);
            let bits = indices(&b[offset + 4..offset + 8]);
            let alpha = if f == Bc3 {
                unorm(b, false, u32::from(mode == BcArithmetic::Bcdec))
            } else {
                [0; 16]
            };
            for i in 0..16 {
                out[i * 4..i * 4 + 4].copy_from_slice(&p[((bits >> (i * 2)) & 3) as usize]);
                if f == Bc2 {
                    out[i * 4 + 3] = ((b[i / 2] >> ((i % 2) * 4)) & 15) * 17;
                }
                if f == Bc3 {
                    out[i * 4 + 3] = alpha[i];
                }
            }
        }
        Bc4Unorm | Bc5Unorm => {
            for c in 0..if f == Bc4Unorm { 1 } else { 2 } {
                let v = unorm(&b[c * 8..], mode == BcArithmetic::Bcdec, 0);
                for i in 0..16 {
                    out[i * 4 + c] = v[i];
                }
            }
        }
        _ => return Err(Error::Unsupported(f)),
    }
    Ok(out)
}
