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
/// **The bytes from 0x40 are kept as stored; those from 0x78 to 0xb4 are the fixed-function pipeline state**, decoded
/// by [`SharedData::fixed_state`] in the game's own layouts ([`FixedState`]). The rest of the layout, in payload
/// offsets:
///
/// ```text
/// 0x40 u64 an id or hash   0x48 u32 4   0x4c u32 1, 10 or 11   0x50 u32 0xc0   0x54 20 zero bytes
/// 0x68 four bytes 00 00 08 01 or 00 04 08 01, then u32 0 or 1     0x70 u64 (several values)
/// 0x88 ff 00 00 00         0xb4 u32 1, 2 or 4   0xb8 u32 offset of a tail of 16-byte entries and the relocations
/// ```
///
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

/// A comparison, in D3D12's order less one (`Never` is 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Compare {
    Never,
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    Always,
}

/// A stencil operation, in D3D12's order less one (`Keep` is 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StencilOp {
    Keep,
    Zero,
    Replace,
    /// Increment, clamped at the maximum.
    IncrementSaturate,
    /// Decrement, clamped at zero.
    DecrementSaturate,
    Invert,
    /// Increment, wrapping to zero.
    Increment,
    /// Decrement, wrapping to the maximum.
    Decrement,
}

/// A blend operation, in D3D12's order less one (`Add` is 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlendOp {
    Add,
    Subtract,
    ReverseSubtract,
    Min,
    Max,
}

/// Which triangles are culled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cull {
    None,
    Front,
    Back,
}

/// How a triangle is filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fill {
    Solid,
    Wireframe,
}

/// What the stencil test does for one facing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StencilFace {
    /// On a failed stencil test.
    pub fail: StencilOp,
    /// On a passed stencil test and a failed depth test.
    pub depth_fail: StencilOp,
    /// On both passed.
    pub pass: StencilOp,
    pub compare: Compare,
}

/// The fixed-function pipeline state a raster permutation's shared data packs ([`SharedData::fixed_state`]), field by
/// field as stored. The layouts are the game's own: `Skate.exe` (build sha256 `fbce74d5e28ef525`) prints the same
/// packed opcodes as JSON in its rc2 IR filter (`fb::rc2::IrJsonFilter`; the rasterizer, depth-stencil and blend
/// printers at RVA 0x4130300, 0x4130430 and 0x4130550, their format strings at 0x654b3f0, 0x654ba10, 0x6571b00 and
/// 0x6573b10, the enum names at 0x40cd450 (compare), 0x40ce1f0 (stencil op), 0x40cc8a0 (blend mode) and 0x40cc9a0
/// (blend op)). Offsets in the shared-data payload:
///
/// ```text
/// 0x78 f32 slope-scaled depth bias   0x7c i16 depth bias   0x7e u8 forced sample count
/// 0x7f bits: fill (0 solid, 1 wireframe) b0, cull b1-2 (none, front, back), front counter-clockwise b3,
///      depth clip b4, multisample b5, antialiased line b6, conservative b7
/// 0x80 bits: depth test b0, depth write b1, stencil test b2, depth compare b3-5
/// 0x81 u8 stencil read mask          0x82 u8 stencil write mask
/// 0x83 front face, 0x85 back face, two bytes each: fail op b0-2, depth-fail op b3-5; pass op b0-2, compare b3-5
/// 0x87 bits: logic op b0-3, logic op enable b4, alpha to coverage b5, independent blend b6
/// 0x8c eight 5-byte render target records: blend enable b0 and source b1-5; destination b0-4; source alpha b0-4;
///      destination alpha b0-4 and alpha op b5-7; write mask
/// ```
///
/// Compares (`Never` .. `Always`) and stencil ops (`Keep`, `Zero`, `Replace`, `IncrementSaturate`,
/// `DecrementSaturate`, `Invert`, `Increment`, `Decrement`) are D3D12's, in D3D12's order less one. Blend factors are
/// the game's `RenderBlendMode` codes (0 `Zero` .. 16 `InvSource1Alpha`), kept as stored. The printed record has no
/// colour blend operation (only the alpha one); bits 5-7 of the destination byte are refused unless zero.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FixedState {
    pub slope_scaled_depth_bias: f32,
    /// D3D12's integer depth bias.
    pub depth_bias: i16,
    pub forced_sample_count: u8,
    pub fill: Fill,
    pub cull: Cull,
    pub front_counter_clockwise: bool,
    pub depth_clip: bool,
    pub multisample: bool,
    pub antialiased_line: bool,
    pub conservative: bool,
    pub depth_test: bool,
    pub depth_write: bool,
    pub stencil_test: bool,
    pub depth_compare: Compare,
    pub stencil_read_mask: u8,
    pub stencil_write_mask: u8,
    pub front: StencilFace,
    pub back: StencilFace,
    pub logic_op: u8,
    pub logic_op_enable: bool,
    pub alpha_to_coverage: bool,
    pub independent_blend: bool,
    pub targets: [TargetRecord; 8],
}

/// One render target's blend record, as stored ([`FixedState`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetRecord {
    pub blend: bool,
    /// `RenderBlendMode` codes.
    pub source: u8,
    pub destination: u8,
    pub source_alpha: u8,
    pub destination_alpha: u8,
    pub alpha_op: BlendOp,
    /// Bit 0 red, 1 green, 2 blue, 3 alpha.
    pub write_mask: u8,
}

