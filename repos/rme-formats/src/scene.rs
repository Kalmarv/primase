//! Pure quartet decoder. All offsets in diagnostics are relative to the named decoded file.
//! Contracts: SHRED's stored bounds and tables, as its native loader reads them.
use crate::attributes::{self, Attributes};
use crate::pak::Pak;
use crate::skin::{SourceBone, SourceSkinVertex};
use crate::{Affine, Result, invalid, scp_tables, skin};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

/// A span of a quartet file the reader accounts for but does not decode, with why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unread {
    pub file: &'static str,
    pub offset: usize,
    pub bytes: usize,
    pub reason: &'static str,
}

#[derive(Clone, Debug)]
pub struct Submesh {
    pub indices: Range<usize>,
    pub material: u32,
    /// Logical ATTR8..11 -> stored UV0..3; 255 retains the same-index source.
    pub uv_remap: [u8; 4],
}

#[derive(Clone, Debug)]
pub struct Model {
    pub id: u32,
    pub bounds: [[f32; 3]; 2],
    pub positions: Vec<[f32; 3]>,
    pub format: u32,
    pub attributes: Attributes,
    pub processed: Option<Vec<SourceSkinVertex>>,
    pub processed_batches: usize,
    pub indices: Vec<u32>,
    pub submeshes: Vec<Submesh>,
    /// MLP offset and reason; no geometry is fabricated for an unsupported model.
    pub refusal: Option<(usize, &'static str)>,
}

#[derive(Clone, Debug)]
pub struct Placement {
    pub id: u32,
    pub transform: Affine,
    /// Offset of this 48-byte record in its SCP, all source words, and its name hash (+0x0c).
    pub offset: usize,
    pub words: [u32; 12],
    pub name: u32,
}

#[derive(Clone, Debug)]
pub struct Scene {
    pub models: Vec<Model>,
    pub placements: Vec<Placement>,
    /// The SCP's node table, world matrices and blob sections (spot chunks).
    pub tables: scp_tables::Tables,
    pub source_bones: Option<Vec<SourceBone>>,
    pub unread: Vec<Unread>,
}

#[derive(Clone, Copy)]
pub(crate) struct Bytes<'a> {
    pub(crate) data: &'a [u8],
    file: &'static str,
    base: usize,
}
impl<'a> Bytes<'a> {
    fn error(self, at: usize, reason: &str) -> crate::Error {
        invalid(self.base + at, format!("{}: {reason}", self.file))
    }
    pub(crate) fn slice(self, at: usize, n: usize) -> Result<&'a [u8]> {
        let end = at
            .checked_add(n)
            .ok_or_else(|| self.error(at, "range overflow"))?;
        self.data
            .get(at..end)
            .ok_or_else(|| self.error(at, "range outside file/resource"))
    }
    fn sub(self, at: usize, n: usize) -> Result<Self> {
        Ok(Self {
            data: self.slice(at, n)?,
            base: self.base + at,
            ..self
        })
    }
    pub(crate) fn u(self, at: usize) -> Result<usize> {
        Ok(u32::from_be_bytes(self.slice(at, 4)?.try_into().expect("four bytes")) as usize)
    }
    pub(crate) fn f(self, at: usize) -> Result<f32> {
        let n = f32::from_bits(self.u(at)? as u32);
        if !n.is_finite() {
            return Err(self.error(at, "nonfinite float"));
        }
        Ok(n)
    }
    pub(crate) fn v3(self, at: usize) -> Result<[f32; 3]> {
        Ok([self.f(at)?, self.f(at + 4)?, self.f(at + 8)?])
    }
    pub(crate) fn require(self, at: usize, yes: bool, reason: &str) -> Result<()> {
        if yes { Ok(()) } else { Err(self.error(at, reason)) }
    }
    pub(crate) fn table(self, at: usize, count: usize, stride: usize) -> Result<()> {
        self.slice(
            at,
            count
                .checked_mul(stride)
                .ok_or_else(|| self.error(at, "table size overflow"))?,
        )?;
        Ok(())
    }
    pub(crate) fn matrix(self, at: usize) -> Result<Affine> {
        self.slice(at, 64)?;
        let mut cols = [[0.; 3]; 4];
        for (j, col) in cols.iter_mut().enumerate() {
            *col = self.v3(at + j * 16)?;
            self.require(
                at + j * 16 + 12,
                self.f(at + j * 16 + 12)? == if j == 3 { 1. } else { 0. },
                "nonaffine matrix",
            )?;
        }
        Ok(Affine { cols })
    }
}

