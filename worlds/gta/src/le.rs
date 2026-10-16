//! Little-endian reads at a byte offset. Callers check bounds first (slices from `Resource::at` have the length
//! asked for), so an out-of-range offset is a bug and panics.

pub(crate) fn u16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}

pub(crate) fn u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

pub(crate) fn u64(d: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(d[o..o + 8].try_into().expect("8 bytes"))
}

pub(crate) fn f32(d: &[u8], o: usize) -> f32 {
    f32::from_bits(u32(d, o))
}

pub(crate) fn vec3(d: &[u8], o: usize) -> [f32; 3] {
    [f32(d, o), f32(d, o + 4), f32(d, o + 8)]
}

pub(crate) fn vec4(d: &[u8], o: usize) -> [f32; 4] {
    [f32(d, o), f32(d, o + 4), f32(d, o + 8), f32(d, o + 12)]
}
