//! Native table 0x1445920, packed variant selected by mask bit 0x80000; no tangent sign is inferred.
use crate::Result;
use crate::scene::Bytes;
use std::collections::BTreeMap;

/// Decoded source lanes, padded with zero to four components. Values retain their source encoding's meaning.
#[derive(Clone, Debug, PartialEq)]
pub struct VertexInput {
    pub lanes: u8,
    pub values: Vec<[f32; 4]>,
}

#[derive(Clone, Debug, Default)]
pub struct Attributes {
    /// Source normals: exact RSX CMP for rigid meshes, authored float3 for processed skins. No repair.
    pub normals: Vec<[f32; 3]>,
    pub colors: Option<Vec<[f32; 4]>>,
    pub uvs: [Option<Vec<[f32; 2]>>; 4],
    pub raw: BTreeMap<String, VertexInput>,
    pub zero_normals: usize,
}

/// RSX sign extends 11:11:10 lanes after shifting to i16 and divides by 32767.
pub fn cmp(word: u32) -> [f32; 3] {
    let signed = |shift: u32, bits: u32| {
        let lane = ((word >> shift) & ((1 << bits) - 1)) as i32;
        let lane = if lane & (1 << (bits - 1)) != 0 {
            lane - (1 << bits)
        } else {
            lane
        };
        (lane * (1 << (16 - bits))) as f32 / 32767.
    };
    [signed(0, 11), signed(11, 11), signed(22, 10)]
}

/// Byte size from the native declaration entries; unsupported flags/entries refuse this model.
pub(crate) fn stride(mask: u32) -> Option<usize> {
    if mask & !0x807ff != 0 || mask & 0x80005 != 0x80005 {
        return None;
    }
    Some(
        (1..=10)
            .filter(|&bit| bit != 2 && mask & (1 << bit) != 0)
            .map(|bit| if bit >= 9 { 8 } else { 4 })
            .sum(),
    )
}

pub(crate) fn decode(
    mask: u32,
    count: usize,
    cpu: Bytes<'_>,
    gpu: Bytes<'_>,
    gpu_stride: usize,
) -> Result<Attributes> {
    let mut a = Attributes::default();
    for row in cpu.data.as_chunks::<16>().0 {
        let n = cmp(u32::from_be_bytes(row[12..16].try_into().expect("vertex word")));
        let len = n.iter().map(|x| x * x).sum::<f32>().sqrt();
        cpu.require(
            a.normals.len() * 16 + 12,
            len == 0. || (len - 1.).abs() <= 0.005,
            "nonunit CMP normal (0.005 tolerance)",
        )?;
        a.zero_normals += usize::from(len == 0.);
        a.normals.push(n);
    }
    let mut offset = 0;
    for bit in (1..=10).filter(|&b| b != 2 && mask & (1 << b) != 0) {
        let mut values = Vec::with_capacity(count);
        let lanes = if bit == 1 || bit >= 9 {
            4
        } else if bit >= 5 {
            2
        } else {
            3
        };
        for i in 0..count {
            let p = i * gpu_stride + offset;
            let mut v = [0.; 4];
            if bit == 1 {
                for (dst, &src) in v.iter_mut().zip(gpu.slice(p, 4)?) {
                    *dst = f32::from(src) / 255.;
                }
            } else if bit <= 4 {
                v[..3].copy_from_slice(&cmp(gpu.u(p)? as u32));
            } else {
                for (k, dst) in v.iter_mut().take(lanes).enumerate() {
                    let bytes = gpu.slice(p + k * 2, 2)?;
                    *dst = half::f16::from_bits(u16::from_be_bytes([bytes[0], bytes[1]])).to_f32();
                    gpu.require(p + k * 2, dst.is_finite(), "nonfinite half vertex input")?;
                }
            }
            values.push(v);
        }
        if bit == 1 {
            a.colors = Some(values);
        } else {
            let name = match bit {
                3 => "attr15".into(),
                4 => "attr14".into(),
                5..=8 => format!("stored_uv{}", bit - 5),
                9 => "attr12".into(),
                10 => "attr13".into(),
                _ => unreachable!(),
            };
            if (5..=8).contains(&bit) {
                a.uvs[bit - 5] = Some(values.iter().map(|v| [v[0], v[1]]).collect());
            }
            a.raw.insert(
                name,
                VertexInput {
                    lanes: lanes as u8,
                    values,
                },
            );
        }
        offset += if bit >= 9 { 8 } else { 4 };
    }
    Ok(a)
}