/// Validate disjoint coverage through EOF. Alignment holes must be zero; payload holes are errors.
fn coverage(b: Bytes<'_>, mut spans: Vec<Range<usize>>, padding: bool) -> Result<()> {
    spans.retain(|r| !r.is_empty());
    spans.sort_by_key(|r| r.start);
    let mut end = 0;
    for r in spans
        .into_iter()
        .chain(std::iter::once(b.data.len()..b.data.len()))
    {
        b.require(r.start, r.start >= end, "overlapping ranges")?;
        let gap = b.slice(end, r.start - end)?;
        if !gap.is_empty() {
            b.require(end, padding, "unconsumed payload bytes")?;
            if let Some(i) = gap.iter().position(|&x| x != 0) {
                return Err(b.error(end + i, "nonzero alignment/uncovered bytes"));
            }
        }
        b.slice(r.start, r.end - r.start)?;
        end = r.end;
    }
    Ok(())
}

/// Metadata has opaque saved-address/sentinel words beyond the decoded structures. Account for each gap.
fn unread_gaps(b: Bytes<'_>, mut spans: Vec<Range<usize>>) -> Result<Vec<Unread>> {
    spans.retain(|r| !r.is_empty());
    spans.sort_by_key(|r| r.start);
    let mut unread = Vec::new();
    let mut end = 0;
    for r in spans
        .into_iter()
        .chain(std::iter::once(b.data.len()..b.data.len()))
    {
        b.require(r.start, r.start >= end, "overlapping metadata structures")?;
        b.slice(r.start, r.len())?;
        if r.start > end {
            unread.push(Unread {
                file: b.file,
                offset: b.base + end,
                bytes: r.start - end,
                reason: "opaque mesh metadata outside decoded structures",
            });
        }
        end = r.end;
    }
    Ok(unread)
}

fn stream(b: Bytes<'_>, at: usize) -> Result<(usize, usize, Range<usize>)> {
    b.slice(at, 24)?;
    let count = b.u(at)?;
    let stride = b.u(at + 8)?;
    let off = b.u(at + 16)?;
    let size = b.u(at + 20)?;
    b.require(
        at + 20,
        count.checked_mul(stride) == Some(size),
        "stream size != count * stride",
    )?;
    Ok((count, stride, off..off + size))
}

