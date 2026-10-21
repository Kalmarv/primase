//! Neutral body-shape targets for a costume.
//!
//! Studio gives an imported costume the six body-shape targets of the CAS body (in two chunks: LODs 0-1 and 2-5),
//! transferred from the donor: each target holds every vertex's absolute position and tangent frame under that shape,
//! so a player's body sliders move the costume. Transferred onto a mesh the donor's shapes were not made for, they
//! crease it (the Sekiro costumes, confirmed in game 2026-10-10); [`neutralise`] makes all six targets the mesh's own
//! positions and tangent frames, so the costume keeps its shape under every slider. Nothing else in the chunks
//! changes.
//!
//! A target chunk (first word 7) holds at the offset and size in words 5 and 6 a block: `01 00 06 08` (six targets),
//! the section count at byte 5, the section table's offset (48) and the declaration's at words 2 and 3, the
//! declaration `01 03 00 00 34 0d 00 01` (position as three `f32`, tangent frame (usage 52) as four bytes); a section
//! record of five words: its identity (bits 8-15 the LOD, 16-31 the section), vertex count, start and end within the
//! block, and its vertex count again; a section's span is the six targets' positions, then their tangent frames.
//! Refused, naming what: any other layout, a section the mesh does not have or whose vertex count differs, a span
//! outside the block.

use primase_compress::oodle::Oodle;
use primase_frostbite::fbmod::{Body, Fbmod};

use super::textures::guid_text;
use super::{Error, Result};
use crate::postcompile::meshset::{self, MeshSet, Section};

const TARGETS: usize = 6;
const MAGIC: [u8; 4] = [1, 0, 6, 8];
const DECL: [u8; 8] = [0x01, 0x03, 0x00, 0x00, 0x34, 0x0d, 0x00, 0x01];
const RECORD: usize = 20;
const POSITION: u8 = 1;
const TANGENT_FRAME: u8 = 52;

/// One section's targets in a chunk: its LOD and section, vertex count, and the span (in the chunk) of its six
/// targets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Targets {
    pub lod: usize,
    pub section: usize,
    pub vertices: usize,
    pub span: std::ops::Range<usize>,
}

fn word(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
}

fn bad(what: String) -> Error {
    Error::Missing(format!("shape targets: {what}"))
}

/// The sections of a target chunk (see the module documentation); `None` when the chunk is not one (its first word
/// is not 7).
pub fn targets(chunk: &[u8]) -> Result<Option<Vec<Targets>>> {
    if word(chunk, 0) != Some(7) {
        return Ok(None);
    }
    let w = |at: usize| {
        word(chunk, at)
            .map(|v| v as usize)
            .ok_or_else(|| bad(format!("ends before byte {at}")))
    };
    let (off, size) = (w(20)?, w(24)?);
    let block = chunk
        .get(off..off.saturating_add(size))
        .ok_or_else(|| bad(format!("block {off}+{size} outside {} bytes", chunk.len())))?;
    let b = |at: usize| {
        word(block, at)
            .map(|v| v as usize)
            .ok_or_else(|| bad(format!("block ends before {at}")))
    };
    if block.get(..4) != Some(&MAGIC[..]) {
        return Err(bad(format!(
            "block starts {:02x?}, not {MAGIC:02x?}",
            block.get(..4)
        )));
    }
    let n = usize::from(block[5]);
    let (table, decl) = (b(8)?, b(12)?);
    if table != 48 || decl != table + n * RECORD {
        return Err(bad(format!(
            "section table at {table}, declaration at {decl}, {n} sections"
        )));
    }
    if block.get(decl..decl + 8) != Some(&DECL[..]) {
        return Err(bad(format!("declaration {:02x?}", block.get(decl..decl + 8))));
    }
    (0..n)
        .map(|i| {
            let r = table + i * RECORD;
            let (identity, count, start, end, mapped) = (b(r)?, b(r + 4)?, b(r + 8)?, b(r + 12)?, b(r + 16)?);
            if mapped != count || end.checked_sub(start) != Some(count * TARGETS * 16) || end > size {
                return Err(bad(format!(
                    "section record {i}: {count} vertices, span {start}..{end}, {mapped} mapped, block {size}"
                )));
            }
            Ok(Targets {
                lod: (identity >> 8) & 0xff,
                section: identity >> 16,
                vertices: count,
                span: off + start..off + end,
            })
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

/// The bytes of the stored stream of `sec` that holds the element of `usage` (alone in its stream), in the LOD chunk
/// `lod`; `size` the element's bytes.
fn stream<'a>(lod: &'a [u8], sec: &Section, usage: u8, size: u8) -> Result<&'a [u8]> {
    let e = sec
        .decl
        .elements
        .iter()
        .find(|e| e.usage == usage)
        .ok_or_else(|| bad(format!("a section without usage {usage}")))?;
    let s = usize::from(e.stream);
    if e.offset != 0 || sec.decl.strides.get(s) != Some(&size) {
        return Err(bad(format!(
            "usage {usage} is not alone in its stream of {size} bytes"
        )));
    }
    let n = sec.vertices as usize;
    let before: usize = sec.decl.strides[..s].iter().map(|&x| usize::from(x)).sum();
    let at = sec.vertex_offset as usize + before * n;
    lod.get(at..at + usize::from(size) * n)
        .ok_or_else(|| bad(format!("usage {usage} outside the LOD chunk")))
}

/// What [`neutralise`] changed: per target chunk, its name and the sections made neutral.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Neutralised {
    pub chunks: Vec<(String, usize)>,
}

