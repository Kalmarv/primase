//! The techniques and fixed-function state of an effect of the Enhanced shader database: what follows the six program
//! lists in an effect record ([`crate::rage::sgd2::Effect::rest`]), the part of it that names the passes the game draws
//! with and the programs of each. The record's other tables (the parameter table of
//! [`crate::rage::sgd2_table::effect_parameters`], the root signature table and the string tables) are not read here.
//!
//! ```text
//! (a header of tables kept as stored)
//! u32 nDS, nDS depth-stencil records of 24 bytes:
//!     the 14 bytes of a D3D12_DEPTH_STENCIL_DESC with one-byte fields (DepthEnable, DepthWriteMask, DepthFunc,
//!     StencilEnable, StencilReadMask, StencilWriteMask, then front and back face: StencilFailOp,
//!     StencilDepthFailOp, StencilPassOp, StencilFunc), then 10 bytes kept as stored
//! u32 nBlend, nBlend blend records of 68 bytes:
//!     4 bytes (kept), then eight render targets of 8 bytes: BlendEnable, SrcBlend, DestBlend, BlendOp,
//!     SrcBlendAlpha, DestBlendAlpha, BlendOpAlpha, RenderTargetWriteMask
//! u32 nSamplers, nSamplers sampler records of 36 bytes:
//!     Filter, AddressU, AddressV, AddressW, MipLODBias f32, MaxAnisotropy, ComparisonFunc, 22 bytes (the border
//!     colour, the minimum LOD and two more bytes, kept), MaxLOD f32
//! u32 nTechniques, nTechniques entries of 4 bytes: u16 offset of the technique's name in the name block (bit 15
//!     set on every second entry; ignored), u8 (kept), u8 the technique's number of passes
//! u32 nPasses (the sum), nPasses pass records of 24 bytes, the passes of the first technique first:
//!     u8 vertex program, u8 pixel program, u8 geometry program, u8 hull and domain program (each the 1-based position
//!     in the effect's list of that stage, 0 for none), 20 bytes kept as stored
//! (the root signature table, `proplst`, and the name block: u32 length, the NUL-terminated names to the end)
//! ```
//!
//! The enumerations are Direct3D 12's (blend factors ZERO = 1, ONE = 2, ..., INV_SRC_ALPHA = 6; comparison functions
//! NEVER = 1, LESS = 2, ..., GREATER = 5, ALWAYS = 8; address modes WRAP = 1, MIRROR = 2, CLAMP = 3), which the records'
//! values fit on every record of the shader database (`tests/effect_techniques.rs`). Which of the depth-stencil and
//! blend records a pass draws with is not among the fields named: the pass record's remaining 20 bytes are kept as
//! stored.
//!
//! The tables are found from the technique table, the one place in the record that says what it is: the position
//! where a count of 1 to 512 is followed by that many entries whose name offsets are the starts of names in the name
//! block and whose pass counts sum to the count that follows them. The state tables before it are found backwards, each
//! by its own count and its records' fields being in range. An effect with no such place (the post-processing and
//! lighting effects) has no technique table here, and one with two is refused. No I/O.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use serde::Serialize;

use crate::{Result, invalid};

const FORMAT: &str = "sgd2 effect techniques";
/// The marker of the root signature table, which the name block follows.
const ROOT_SIGNATURES: &[u8] = b"proplst\0";
const DEPTH_STENCIL: usize = 24;
const BLEND: usize = 68;
const SAMPLER: usize = 36;
const PASS: usize = 24;

/// One face of a depth-stencil state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct StencilFace {
    pub fail_op: u8,
    pub depth_fail_op: u8,
    pub pass_op: u8,
    pub func: u8,
}

