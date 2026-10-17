//! Little-endian reads at a byte offset, checked: bytes a file says are there and are not (an offset past the slice,
//! an offset that does not fit) are the format's error, never a panic.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::{Result, invalid};

/// The `N` bytes at `o`.
fn bytes<const N: usize>(d: &[u8], o: usize) -> Result<[u8; N]> {
    o.checked_add(N)
        .and_then(|end| d.get(o..end))
        .and_then(|s| <[u8; N]>::try_from(s).ok())
        .ok_or_else(|| {
            invalid(
                "bytes",
                o as u64,
                format!("{N} bytes past the {} there are", d.len()),
            )
        })
}

pub(crate) fn u8(d: &[u8], o: usize) -> Result<u8> {
    bytes(d, o).map(u8::from_le_bytes)
}

pub(crate) fn u16(d: &[u8], o: usize) -> Result<u16> {
    bytes(d, o).map(u16::from_le_bytes)
}

pub(crate) fn u32(d: &[u8], o: usize) -> Result<u32> {
    bytes(d, o).map(u32::from_le_bytes)
}

pub(crate) fn u64(d: &[u8], o: usize) -> Result<u64> {
    bytes(d, o).map(u64::from_le_bytes)
}

pub(crate) fn f32(d: &[u8], o: usize) -> Result<f32> {
    bytes(d, o).map(f32::from_le_bytes)
}

pub(crate) fn vec3(d: &[u8], o: usize) -> Result<[f32; 3]> {
    let [a0, a1, a2, a3, b0, b1, b2, b3, c0, c1, c2, c3] = bytes(d, o)?;
    Ok([
        f32::from_le_bytes([a0, a1, a2, a3]),
        f32::from_le_bytes([b0, b1, b2, b3]),
        f32::from_le_bytes([c0, c1, c2, c3]),
    ])
}

pub(crate) fn vec4(d: &[u8], o: usize) -> Result<[f32; 4]> {
    let [a0, a1, a2, a3, b0, b1, b2, b3, c0, c1, c2, c3, d0, d1, d2, d3] = bytes(d, o)?;
    Ok([
        f32::from_le_bytes([a0, a1, a2, a3]),
        f32::from_le_bytes([b0, b1, b2, b3]),
        f32::from_le_bytes([c0, c1, c2, c3]),
        f32::from_le_bytes([d0, d1, d2, d3]),
    ])
}

/// Two little-endian f32 of an 8-byte array.
pub(crate) fn f32x2_of(c: &[u8; 8]) -> [f32; 2] {
    let [a0, a1, a2, a3, b0, b1, b2, b3] = *c;
    [
        f32::from_le_bytes([a0, a1, a2, a3]),
        f32::from_le_bytes([b0, b1, b2, b3]),
    ]
}

/// Three little-endian f32 of a 12-byte array.
pub(crate) fn vec3_of(c: &[u8; 12]) -> [f32; 3] {
    let [a0, a1, a2, a3, b0, b1, b2, b3, c0, c1, c2, c3] = *c;
    [
        f32::from_le_bytes([a0, a1, a2, a3]),
        f32::from_le_bytes([b0, b1, b2, b3]),
        f32::from_le_bytes([c0, c1, c2, c3]),
    ]
}

/// Four little-endian f32 of a 16-byte array.
pub(crate) fn vec4_of(c: &[u8; 16]) -> [f32; 4] {
    let [a0, a1, a2, a3, b0, b1, b2, b3, c0, c1, c2, c3, d0, d1, d2, d3] = *c;
    [
        f32::from_le_bytes([a0, a1, a2, a3]),
        f32::from_le_bytes([b0, b1, b2, b3]),
        f32::from_le_bytes([c0, c1, c2, c3]),
        f32::from_le_bytes([d0, d1, d2, d3]),
    ]
}

/// Two little-endian u32 of an 8-byte array.
pub(crate) fn u32x2_of(c: &[u8; 8]) -> [u32; 2] {
    let [a0, a1, a2, a3, b0, b1, b2, b3] = *c;
    [
        u32::from_le_bytes([a0, a1, a2, a3]),
        u32::from_le_bytes([b0, b1, b2, b3]),
    ]
}

/// Three little-endian u16 of a 6-byte array.
pub(crate) fn u16x3_of(c: &[u8; 6]) -> [u16; 3] {
    let [a0, a1, b0, b1, c0, c1] = *c;
    [
        u16::from_le_bytes([a0, a1]),
        u16::from_le_bytes([b0, b1]),
        u16::from_le_bytes([c0, c1]),
    ]
}

/// Eight little-endian u16 of a 16-byte array.
pub(crate) fn u16x8_of(c: &[u8; 16]) -> [u16; 8] {
    let [a0, a1, b0, b1, c0, c1, d0, d1, e0, e1, f0, f1, g0, g1, h0, h1] = *c;
    [
        u16::from_le_bytes([a0, a1]),
        u16::from_le_bytes([b0, b1]),
        u16::from_le_bytes([c0, c1]),
        u16::from_le_bytes([d0, d1]),
        u16::from_le_bytes([e0, e1]),
        u16::from_le_bytes([f0, f1]),
        u16::from_le_bytes([g0, g1]),
        u16::from_le_bytes([h0, h1]),
    ]
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    reason = "fixtures, not untrusted bytes"
)]
mod tests {
    /// Reads inside the slice return its bytes; one byte past, or an offset that does not fit, is an error.
    #[test]
    fn reads_past_the_bytes_are_errors() {
        let d = [1u8, 0, 0, 0, 0, 0, 0x80, 0x3F];
        assert_eq!(super::u32(&d, 0).unwrap(), 1);
        assert_eq!(super::f32(&d, 4).unwrap(), 1.0);
        assert!(super::u32(&d, 5).is_err());
        assert!(super::u64(&d, 1).is_err());
        assert!(super::u16(&d, usize::MAX).is_err());
        assert!(super::vec3(&d, 0).is_err());
    }
}
