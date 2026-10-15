//! First-use compaction of all vertex streams, without interpreting semantics.
use crate::{Error, Result, finite};
use primase_core::{Primitive, Stream, Topology};

#[derive(Clone, Debug, PartialEq)]
pub struct CompactedPrimitive {
    pub primitive: Primitive,
    /// Source vertex -> compacted vertex. Unreferenced vertices are None.
    pub old_to_new: Vec<Option<u32>>,
    /// Compacted vertex -> source vertex, in first index-use order.
    pub gather: Vec<u32>,
}
/// Preserve every semantic and stream representation, topology, material and
/// index order. Non-indexed primitives retain sequential storage and None indices.
/// Reject invalid lengths, indices, topology counts and non-finite attributes.
pub fn compact_primitive(source: &Primitive) -> Result<CompactedPrimitive> {
    let n = source
        .positions()
        .ok_or_else(|| Error("missing float3 positions".into()))?
        .len();
    if n > u32::MAX as usize {
        return Err(Error("vertex count exceeds u32".into()));
    }
    for stream in source.streams.values() {
        if stream.len() != n {
            return Err(Error("attribute length differs from positions".into()));
        }
        match stream {
            Stream::F32x2(v) => {
                for v in v {
                    finite(v)?;
                }
            }
            Stream::F32x3(v) => {
                for v in v {
                    finite(v)?;
                }
            }
            Stream::F32x4(v) => {
                for v in v {
                    finite(v)?;
                }
            }
            Stream::U8x4(_) | Stream::U16x4(_) => {}
        }
    }
    let count = source.indices.as_ref().map_or(n, Vec::len);
    let width = match source.topology {
        Topology::Triangles => 3,
        Topology::Lines => 2,
        Topology::Points => 1,
    };
    if !count.is_multiple_of(width) {
        return Err(Error("incomplete primitive".into()));
    }
    let mut old_to_new = vec![None; n];
    let mut gather = Vec::new();
    let mut remap = |old: u32| -> Result<u32> {
        let slot = old_to_new
            .get_mut(old as usize)
            .ok_or_else(|| Error("index outside vertex stream".into()))?;
        Ok(*slot.get_or_insert_with(|| {
            let new = gather.len() as u32;
            gather.push(old);
            new
        }))
    };
    let indices = if let Some(indices) = &source.indices {
        Some(indices.iter().map(|&i| remap(i)).collect::<Result<Vec<_>>>()?)
    } else {
        for i in 0..n {
            remap(i as u32)?;
        }
        None
    };
    let streams = source
        .streams
        .iter()
        .map(|(semantic, stream)| {
            macro_rules! collect {
                ($v:expr,$kind:ident) => {
                    Stream::$kind(gather.iter().map(|&i| $v[i as usize]).collect())
                };
            }
            let stream = match stream {
                Stream::F32x2(v) => collect!(v, F32x2),
                Stream::F32x3(v) => collect!(v, F32x3),
                Stream::F32x4(v) => collect!(v, F32x4),
                Stream::U8x4(v) => collect!(v, U8x4),
                Stream::U16x4(v) => collect!(v, U16x4),
            };
            (*semantic, stream)
        })
        .collect();
    Ok(CompactedPrimitive {
        primitive: Primitive {
            topology: source.topology,
            streams,
            indices,
            material: source.material,
        },
        old_to_new,
        gather,
    })
}
