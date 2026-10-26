//! Processed 64-byte skin records, from the native preprocessing writer (0x430e38 / 0x430b28).
use super::{attributes::Attributes, scene::Bytes};
use crate::{
    Result,
    components::{SourceBone, SourceSkinVertex, VertexInput},
};
use std::ops::Range;

pub(super) struct Decoded {
    pub positions: Vec<[f32; 3]>,
    pub attributes: Attributes,
    pub vertices: Vec<SourceSkinVertex>,
    pub stream: Range<usize>,
    pub batches: usize,
}

pub(super) fn skeleton(scp: Bytes<'_>) -> Result<Vec<SourceBone>> {
    let count = scp.u(0x20)?;
    let table = scp.u(0x24)?;
    scp.require(0x20, count > 0, "processed skin has no source skeleton")?;
    scp.table(table, count, 16)?;
    let mut bones = Vec::with_capacity(count);
    for i in 0..count {
        let at = table + i * 16;
        let name = scp.u(at + 8)?;
        let tail = scp.slice(name, scp.data.len().saturating_sub(name))?;
        scp.require(name, tail.contains(&0), "unterminated source bone name")?;
        let length = tail.iter().position(|&x| x == 0).expect("checked terminator");
        bones.push(SourceBone {
            words: [scp.u(at)? as u32, scp.u(at + 4)? as u32],
            name: tail[..length].to_vec(),
            matrix: scp.matrix(scp.u(at + 12)?)?,
        });
    }
    Ok(bones)
}

fn short(b: Bytes<'_>, at: usize) -> Result<usize> {
    let bytes = b.slice(at, 2)?;
    Ok(usize::from(u16::from_be_bytes([bytes[0], bytes[1]])))
}

pub(super) fn decode(
    b: Bytes<'_>,
    ep: usize,
    count: usize,
    bones: usize,
    bounds: [[f32; 3]; 2],
) -> Result<Decoded> {
    let size = b.u(ep)?;
    b.slice(ep, size)?;
    b.require(
        ep,
        size >= 128 && count <= size / 64,
        "invalid processed skin size/count",
    )?;
    let end = ep + size;
    b.require(
        ep + 4,
        b.slice(ep + 4, 60)?
            .as_chunks::<4>()
            .0
            .iter()
            .all(|w| *w == [0xaa, 0xbb, 0xcc, 0xdd]),
        "processed skin envelope padding",
    )?;
    let mut result = Decoded {
        positions: Vec::with_capacity(count),
        attributes: Attributes::default(),
        vertices: Vec::with_capacity(count),
        stream: ep..end,
        batches: 0,
    };
    let mut basis = Vec::with_capacity(count);
    let mut uvs = [Vec::with_capacity(count), Vec::with_capacity(count)];
    let mut cursor = ep + 64;
    while result.vertices.len() < count {
        let header = b.slice(cursor, 64)?;
        result.batches += 1;
        b.require(cursor, cursor + 64 <= end, "processed batch exceeds stream")?;
        let palette: [u8; 4] = header[..4].try_into().expect("four palette bytes");
        let n = short(b, cursor + 4)?;
        let active = short(b, cursor + 6)?;
        b.require(
            cursor + 4,
            n > 0 && n <= count - result.vertices.len(),
            "processed batch vertex count",
        )?;
        b.require(
            cursor + 6,
            (1..=4).contains(&active),
            "processed batch influence count",
        )?;
        b.require(
            cursor,
            palette[..active]
                .iter()
                .all(|&i| i != 255 && usize::from(i) < bones)
                && palette[active..].iter().all(|&i| i == 255),
            "processed batch bone palette",
        )?;
        b.require(
            cursor + 8,
            header[8..]
                .as_chunks::<4>()
                .0
                .iter()
                .all(|w| *w == [0xdd, 0xcc, 0xbb, 0xaa]),
            "processed batch padding",
        )?;
        cursor += 64;
        for _ in 0..n {
            let bytes = b.slice(cursor, 64)?;
            b.require(cursor, cursor + 64 <= end, "processed vertex exceeds stream")?;
            let weights: [u8; 4] = bytes[12..16].try_into().expect("four weights");
            b.require(
                cursor + 12,
                weights.iter().map(|&w| u16::from(w)).sum::<u16>() == 255
                    && weights[active..].iter().all(|&w| w == 0),
                "processed weights must sum to 255 with inactive lanes zero",
            )?;
            let normal_bits = [
                b.u(cursor + 16)? as u32,
                b.u(cursor + 20)? as u32,
                b.u(cursor + 24)? as u32,
            ];
            let normal = normal_bits.map(f32::from_bits);
            if normal.iter().all(|n| n.is_finite()) {
                let length = normal.iter().map(|n| n * n).sum::<f32>().sqrt();
                b.require(
                    cursor + 16,
                    length == 0. || (length - 1.).abs() <= 0.005,
                    "nonunit processed normal (0.005 tolerance)",
                )?;
                result.attributes.zero_normals += usize::from(length == 0.);
            }
            let position = b.v3(cursor)?;
            b.require(
                cursor,
                (0..3).all(|k| position[k] >= bounds[0][k] - 0.05 && position[k] <= bounds[1][k] + 0.05),
                "processed position outside stored AABB",
            )?;
            result.positions.push(position);
            result.attributes.normals.push(normal);
            basis.push([
                b.f(cursor + 32)?,
                b.f(cursor + 36)?,
                b.f(cursor + 40)?,
                b.f(cursor + 44)?,
            ]);
            for (set, uv) in uvs.iter_mut().enumerate() {
                let p = 48 + set * 4;
                let value = [
                    half::f16::from_bits(u16::from_be_bytes([bytes[p], bytes[p + 1]])).to_f32(),
                    half::f16::from_bits(u16::from_be_bytes([bytes[p + 2], bytes[p + 3]])).to_f32(),
                ];
                b.require(
                    cursor + p,
                    value.iter().all(|v| v.is_finite()),
                    "nonfinite processed UV",
                )?;
                uv.push(value);
            }
            result.vertices.push(SourceSkinVertex {
                bones: palette,
                weights,
                normal_bits,
                extra: b.u(cursor + 28)? as u32,
                padding: bytes[56..64].try_into().expect("eight source padding bytes"),
            });
            cursor += 64;
        }
    }
    b.slice(cursor, 64)?;
    b.require(
        cursor,
        cursor + 64 == end && short(b, cursor + 4)? == 0 && short(b, cursor + 6)? == 0,
        "processed stream terminal/size mismatch",
    )?;
    result.attributes.raw.insert(
        "skin_basis".into(),
        VertexInput {
            lanes: 4,
            values: basis,
        },
    );
    for (set, uv) in uvs.into_iter().enumerate() {
        result.attributes.raw.insert(
            format!("stored_uv{set}"),
            VertexInput {
                lanes: 2,
                values: uv.iter().map(|v| [v[0], v[1], 0., 0.]).collect(),
            },
        );
        result.attributes.uvs[set] = Some(uv);
    }
    Ok(result)
}
