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

/// ExpressionShaderRasterPermutationSharedData.
pub const SHARED_RASTER: u32 = 0x2500_47B2;
/// ExpressionShaderPermutationSharedData (a compute permutation's).
pub const SHARED_COMPUTE: u32 = 0x879B_A711;

/// What a permutation's shared-data resource names ([`SHARED_RASTER`], [`SHARED_COMPUTE`]; both lay out the same way):
/// the ids of the pass binding, the binding signature, up to three binding sets and the root slot initial data, as
/// the first 0x40 bytes of the payload state them (an absent one is 0), and the bytes after them as stored. The sets'
/// slots are the ones [`crate::binding::Bindings`] maps registers through.
///
/// ```text
/// 0x00 u64 pass binding id    0x08 u64 binding signature id   0x10 two u32 (kept)
/// 0x18 u64 set 0 id           0x20 u64 set 1 id               0x28 u64 set 2 id
/// 0x30 u64 (kept)             0x38 u64 root slot initial data id
/// ```
///
/// **The bytes from 0x40 are fixed-function pipeline state, not decoded.** Measured on the 513 distinct raster
/// permutations of the drawing programs of a sample of 15 bundles: they take 13 distinct values, of 148, 168, 184,
/// 188, 204, 284 or 300 bytes (the payload is the metadata's size plus its relocation size, 0x40 bytes more), and
/// the two words at 0x10 and 0x30 take 4. The root slot initial data (12 distinct, 160 bytes each) and the binding
/// signature (one, 272 bytes) are other resources of the family, not read. The state's layout, in payload offsets,
/// as every one of the 13 shows it:
///
/// ```text
/// 0x40 u64 an id or hash   0x48 u32 4   0x4c u32 1, 10 or 11   0x50 u32 0xc0   0x54 20 zero bytes
///      (the 148-byte value has zero from 0x40 to 0x67)
/// 0x68 four bytes 00 00 08 01 or 00 04 08 01, then u32 0 or 1     0x70 u64 (three values)
/// 0x78 seven zero bytes and 0x1c or 0x18     0x80 four bytes, then 32 00 32 04, 38 00 38 04 or 38 00 38 44,
///      then ff 00 00 00
/// 0x8c eight 5-byte records, one for each render target: 02 00 01 00 0f in all of them but the first record of the
///      two 284- and 300-byte values (03 05 01 05 0f), which are the 16 permutations of the second pass binding
/// 0xb4 u32 1, 2 or 4   0xb8 u32 offset (0xc0 to 0x130) of a tail of 16-byte entries and the relocations
/// ```
///
/// What the game's own enum tables settle (the `Skate.exe` of build sha256 `fbce74d5e28ef525` names them in value
/// order, at file offset 0x64d7cb8 on: `RenderCullMode` None, Front, Back; `RenderBlendMode` Zero, One, SourceColor,
/// InvSourceColor, SourceAlpha, InvSourceAlpha, DestColor, InvDestColor, DestAlpha, InvDestAlpha, SourceAlphaSaturate,
/// Constant, InvConstant and the four Source1 modes; `RenderBlendOp` Add, Subtract, RevSubtract, Min, Max;
/// `RenderWriteMask` Red, Green, Blue, Alpha, None, Color, All; `RenderDepthMode` Disabled, ReadOnly, ReadAndWrite,
/// WriteOnly, ReadOnlyInverted, ReadAndWriteInverted, ReadOnlyEqual; `DepthStencilCompareFunc` Never, Less, Equal,
/// LessEqual, Greater, NotEqual, GreaterEqual, Always): the last byte of a record, 0x0f in all 8 records of all 13
/// values, is the write mask All, and eight records are the game's render target count. **Not settled:** which byte
/// of a record is the blend enable, the factors and the operation (a reading that fits the two kinds of record: byte
/// 0 a flags byte whose bit 0 is the blend enable, bytes 1 and 3 the destination factors, byte 2 the source factor;
/// the blended record would then be premultiplied alpha, source One, destination InvSourceAlpha), where the depth
/// mode, the cull mode, the fill mode and the stencil lie, and what the u64s and the tail hold. None of it is mapped
/// onto [`primase_core::shader::RenderState`]: a target stated without a blend would claim it is not blended, so
/// the binding leaves the state as it was until an anchor (a material known to blend, or the game's code that turns
/// these bytes into its pipeline state) confirms the fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedData {
    pub id: u64,
    pub pass_binding: Option<u64>,
    pub signature: u64,
    pub sets: [Option<u64>; 3],
    pub root_slots: u64,
    /// The words at 0x10 and 0x30.
    pub words: [u64; 2],
    /// The payload from 0x40.
    pub rest: Vec<u8>,
}

/// Read a shared-data resource of type `rtype` ([`SHARED_RASTER`] or [`SHARED_COMPUTE`]). Refused: another type, a
/// payload that is not the metadata's size and relocation size, one shorter than 0x40 bytes, a signature or root slot
/// id of 0.
pub fn read_shared(rtype: u32, d: &[u8], meta: &[u8; 16]) -> Result<SharedData> {
    if rtype != SHARED_RASTER && rtype != SHARED_COMPUTE {
        return Err(bad(
            "shared data",
            0,
            format!("type {rtype:#010x} is not shared data"),
        ));
    }
    let m = Rd::new(meta, "shared data");
    let (size, reloc) = (m.u32le(0)? as usize, m.u32le(4)? as usize);
    if size.checked_add(reloc) != Some(d.len()) {
        return Err(bad(
            "shared data",
            0,
            format!("size {size} + relocations {reloc} != {}", d.len()),
        ));
    }
    let r = Rd::new(d, "shared data");
    let some = |at: usize| -> Result<Option<u64>> { Ok(Some(r.u64le(at)?).filter(|&v| v != 0)) };
    let signature = r.u64le(8)?;
    let root_slots = r.u64le(0x38)?;
    if signature == 0 || root_slots == 0 {
        return Err(bad(
            "shared data",
            8,
            "a shared data without a signature or root slots",
        ));
    }
    Ok(SharedData {
        id: resource_id(meta),
        pass_binding: some(0)?,
        signature,
        sets: [some(0x18)?, some(0x20)?, some(0x28)?],
        root_slots,
        words: [r.u64le(0x10)?, r.u64le(0x30)?],
        rest: r.bytes(0x40, d.len() - 0x40)?.to_vec(),
    })
}