/// Decode one quartet in source coordinates. Unsupported format variants are refused individually.
/// Unknown attributes/tables are returned with exact byte spans rather than assigned guessed meanings.
pub fn decode(scp: &[u8], mlp: &[u8], mmp: &[u8], mgp: &[u8]) -> Result<Scene> {
    let s = Bytes {
        data: scp,
        file: "scp",
        base: 0,
    };
    let l = Bytes {
        data: mlp,
        file: "mlp",
        base: 0,
    };
    let m = Bytes {
        data: mmp,
        file: "mmp",
        base: 0,
    };
    let g = Bytes {
        data: mgp,
        file: "mgp",
        base: 0,
    };
    s.require(0, s.u(0)? == 0xbabf003c, "bad magic/version")?;
    s.slice(0, 0x48)?;
    l.require(0, l.u(0)? == 0xfafa0003, "bad magic/version")?;
    l.require(0x18, l.u(0x18)? == 28, "unsupported resource record size")?;
    let count = l.u(0x10)?;
    let data = l.u(0x14)?;
    l.table(28, count, 28)?;
    l.require(
        20,
        data >= 28 + count * 28,
        "metadata base overlaps resource table",
    )?;
    l.slice(0, data)?;
    let mut spans = [std::iter::once(0..data).collect(), vec![], vec![]];
    let mut scene = Scene {
        models: vec![],
        placements: vec![],
        tables: scp_tables::Tables::default(),
        source_bones: None,
        unread: vec![],
    };
    let mut ids = BTreeSet::new();
    for i in 0..count {
        let at = 28 + i * 28;
        let id = l.u(at)? as u32;
        l.require(at, ids.insert(id), "duplicate model ID")?;
        let offsets = [data + l.u(at + 4)?, l.u(at + 8)?, l.u(at + 12)?];
        let sizes = [l.u(at + 16)?, l.u(at + 20)?, l.u(at + 24)?];
        for k in 0..3 {
            spans[k].push(offsets[k]..offsets[k] + sizes[k]);
        }
        let b = l.sub(offsets[0], sizes[0])?;
        let cpu = m.sub(offsets[1], sizes[1])?;
        let gpu = g.sub(offsets[2], sizes[2])?;
        b.slice(0, 0xc0)?;
        b.require(0, b.u(0)? == 0xbaba0031, "bad mesh magic/version")?;
        b.require(0x80, b.u(0x80)? as u32 == id, "resource ID mismatch")?;
        b.require(
            16,
            b.matrix(16)? == Affine::IDENTITY,
            "unsupported nonidentity mesh-local matrix",
        )?;
        let bounds = [b.v3(0x60)?, b.v3(0x70)?];
        b.require(
            0x60,
            (0..3).all(|k| bounds[0][k] <= bounds[1][k]),
            "inverted bounds",
        )?;
        let vp = b.u(0x90)?;
        let gp = b.u(0x94)?;
        let ip = b.u(0x98)?;
        let extra = b.u(0x9c)?;
        let sp = b.u(0xa0)?;
        let ns = b.u(0xa4)?;
        let (vc, stride, vr) = stream(b, vp)?;
        let embedded = extra != u32::MAX as usize;
        b.require(
            vp + 8,
            stride == if embedded { 36 } else { 16 },
            "unsupported primary stride",
        )?;
        if embedded {
            b.slice(extra, 4)?;
        }
        b.slice(ip, 24)?;
        let ic = b.u(ip)?;
        let io = b.u(ip + 8)?;
        let size = b.u(ip + 12)?;
        b.require(ip + 4, b.u(ip + 4)? == 5, "unsupported topology")?;
        b.require(ip + 16, b.u(ip + 16)? == 1, "unsupported index type")?;
        b.require(ip + 12, ic.checked_mul(2) == Some(size), "index size mismatch")?;
        b.require(ip, ic.is_multiple_of(3), "index count is not a triangle list")?;
        let ib = cpu.slice(io, size)?;
        let indices: Vec<u32> = ib
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| u16::from_be_bytes([v[0], v[1]]) as u32)
            .collect();
        for (j, &index) in indices.iter().enumerate() {
            cpu.require(io + j * 2, (index as usize) < vc, "index out of range")?;
        }
        b.table(sp, ns, 96)?;
        let mut subs = Vec::new();
        let mut partition = Vec::new();
        for j in 0..ns {
            let p = sp + j * 96;
            let first = b.u(p + 32)?;
            let n = b.u(p + 36)?;
            b.require(
                p + 32,
                first <= ic && n <= ic - first && first.is_multiple_of(3) && n.is_multiple_of(3),
                "invalid submesh index span",
            )?;
            b.require(p + 40, b.u(p + 40)? == 0, "unsupported base vertex")?;
            partition.push(first..first + n);
            subs.push(Submesh {
                indices: first..first + n,
                material: b.u(p + 52)? as u32,
                uv_remap: b.slice(p + 56, 4)?.try_into().expect("four remap bytes"),
            });
        }
        // A virtual byte per index allows the same exact-partition validation without allocating an index mask.
        partition.sort_by_key(|r| r.start);
        let mut end = 0;
        for r in partition {
            b.require(sp, r.start == end, "submesh partition gap or overlap")?;
            end = r.end;
        }
        b.require(sp, end == ic, "submesh partition does not reach index end")?;
        let mut positions = Vec::new();
        let format = b.u(0x8c)? as u32;
        let mut attributes = Attributes::default();
        let mut processed = None;
        let mut processed_batches = 0;
        let unsupported_uv = subs.iter().enumerate().find_map(|(i, sub)| {
            sub.uv_remap
                .iter()
                .position(|&v| v > 3 && v != 255)
                .map(|slot| b.base + sp + i * 96 + 56 + slot)
        });
        let refusal = if embedded {
            b.require(
                0x94,
                gp == u32::MAX as usize && gpu.data.is_empty(),
                "unsupported embedded MGP stream",
            )?;
            coverage(cpu, std::iter::once(io..io + size).collect(), false)?;
            if format != 0x8007f || b.u(0xb4)? != 1 {
                Some((b.base + 0x8c, "unsupported processed skin format/flags"))
            } else if let Some(offset) = unsupported_uv {
                Some((offset, "unsupported UV source selector"))
            } else {
                if scene.source_bones.is_none() {
                    scene.source_bones = Some(skin::skeleton(s)?);
                }
                let bones = scene.source_bones.as_ref().expect("source skeleton decoded");
                let decoded = skin::decode(b, extra, vc, bones.len(), bounds)?;
                scene.unread.extend(unread_gaps(
                    b,
                    vec![
                        0..0xc0,
                        sp..sp + ns * 96,
                        vp..vp + 24,
                        ip..ip + 24,
                        decoded.stream,
                    ],
                )?);
                positions = decoded.positions;
                attributes = decoded.attributes;
                processed = Some(decoded.vertices);
                processed_batches = decoded.batches;
                None
            }
        } else {
            cpu.slice(vr.start, vr.len())?;
            for j in 0..vc {
                let p = cpu.v3(vr.start + j * 16)?;
                cpu.require(
                    vr.start + j * 16,
                    (0..3).all(|k| p[k] >= bounds[0][k] - 0.05 && p[k] <= bounds[1][k] + 0.05),
                    "position outside stored AABB (0.05 source-unit tolerance)",
                )?;
                positions.push(p);
            }
            coverage(cpu, vec![vr.clone(), io..io + size], false)?;
            let (gs, gr) = if gp == u32::MAX as usize {
                b.require(0x94, gpu.data.is_empty(), "missing MGP descriptor with payload")?;
                (0, 0..0)
            } else {
                let (gc, gs, gr) = stream(b, gp)?;
                b.require(gp, gc == vc, "MGP vertex count mismatch")?;
                coverage(gpu, vec![gr.clone()], false)?;
                (gs, gr)
            };
            let mut meta = vec![0..0xc0, sp..sp + ns * 96, vp..vp + 24, ip..ip + 24];
            if gp != u32::MAX as usize {
                meta.push(gp..gp + 24);
            }
            scene.unread.extend(unread_gaps(b, meta)?);
            if attributes::stride(format).is_none() {
                Some((b.base + 0x8c, "unsupported rigid vertex format mask"))
            } else if let Some(offset) = unsupported_uv {
                Some((offset, "unsupported UV source selector"))
            } else {
                b.require(
                    0x94,
                    attributes::stride(format) == Some(gs),
                    "MGP stride differs from vertex format mask",
                )?;
                attributes = attributes::decode(
                    format,
                    vc,
                    cpu.sub(vr.start, vr.len())?,
                    gpu.sub(gr.start, gr.len())?,
                    gs,
                )?;
                None
            }
        };
        scene.models.push(Model {
            id,
            bounds,
            positions,
            format,
            attributes,
            processed,
            processed_batches,
            indices,
            submeshes: subs,
            refusal,
        });
    }
    coverage(l, std::mem::take(&mut spans[0]), true)?;
    coverage(m, std::mem::take(&mut spans[1]), true)?;
    coverage(g, std::mem::take(&mut spans[2]), true)?;
    let n = s.u(16)?;
    let off = s.u(20)?;
    s.table(off, n, 48)?;
    // The node and world-matrix tables and the blob chain are read by `scp_tables`.
    let (tables, tables_unread) = scp_tables::decode(scp)?;
    scene.tables = tables;
    scene.unread.extend(tables_unread);
    // Other native-loader tables remain opaque. Processed skins already retain the named bone table.
    for (field, stride) in [(0x20, 16), (0x28, 56), (0x30, 512)] {
        let count = s.u(field)?;
        let start = s.u(field + 4)?;
        if count > 0 && !(field == 0x20 && scene.source_bones.is_some()) {
            s.table(start, count, stride)?;
            scene.unread.push(Unread {
                file: "scp",
                offset: start,
                bytes: count * stride,
                reason: "unknown scene table",
            });
        }
    }
    // Remaining SCP records, pointers and trailing blob are deliberately accounted for as unread.
    scene.unread.push(Unread {
        file: "scp",
        offset: 0,
        bytes: scp.len(),
        reason: "opaque header/placement fields, nested tables and blob; overlaps known fields",
    });
    for i in 0..n {
        let at = off + i * 48;
        let id = s.u(at + 8)? as u32;
        s.require(at + 8, ids.contains(&id), "placement references missing mesh")?;
        scene.placements.push(Placement {
            id,
            transform: s.matrix(s.u(at)?)?,
            offset: at,
            words: (0..12)
                .map(|j| s.u(at + j * 4).map(|v| v as u32))
                .collect::<Result<Vec<_>>>()?
                .try_into()
                .map_err(|_| invalid(at, "placement word count"))?,
            name: s.u(at + 12)? as u32,
        });
    }
    Ok(scene)
}