/// A depth-stencil state (D3D12 values).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct DepthStencil {
    pub depth_enable: u8,
    /// 0 none, 1 all.
    pub depth_write_mask: u8,
    pub depth_func: u8,
    pub stencil_enable: u8,
    pub stencil_read_mask: u8,
    pub stencil_write_mask: u8,
    pub front: StencilFace,
    pub back: StencilFace,
    /// The record's last 10 bytes, as stored.
    pub tail: [u8; 10],
}

/// One render target of a blend state (D3D12 values).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct TargetBlend {
    pub enable: u8,
    pub src: u8,
    pub dest: u8,
    pub op: u8,
    pub src_alpha: u8,
    pub dest_alpha: u8,
    pub op_alpha: u8,
    pub write_mask: u8,
}

/// A blend state: eight render targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Blend {
    /// The record's first 4 bytes, as stored.
    pub head: [u8; 4],
    pub targets: [TargetBlend; 8],
}

/// A sampler state (D3D12 values).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SamplerDesc {
    pub filter: u8,
    pub address: [u8; 3],
    pub mip_lod_bias: f32,
    pub max_anisotropy: u8,
    pub comparison_func: u8,
    pub max_lod: f32,
    /// The record, as stored.
    pub raw: Vec<u8>,
}

/// One pass: the programs it runs, each the 1-based position in the effect's list of that stage (0 for none).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Pass {
    pub vertex: u8,
    pub pixel: u8,
    pub geometry: u8,
    /// The hull program and the domain program of a tessellated pass (the same position in both lists).
    pub tessellation: u8,
    /// The record, as stored.
    pub raw: Vec<u8>,
}

/// A technique: a name and its passes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Technique {
    pub name: String,
    pub passes: Vec<Pass>,
}

/// An effect's techniques and the state tables before them.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EffectTechniques {
    pub depth_stencil: Vec<DepthStencil>,
    pub blend: Vec<Blend>,
    pub samplers: Vec<SamplerDesc>,
    pub techniques: Vec<Technique>,
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at.checked_add(4)?)?
        .try_into()
        .ok()
        .map(u32::from_le_bytes)
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at.checked_add(2)?)?
        .try_into()
        .ok()
        .map(u16::from_le_bytes)
}

fn f32_at(b: &[u8], at: usize) -> Option<f32> {
    u32_at(b, at).map(f32::from_bits)
}

fn array<const N: usize>(b: &[u8], at: usize) -> Option<[u8; N]> {
    b.get(at..at.checked_add(N)?)?.try_into().ok()
}

fn byte(b: &[u8], at: usize) -> u8 {
    b.get(at).copied().unwrap_or(0)
}

/// The name block: the bytes after the root signature table (`proplst`, a count, and that many size-prefixed
/// entries), whose length prefix reaches the end of the record.
fn name_block(rest: &[u8]) -> Option<&[u8]> {
    let marker = rest
        .windows(ROOT_SIGNATURES.len())
        .rposition(|w| w == ROOT_SIGNATURES)?;
    let mut at = marker.checked_add(ROOT_SIGNATURES.len())?;
    let entries = u32_at(rest, at)?;
    at = at.checked_add(4)?;
    for _ in 0..entries {
        let size = usize::try_from(u32_at(rest, at)?).ok()?;
        at = at.checked_add(4)?.checked_add(size)?;
    }
    let length = usize::try_from(u32_at(rest, at)?).ok()?;
    let start = at.checked_add(4)?;
    (start.checked_add(length)? == rest.len())
        .then(|| rest.get(start..))
        .flatten()
}

/// The name that starts at `offset` in the block, if one does.
fn name_at(block: &[u8], offset: usize) -> Option<&str> {
    if offset > 0 && *block.get(offset.checked_sub(1)?)? != 0 {
        return None;
    }
    let tail = block.get(offset..)?;
    let end = tail.iter().position(|&b| b == 0)?;
    std::str::from_utf8(tail.get(..end)?).ok()
}

