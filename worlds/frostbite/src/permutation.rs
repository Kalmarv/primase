//! Shader permutations (types [`RASTER`] and [`COMPUTE`], named `expressionshader/permutation<id>`): which compiled
//! programs a program database record's permutation runs. A resource is a relocatable payload (the 16-byte metadata
//! gives its size, its relocation size and the resource's own id) of the permutation's shared-data id, an array of
//! 16-byte GUIDs, and for a raster permutation its input layout id:
//!
//! ```text
//! raster  0x00 u64 shared data id   0x08 u64 pointer to the GUIDs (0x20)   0x10 u64 input layout id   0x18 u64
//!         0x20 GUID vertex program [, 0x30 GUID pixel program]            (the payload ends with the GUIDs)
//! compute 0x00 u64 shared data id   0x08 u64 pointer to the GUIDs (0x10)   0x10 GUID compute program
//! ```
//!
//! A GUID names a CompiledBytecode resource, `shaders/bytecode/<guid>` ([`bytecode_name`]); the ids are the numbers
//! other resources of the family carry in their names and metadata ([`resource_id`]). The raster permutations seen
//! hold a vertex and a pixel program, or a vertex program alone; a compute permutation one program. The word at 0x18
//! of a raster permutation is kept as stored (it is 0 in every one seen).

use crate::{Rd, Result, bad};

const W: &str = "shader permutation";
/// ExpressionShaderRasterPermutation.
pub const RASTER: u32 = 0xD489_FACB;
/// ExpressionShaderComputePermutation.
pub const COMPUTE: u32 = 0x95CE_62DD;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Raster,
    Compute,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Permutation {
    pub kind: Kind,
    /// The resource's own id.
    pub id: u64,
    /// The id of its ExpressionShaderPermutationSharedData resource (the raster family's or the compute family's).
    pub shared_data: u64,
    /// A raster permutation's ExpressionShaderInputLayout id.
    pub input_layout: Option<u64>,
    /// A raster permutation's word at 0x18, as stored.
    pub unknown_18: u64,
    /// The programs' GUIDs as stored, in stored order: vertex then pixel for a raster permutation.
    pub bytecode: Vec<[u8; 16]>,
}

/// The id a shader-family resource carries in the last eight bytes of its 16-byte metadata.
pub fn resource_id(meta: &[u8; 16]) -> u64 {
    u64::from_le_bytes([
        meta[8], meta[9], meta[10], meta[11], meta[12], meta[13], meta[14], meta[15],
    ])
}

/// The name of the CompiledBytecode resource a permutation's GUID names: `shaders/bytecode/` and the GUID as text,
/// its first three fields little endian as stored (`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`).
pub fn bytecode_name(g: &[u8; 16]) -> String {
    let field = |a: usize, b: usize| -> String {
        let mut bytes = g[a..b].to_vec();
        bytes.reverse();
        crate::hex(&bytes)
    };
    format!(
        "shaders/bytecode/{}-{}-{}-{}-{}",
        field(0, 4),
        field(4, 6),
        field(6, 8),
        crate::hex(&g[8..10]),
        crate::hex(&g[10..16])
    )
}

/// Read a permutation of type `rtype` ([`RASTER`] or [`COMPUTE`]) from its payload and metadata. Refused: another
/// type, a payload that is not the metadata's size and relocation list, a GUID array that does not start where the
/// pointer says or does not fill the payload, a size no permutation seen has.
pub fn read(rtype: u32, d: &[u8], meta: &[u8; 16]) -> Result<Permutation> {
    let kind = match rtype {
        RASTER => Kind::Raster,
        COMPUTE => Kind::Compute,
        t => return Err(bad(W, 0, format!("type {t:#010x} is not a permutation"))),
    };
    let m = Rd::new(meta, W);
    let (size, reloc) = (m.u32le(0)? as usize, m.u32le(4)? as usize);
    if size.checked_add(reloc) != Some(d.len()) {
        return Err(bad(
            W,
            0,
            format!("size {size} + relocations {reloc} != {}", d.len()),
        ));
    }
    let r = Rd::new(d, W);
    // the one relocation: the pointer at 8
    if reloc != 4 || r.u32le(size)? != 8 {
        return Err(bad(W, size, "relocations are not the pointer at 8 alone"));
    }
    let start = match kind {
        Kind::Raster => 0x20,
        Kind::Compute => 0x10,
    };
    if r.u64le(8)? != start as u64 || size <= start || !(size - start).is_multiple_of(16) {
        return Err(bad(
            W,
            8,
            format!(
                "GUIDs at {:#x} in a payload of {size}, not at {start:#x} to its end",
                r.u64le(8)?
            ),
        ));
    }
    let count = (size - start) / 16;
    if count > 2 || (kind == Kind::Compute && count != 1) {
        return Err(bad(
            W,
            start,
            format!("{count} programs in a {kind:?} permutation"),
        ));
    }
    Ok(Permutation {
        kind,
        id: resource_id(meta),
        shared_data: r.u64le(0)?,
        input_layout: (kind == Kind::Raster).then(|| r.u64le(0x10)).transpose()?,
        unknown_18: if kind == Kind::Raster { r.u64le(0x18)? } else { 0 },
        bytecode: (0..count)
            .map(|i| r.arr::<16>(start + 16 * i))
            .collect::<Result<_>>()?,
    })
}