/// The quartets of `pak`: entries sharing a stem (the raw archive name before the extension) with extensions `.scp`,
/// `.mlp`, `.mmp` and `.mgp`, as indices into [`Pak::entries`] in that order. Stems are raw archive names, not host
/// filesystem paths. A repeated extension or an incomplete quartet is refused.
pub fn quartets(pak: &Pak) -> Result<BTreeMap<Vec<u8>, [usize; 4]>> {
    let mut groups: BTreeMap<Vec<u8>, [Option<usize>; 4]> = BTreeMap::new();
    for (i, e) in pak.entries().iter().enumerate() {
        if e.attributes == 1 {
            continue;
        }
        let Some(dot) = e.name.iter().rposition(|&c| c == b'.') else {
            continue;
        };
        let slot = match &e.name[dot + 1..] {
            b"scp" => 0,
            b"mlp" => 1,
            b"mmp" => 2,
            b"mgp" => 3,
            _ => continue,
        };
        let group = groups.entry(e.name[..dot].to_vec()).or_insert([None; 4]);
        if group[slot].replace(i).is_some() {
            return Err(invalid(e.offset, "duplicate quartet extension"));
        }
    }
    groups
        .into_iter()
        .map(|(name, g)| {
            if let [Some(a), Some(b), Some(c), Some(d)] = g {
                Ok((name, [a, b, c, d]))
            } else {
                Err(invalid(
                    0,
                    format!("incomplete quartet {:?}", String::from_utf8_lossy(&name)),
                ))
            }
        })
        .collect()
}

/// Decode the quartet whose `.scp`, `.mlp`, `.mmp` and `.mgp` entries are `entries` (from [`quartets`]).
pub fn decode_quartet(pak: &Pak, entries: [usize; 4]) -> Result<Scene> {
    let [a, b, c, d] = entries;
    decode(
        &pak.payload(a)?,
        &pak.payload(b)?,
        &pak.payload(c)?,
        &pak.payload(d)?,
    )
}