/// The technique table at `at`: its entries (name offset, passes) and where its pass records start.
fn technique_table(rest: &[u8], block: &[u8], at: usize) -> Option<(Vec<(usize, usize)>, usize)> {
    let count = usize::try_from(u32_at(rest, at)?)
        .ok()
        .filter(|c| (1..=512).contains(c))?;
    let mut entries = Vec::with_capacity(count);
    let mut total = 0usize;
    for i in 0..count {
        let entry = at.checked_add(4)?.checked_add(i.checked_mul(4)?)?;
        let offset = usize::from(u16_at(rest, entry)? & 0x7fff);
        let passes = usize::from(*rest.get(entry.checked_add(3)?)?);
        if !(1..=16).contains(&passes) || name_at(block, offset).is_none() {
            return None;
        }
        total = total.checked_add(passes)?;
        entries.push((offset, passes));
    }
    let after = at.checked_add(4)?.checked_add(count.checked_mul(4)?)?;
    if usize::try_from(u32_at(rest, after)?).ok()? != total {
        return None;
    }
    let records = after.checked_add(4)?;
    (records.checked_add(total.checked_mul(PASS)?)? <= rest.len()).then_some((entries, records))
}

fn depth_stencil(r: &[u8]) -> Option<DepthStencil> {
    let b = |i| byte(r, i);
    let face = |at| StencilFace {
        fail_op: b(at),
        depth_fail_op: b(at.saturating_add(1)),
        pass_op: b(at.saturating_add(2)),
        func: b(at.saturating_add(3)),
    };
    let s = DepthStencil {
        depth_enable: b(0),
        depth_write_mask: b(1),
        depth_func: b(2),
        stencil_enable: b(3),
        stencil_read_mask: b(4),
        stencil_write_mask: b(5),
        front: face(6),
        back: face(10),
        tail: array(r, 14)?,
    };
    let ops = [s.front, s.back].iter().all(|f| {
        [f.fail_op, f.depth_fail_op, f.pass_op]
            .iter()
            .all(|o| (1..=8).contains(o))
            && (1..=8).contains(&f.func)
    });
    (s.depth_enable <= 1
        && s.depth_write_mask <= 1
        && (1..=8).contains(&s.depth_func)
        && s.stencil_enable <= 1
        && ops)
        .then_some(s)
}

fn blend(r: &[u8]) -> Option<Blend> {
    let mut targets = [TargetBlend {
        enable: 0,
        src: 0,
        dest: 0,
        op: 0,
        src_alpha: 0,
        dest_alpha: 0,
        op_alpha: 0,
        write_mask: 0,
    }; 8];
    for (i, t) in targets.iter_mut().enumerate() {
        let at = 4usize.saturating_add(i.saturating_mul(8));
        let b = |k: usize| byte(r, at.saturating_add(k));
        *t = TargetBlend {
            enable: b(0),
            src: b(1),
            dest: b(2),
            op: b(3),
            src_alpha: b(4),
            dest_alpha: b(5),
            op_alpha: b(6),
            write_mask: b(7),
        };
        let factor = |f: u8| (1..=20).contains(&f);
        let op = |o: u8| (1..=5).contains(&o);
        if t.enable > 1
            || !factor(t.src)
            || !factor(t.dest)
            || !op(t.op)
            || !factor(t.src_alpha)
            || !factor(t.dest_alpha)
            || !op(t.op_alpha)
        {
            return None;
        }
    }
    Some(Blend {
        head: array(r, 0)?,
        targets,
    })
}

fn sampler(r: &[u8]) -> Option<SamplerDesc> {
    Some(SamplerDesc {
        filter: byte(r, 0),
        address: array(r, 1)?,
        mip_lod_bias: f32_at(r, 4)?,
        max_anisotropy: byte(r, 8),
        comparison_func: byte(r, 9),
        max_lod: f32_at(r, 32)?,
        raw: r.get(..SAMPLER)?.to_vec(),
    })
}