/// Make every body-shape target of the costume its mesh's own positions and tangent frames (see the module
/// documentation). The mesh is the mod's one MeshSet resource; its LODs' chunks are the mod's chunk entries named by
/// their GUIDs.
pub fn neutralise(m: &mut Fbmod, oodle: Option<&Oodle>) -> Result<Neutralised> {
    let mut meshes = m.entries.iter().filter_map(|e| match (&e.body, e.index()) {
        (Body::Res(r), Some(i)) if r.res_type == primase_frostbite::MESHSET => Some(i),
        _ => None,
    });
    let (Some(mesh), None) = (meshes.next(), meshes.next()) else {
        return Err(bad("the mod does not hold exactly one MeshSet".into()));
    };
    let set: MeshSet = meshset::read(&m.payload(mesh, oodle)?)?;
    let chunk_of = |m: &Fbmod, name: &str| -> Result<Option<(usize, Vec<u8>)>> {
        let Some((i, e)) = m
            .entries
            .iter()
            .enumerate()
            .find(|(_, e)| matches!(e.body, Body::Chunk(_)) && e.name == name)
        else {
            return Ok(None);
        };
        let index = e
            .index()
            .ok_or_else(|| bad(format!("chunk {name} has no payload")))?;
        Ok(Some((i, m.payload(index, oodle)?)))
    };
    let lods = set
        .lods
        .iter()
        .map(|l| {
            let name = guid_text(&l.chunk);
            chunk_of(m, &name)?
                .map(|(_, b)| b)
                .ok_or_else(|| bad(format!("the LOD chunk {name} is not in the mod")))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut report = Neutralised::default();
    let chunk_entries: Vec<usize> = (0..m.entries.len())
        .filter(|&i| matches!(m.entries[i].body, Body::Chunk(_)))
        .collect();
    for i in chunk_entries {
        let name = m.entries[i].name.clone();
        let Some((_, mut data)) = chunk_of(m, &name)? else {
            continue;
        };
        let Some(sections) = targets(&data)? else {
            continue;
        };
        for t in &sections {
            let sec = set
                .lods
                .get(t.lod)
                .and_then(|l| l.sections.get(t.section))
                .ok_or_else(|| {
                    bad(format!(
                        "{name}: LOD {} section {} is not the mesh's",
                        t.lod, t.section
                    ))
                })?;
            if sec.vertices as usize != t.vertices {
                return Err(bad(format!(
                    "{name}: LOD {} section {} has {} vertices, its targets {}",
                    t.lod, t.section, sec.vertices, t.vertices
                )));
            }
            let positions = stream(&lods[t.lod], sec, POSITION, 12)?;
            let frames = stream(&lods[t.lod], sec, TANGENT_FRAME, 4)?;
            let mut neutral = Vec::with_capacity(t.span.len());
            for _ in 0..TARGETS {
                neutral.extend_from_slice(positions);
            }
            for _ in 0..TARGETS {
                neutral.extend_from_slice(frames);
            }
            data[t.span.clone()].copy_from_slice(&neutral);
        }
        let index = m.add_payload(&data)?;
        if let Some(s) = m.entries[i].stored.as_mut() {
            s.index = index;
        }
        report.chunks.push((name, sections.len()));
    }
    if report.chunks.is_empty() {
        return Err(bad("the mod has no target chunk".into()));
    }
    m.drop_unused_payloads();
    Ok(report)
}
