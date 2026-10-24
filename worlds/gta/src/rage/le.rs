//! GTA's byte order, and little-endian numbers decoded from fixed-size arrays already read. The checked reads at an
//! offset are [`primase_bytes::View`]'s: bytes a file says are there and are not (an offset past the slice, an offset
//! that does not fit) are the format's error ([`crate::Error::Invalid`]), never a panic.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
/// GTA's resources are little-endian: the byte order of every positional read
/// (`primase_bytes::View::new(d).u32(o, LE)`).
pub(crate) const LE: primase_bytes::Endian = primase_bytes::Endian::Little;

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

/// Four little-endian u32 of a 16-byte array.
pub(crate) fn u32x4_of(c: &[u8; 16]) -> [u32; 4] {
    let [a0, a1, a2, a3, b0, b1, b2, b3, c0, c1, c2, c3, d0, d1, d2, d3] = *c;
    [
        u32::from_le_bytes([a0, a1, a2, a3]),
        u32::from_le_bytes([b0, b1, b2, b3]),
        u32::from_le_bytes([c0, c1, c2, c3]),
        u32::from_le_bytes([d0, d1, d2, d3]),
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
    clippy::cast_possible_truncation,
    reason = "fixtures, not untrusted bytes"
)]
mod tests {
    use super::LE;
    use primase_bytes::View;

    /// Reads inside the slice return its bytes; one byte past, or an offset that does not fit, is the format's error,
    /// naming the offset and how many bytes there are.
    #[test]
    fn reads_past_the_bytes_are_errors() {
        let d = [1u8, 0, 0, 0, 0, 0, 0x80, 0x3F];
        let v = View::new(&d);
        assert_eq!(v.u32(0, LE).unwrap(), 1);
        assert_eq!(v.f32(4, LE).unwrap(), 1.0);
        assert!(v.u32(5, LE).is_err());
        assert!(v.u64(1, LE).is_err());
        assert!(v.u16(usize::MAX, LE).is_err());
        assert!(v.array::<12>(0).is_err());
        let e = crate::Error::from(v.u32(5, LE).unwrap_err());
        assert_eq!(e.to_string(), "bytes at 0x5: 4 bytes past the 8 there are");
    }
}