/// A table of `count` records of `size` bytes whose count sits just before it and which ends at `end`: the records,
/// each accepted by `read`, and where the count sits.
fn table_before<T>(
    rest: &[u8],
    end: usize,
    size: usize,
    max: usize,
    read: impl Fn(&[u8]) -> Option<T>,
) -> Vec<(usize, Vec<T>)> {
    let mut found = Vec::new();
    for count in 0..=max {
        let Some(start) = count
            .checked_mul(size)
            .and_then(|n| n.checked_add(4))
            .and_then(|n| end.checked_sub(n))
        else {
            break;
        };
        if u32_at(rest, start).and_then(|c| usize::try_from(c).ok()) != Some(count) {
            continue;
        }
        let records: Option<Vec<T>> = (0..count)
            .map(|i| {
                let at = start.checked_add(4)?.checked_add(i.checked_mul(size)?)?;
                read(rest.get(at..at.checked_add(size)?)?)
            })
            .collect();
        if let Some(records) = records {
            found.push((start, records));
        }
    }
    found
}

/// Read an effect record's techniques: `Ok(None)` for an effect with no technique table.
pub fn parse(rest: &[u8]) -> Result<Option<EffectTechniques>> {
    let Some(block) = name_block(rest) else {
        return Ok(None);
    };
    let mut tables = (0..rest.len()).filter_map(|at| technique_table(rest, block, at).map(|t| (at, t)));
    let Some((at, (entries, records))) = tables.next() else {
        return Ok(None);
    };
    if let Some((other, _)) = tables.next() {
        return Err(invalid(
            FORMAT,
            at as u64,
            format!("two technique tables, at {at} and {other}"),
        ));
    }
    let mut techniques = Vec::with_capacity(entries.len());
    let mut pass_at = records;
    for (offset, passes) in entries {
        let name = name_at(block, offset).unwrap_or_default().to_owned();
        let mut list = Vec::with_capacity(passes);
        for _ in 0..passes {
            let raw: [u8; PASS] =
                array(rest, pass_at).ok_or_else(|| invalid(FORMAT, pass_at as u64, "a short pass record"))?;
            list.push(Pass {
                vertex: byte(&raw, 0),
                pixel: byte(&raw, 1),
                geometry: byte(&raw, 2),
                tessellation: byte(&raw, 3),
                raw: raw.to_vec(),
            });
            pass_at = pass_at.saturating_add(PASS);
        }
        techniques.push(Technique { name, passes: list });
    }
    // the state tables, backwards from the technique table: samplers, blend, depth-stencil
    let mut chain = Vec::new();
    for (s_at, samplers) in table_before(rest, at, SAMPLER, 64, sampler) {
        for (b_at, blends) in table_before(rest, s_at, BLEND, 16, blend) {
            for (d_at, depth) in table_before(rest, b_at, DEPTH_STENCIL, 16, depth_stencil) {
                chain.push((depth, blends.clone(), samplers.clone(), d_at));
            }
        }
    }
    let mut chain = chain.into_iter();
    let Some((depth_stencil, blend, samplers, _)) = chain.next() else {
        return Err(invalid(
            FORMAT,
            at as u64,
            "no state tables before the technique table",
        ));
    };
    if chain.next().is_some() {
        return Err(invalid(
            FORMAT,
            at as u64,
            "the state tables before the technique table are ambiguous",
        ));
    }
    Ok(Some(EffectTechniques {
        depth_stencil,
        blend,
        samplers,
        techniques,
    }))
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    reason = "hand-built fixtures"
)]
mod tests {
    use super::*;

    fn depth(write: u8, func: u8) -> Vec<u8> {
        let mut r = vec![1, write, func, 0, 0xff, 0xff, 1, 1, 1, 8, 1, 1, 1, 8];
        r.extend([0; 8]);
        r.extend([1, 1]);
        r
    }