const COMPARES: [Compare; 8] = [
    Compare::Never,
    Compare::Less,
    Compare::Equal,
    Compare::LessEqual,
    Compare::Greater,
    Compare::NotEqual,
    Compare::GreaterEqual,
    Compare::Always,
];
const STENCIL_OPS: [StencilOp; 8] = [
    StencilOp::Keep,
    StencilOp::Zero,
    StencilOp::Replace,
    StencilOp::IncrementSaturate,
    StencilOp::DecrementSaturate,
    StencilOp::Invert,
    StencilOp::Increment,
    StencilOp::Decrement,
];
const BLEND_OPS: [BlendOp; 5] = [
    BlendOp::Add,
    BlendOp::Subtract,
    BlendOp::ReverseSubtract,
    BlendOp::Min,
    BlendOp::Max,
];

/// The payload offset of the state and one past its last render target record.
const STATE_AT: usize = 0x78;
const STATE_END: usize = 0x8c + 8 * 5;

impl SharedData {
    /// The fixed-function state ([`FixedState`]) of a raster permutation's shared data. Refused: a payload too short
    /// to hold it, a blend mode past the game's 17, an alpha blend op past its 5, a cull mode of 3, colour-op bits.
    pub fn fixed_state(&self) -> Result<FixedState> {
        const W: &str = "shared data state";
        // `rest` starts at payload offset 0x40
        let s = self.rest.get(STATE_AT - 0x40..STATE_END - 0x40).ok_or_else(|| {
            bad(
                W,
                STATE_AT,
                format!("{} bytes hold no state", self.rest.len() + 0x40),
            )
        })?;
        let b = |o: usize| s[o - STATE_AT];
        let face = |o: usize| StencilFace {
            fail: STENCIL_OPS[usize::from(b(o) & 7)],
            depth_fail: STENCIL_OPS[usize::from(b(o) >> 3 & 7)],
            pass: STENCIL_OPS[usize::from(b(o + 1) & 7)],
            compare: COMPARES[usize::from(b(o + 1) >> 3 & 7)],
        };
        let mode = |o: usize| -> Result<u8> {
            let v = b(o) & 0x1f;
            if v > 16 {
                return Err(bad(W, o, format!("blend mode {v} is past the game's 17")));
            }
            Ok(v)
        };
        let mut targets = [TargetRecord {
            blend: false,
            source: 0,
            destination: 0,
            source_alpha: 0,
            destination_alpha: 0,
            alpha_op: BlendOp::Add,
            write_mask: 0,
        }; 8];
        for (i, t) in targets.iter_mut().enumerate() {
            let o = 0x8c + 5 * i;
            if b(o + 1) >> 5 != 0 {
                return Err(bad(W, o + 1, "colour blend operation bits are set"));
            }
            let source = b(o) >> 1 & 0x1f;
            if source > 16 {
                return Err(bad(W, o, format!("blend mode {source} is past the game's 17")));
            }
            *t = TargetRecord {
                blend: b(o) & 1 != 0,
                source,
                destination: mode(o + 1)?,
                source_alpha: mode(o + 2)?,
                destination_alpha: mode(o + 3)?,
                alpha_op: *BLEND_OPS.get(usize::from(b(o + 3) >> 5)).ok_or_else(|| {
                    bad(
                        W,
                        o + 3,
                        format!("blend op {} is past the game's 5", b(o + 3) >> 5),
                    )
                })?,
                write_mask: b(o + 4),
            };
        }
        let (raster, depth, blend) = (b(0x7f), b(0x80), b(0x87));
        Ok(FixedState {
            slope_scaled_depth_bias: f32::from_le_bytes([b(0x78), b(0x79), b(0x7a), b(0x7b)]),
            depth_bias: i16::from_le_bytes([b(0x7c), b(0x7d)]),
            forced_sample_count: b(0x7e),
            fill: if raster & 1 == 0 {
                Fill::Solid
            } else {
                Fill::Wireframe
            },
            cull: match raster >> 1 & 3 {
                0 => Cull::None,
                1 => Cull::Front,
                2 => Cull::Back,
                _ => return Err(bad(W, 0x7f, "cull mode 3")),
            },
            front_counter_clockwise: raster >> 3 & 1 != 0,
            depth_clip: raster >> 4 & 1 != 0,
            multisample: raster >> 5 & 1 != 0,
            antialiased_line: raster >> 6 & 1 != 0,
            conservative: raster >> 7 != 0,
            depth_test: depth & 1 != 0,
            depth_write: depth >> 1 & 1 != 0,
            stencil_test: depth >> 2 & 1 != 0,
            depth_compare: COMPARES[usize::from(depth >> 3 & 7)],
            stencil_read_mask: b(0x81),
            stencil_write_mask: b(0x82),
            front: face(0x83),
            back: face(0x85),
            logic_op: blend & 0xf,
            logic_op_enable: blend >> 4 & 1 != 0,
            alpha_to_coverage: blend >> 5 & 1 != 0,
            independent_blend: blend >> 6 & 1 != 0,
            targets,
        })
    }
}