    fn blend_record(enable: u8) -> Vec<u8> {
        let mut r = vec![0, 1, 0, 0];
        for _ in 0..8 {
            r.extend([enable, 5, 6, 1, 2, 1, 1, 0xf]);
        }
        r
    }

    fn sampler_record(filter: u8, address: u8) -> Vec<u8> {
        let mut r = vec![filter, address, address, address, 0, 0, 0, 0, 1, 1];
        r.extend([0; 22]);
        r.extend(12f32.to_le_bytes());
        r
    }

    /// A record of two techniques, `draw` (one pass) and `skin` (two), after a header, one depth-stencil state, one
    /// blend state and two samplers; the name block follows the root signature marker.
    fn record() -> Vec<u8> {
        let mut r = vec![0u8; 12];
        r.extend(1u32.to_le_bytes());
        r.extend(depth(1, 5));
        r.extend(1u32.to_le_bytes());
        r.extend(blend_record(1));
        r.extend(2u32.to_le_bytes());
        r.extend(sampler_record(0x55, 3));
        r.extend(sampler_record(0, 1));
        // names: "\0s0\0draw\0skin\0p0\0"
        let names: &[u8] = b"\0s0\0draw\0skin\0p0\0";
        r.extend(2u32.to_le_bytes());
        r.extend([4u8, 0, 0, 1]); // draw at 4, 1 pass
        r.extend([9u8, 0x80, 1, 2]); // skin at 9, flag bit set, 2 passes
        r.extend(3u32.to_le_bytes());
        for (v, p, t) in [(1u8, 1u8, 0u8), (2, 1, 0), (2, 2, 1)] {
            let mut pass = vec![v, p, 0, t];
            pass.extend([0; 20]);
            r.extend(pass);
        }
        r.extend(b"proplst\0");
        r.extend(0u32.to_le_bytes());
        r.extend((names.len() as u32).to_le_bytes());
        r.extend(names);
        r
    }

    #[test]
    fn a_record_worked_by_hand_reads_field_by_field() {
        let t = parse(&record()).unwrap().unwrap();
        assert_eq!(
            (t.depth_stencil.len(), t.blend.len(), t.samplers.len()),
            (1, 1, 2)
        );
        assert_eq!(
            (t.depth_stencil[0].depth_func, t.depth_stencil[0].depth_write_mask),
            (5, 1)
        );
        assert_eq!(t.blend[0].targets[3].src, 5);
        assert_eq!(t.blend[0].targets[7].write_mask, 0xf);
        assert_eq!(
            (t.samplers[0].filter, t.samplers[0].address, t.samplers[0].max_lod),
            (0x55, [3; 3], 12.0)
        );
        assert_eq!(t.techniques.len(), 2);
        assert_eq!(
            (t.techniques[0].name.as_str(), t.techniques[0].passes.len()),
            ("draw", 1)
        );
        assert_eq!(
            (t.techniques[1].name.as_str(), t.techniques[1].passes.len()),
            ("skin", 2)
        );
        let p = &t.techniques[1].passes[1];
        assert_eq!((p.vertex, p.pixel, p.tessellation), (2, 2, 1));
    }

    #[test]
    fn a_record_without_a_technique_table_has_none() {
        assert_eq!(parse(&[0; 64]).unwrap(), None);
        // a name block, no table
        let mut r = vec![0u8; 16];
        r.extend(b"proplst\0");
        r.extend(0u32.to_le_bytes());
        r.extend(4u32.to_le_bytes());
        r.extend(b"\0ab\0");
        assert_eq!(parse(&r).unwrap(), None);
    }

    #[test]
    fn a_pass_count_that_does_not_add_up_is_no_table() {
        let mut r = record();
        // the total after the entries: 3 -> 4
        let at = r.windows(4).rposition(|w| w == 3u32.to_le_bytes()).unwrap();
        r[at] = 4;
        assert_eq!(parse(&r).unwrap(), None);
    }
}
