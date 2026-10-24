//! ReSkate Studio's `.fbmod`: the Frosty mod layout Studio labels version 6 (the legacy layout with a length-prefixed
//! profile and no resource-table SHA-1; not the other Frosty v6 layout). Header, entry table, payload table, payloads:
//!
//! ```text
//! "FROSTY\0\1"  u32 version (6)  u64 payload-table offset  u32 payload count
//! u8 length + profile  u32 game version  six zero-terminated strings (title, author, category, version,
//! description, link)  u32 entry count  entries...  payload table: (u64 offset, u64 size) per payload  payloads
//! ```
//!
//! An entry is `u8 kind, i32 payload index (-1: none), name`; with a payload, `SHA-1 (20), u64 decoded size, u8 flags,
//! u32 handler, user string`; then `u32 count + u32 bundle hashes`; then by kind: a resource (2) `u32 type, u64 id,
//! u32 length + metadata`; a chunk (3) six `u32` and `u32 count + u32 added-bundle hashes`. Kind 0 (embedded files:
//! the icon and screenshots) and 1 (EBX) have nothing more. Payloads are CAS-framed, shared by index; the SHA-1 is of
//! the stored (encoded) payload. Layout after FrostyToolsuite's `FrostyModReader`, `BaseModResource`, `ResResource` and
//! `ChunkResource`; the bytes are Studio's, read to the exact end.
//!
//! Decoding a payload needs the game's Oodle library, which is reader I/O: [`crate::read::fbmod`]. [`write`] is the reader's counterpart: an unmodified file read and written again is byte-identical. The SHA-1 of a
//! payload is recomputed on writing (and checked on reading), so it is not a field.

use crate::cas::CasRaw;
use crate::{Result, bad, capacity, past_end};
use primase_bytes::Endian;
use primase_compress::Compressor;
use sha1::{Digest, Sha1};

const W: &str = "fbmod";
pub const MAGIC: [u8; 8] = *b"FROSTY\0\x01";
/// The only version Studio writes.
pub const VERSION: u32 = 6;
/// Fixed bytes before the profile: magic, version, payload table offset, payload count, profile length.
const FIXED: usize = 8 + 4 + 8 + 4 + 1;

/// The kind byte of an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntryKind {
    /// An embedded file: the icon or a screenshot.
    Embedded,
    Ebx,
    Res,
    Chunk,
}

impl EntryKind {
    pub fn id(self) -> u8 {
        match self {
            Self::Embedded => 0,
            Self::Ebx => 1,
            Self::Res => 2,
            Self::Chunk => 3,
        }
    }
}

/// What a resource entry stores beyond the common fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResInfo {
    /// The resource type hash (a MeshSet is [`crate::MESHSET`]).
    pub res_type: u32,
    pub rid: u64,
    pub meta: Vec<u8>,
}

/// What a chunk entry stores beyond the common fields (FrostyToolsuite's `ChunkResource`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkInfo {
    pub range_start: u32,
    pub range_end: u32,
    pub logical_offset: u32,
    pub logical_size: u32,
    pub h32: u32,
    pub first_mip: i32,
    /// Bundles the chunk is added to (the entry's own `bundles` are the bundles it modifies).
    pub added_bundles: Vec<u32>,
}

/// The kind of an entry with what only that kind stores.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    Embedded,
    Ebx,
    Res(ResInfo),
    Chunk(ChunkInfo),
}

impl Body {
    pub fn kind(&self) -> EntryKind {
        match self {
            Self::Embedded => EntryKind::Embedded,
            Self::Ebx => EntryKind::Ebx,
            Self::Res(_) => EntryKind::Res,
            Self::Chunk(_) => EntryKind::Chunk,
        }
    }
}

/// The payload an entry points at and how it is described.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    /// Index into [`Fbmod::payloads`]; entries may share one.
    pub index: usize,
    /// The payload's size once decoded.
    pub size: u64,
    pub flags: u8,
    pub handler: u32,
    pub user: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// `None` for an entry without a payload (index -1: the embedded files of a fresh mod).
    pub stored: Option<Stored>,
    /// Hashes of the bundles the entry modifies.
    pub bundles: Vec<u32>,
    pub body: Body,
}

impl Entry {
    pub fn kind(&self) -> EntryKind {
        self.body.kind()
    }
    /// Index of the entry's payload, if it has one.
    pub fn index(&self) -> Option<usize> {
        self.stored.as_ref().map(|s| s.index)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fbmod {
    pub version: u32,
    pub profile: String,
    /// The game version the mod was made for.
    pub game_version: u32,
    pub title: String,
    pub author: String,
    pub category: String,
    pub mod_version: String,
    pub description: String,
    pub link: String,
    pub entries: Vec<Entry>,
    /// CAS-framed payloads, as stored.
    pub payloads: Vec<Vec<u8>>,
}

/// The mod's bytes read in order ([`primase_bytes::Cursor`], little-endian), a read past them refused as the mod's.
struct Cur<'a> {
    c: primase_bytes::Cursor<'a>,
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        self.c.take(n).map_err(|e| past_end(W, e, self.c.bytes().len()))
    }
    fn arr<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.c.array().map_err(|e| past_end(W, e, self.c.bytes().len()))
    }
    fn u8(&mut self) -> Result<u8> {
        self.c.u8().map_err(|e| past_end(W, e, self.c.bytes().len()))
    }
    fn u32(&mut self) -> Result<u32> {
        self.c
            .u32(Endian::Little)
            .map_err(|e| past_end(W, e, self.c.bytes().len()))
    }
    fn i32(&mut self) -> Result<i32> {
        self.c
            .i32(Endian::Little)
            .map_err(|e| past_end(W, e, self.c.bytes().len()))
    }
    fn u64(&mut self) -> Result<u64> {
        self.c
            .u64(Endian::Little)
            .map_err(|e| past_end(W, e, self.c.bytes().len()))
    }
    fn position(&self) -> usize {
        self.c.position()
    }
    fn count(&mut self, min_bytes: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        capacity(W, n, min_bytes, self.c.remaining())
    }
    fn string(&mut self) -> Result<String> {
        let at = self.position();
        let rest = self.c.bytes().get(at..).unwrap_or_default();
        let end = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| bad(W, at, "unterminated string"))?;
        let s = std::str::from_utf8(&rest[..end]).map_err(|_| bad(W, at, "string is not UTF-8"))?;
        self.take(end + 1)?;
        Ok(s.to_owned())
    }
    fn u32s(&mut self) -> Result<Vec<u32>> {
        let n = self.count(4)?;
        (0..n).map(|_| self.u32()).collect()
    }
}

fn sha1(data: &[u8]) -> [u8; 20] {
    Sha1::digest(data).into()
}

/// Read a mod to its exact end. Refused: another magic or version, an unknown entry kind, a payload index that does
/// not exist, a stored SHA-1 that is not the payload's, payloads that are not laid out one after the other to the end
/// of the file, and bytes after them.
pub fn read(bytes: &[u8]) -> Result<Fbmod> {
    let mut c = Cur {
        c: primase_bytes::Cursor::new(bytes),
    };
    if c.arr::<8>()? != MAGIC {
        return Err(bad(W, 0, "not a FROSTY mod"));
    }
    let version = c.u32()?;
    if version != VERSION {
        return Err(bad(W, 8, format!("version {version}, expected {VERSION}")));
    }
    let table_at = usize::try_from(c.u64()?).map_err(|_| bad(W, 12, "payload table offset"))?;
    let payload_count = c.u32()? as usize;
    let plen = usize::from(c.u8()?);
    let profile = std::str::from_utf8(c.take(plen)?)
        .map_err(|_| bad(W, FIXED, "profile is not UTF-8"))?
        .to_owned();
    let game_version = c.u32()?;
    let mut details = Vec::with_capacity(6);
    for _ in 0..6 {
        details.push(c.string()?);
    }
    let [title, author, category, mod_version, description, link]: [String; 6] =
        details.try_into().map_err(|_| bad(W, c.position(), "details"))?;
    // kind, index, name terminator, bundle count
    let n = c.count(10)?;
    let mut entries = Vec::with_capacity(n);
    for _ in 0..n {
        let at = c.position();
        let kind = c.u8()?;
        let index = c.i32()?;
        let name = c.string()?;
        let index = match usize::try_from(index) {
            Ok(i) if i < payload_count => Some(i),
            _ if index == -1 => None,
            _ => return Err(bad(W, at, format!("payload {index} of {payload_count}: {name}"))),
        };
        let (stored, sha) = match index {
            Some(index) => {
                let sha = c.arr::<20>()?;
                let size = c.u64()?;
                let flags = c.u8()?;
                let handler = c.u32()?;
                let user = c.string()?;
                (
                    Some(Stored {
                        index,
                        size,
                        flags,
                        handler,
                        user,
                    }),
                    Some(sha),
                )
            }
            None => (None, None),
        };
        let bundles = c.u32s()?;
        let body = match kind {
            0 => Body::Embedded,
            1 => Body::Ebx,
            2 => {
                let res_type = c.u32()?;
                let rid = c.u64()?;
                let len = c.u32()? as usize;
                Body::Res(ResInfo {
                    res_type,
                    rid,
                    meta: c.take(len)?.to_vec(),
                })
            }
            3 => Body::Chunk(ChunkInfo {
                range_start: c.u32()?,
                range_end: c.u32()?,
                logical_offset: c.u32()?,
                logical_size: c.u32()?,
                h32: c.u32()?,
                first_mip: c.i32()?,
                added_bundles: c.u32s()?,
            }),
            k => return Err(bad(W, at, format!("unknown entry kind {k}: {name}"))),
        };
        entries.push((
            Entry {
                name,
                stored,
                bundles,
                body,
            },
            sha,
            at,
        ));
    }
    if c.position() != table_at {
        return Err(bad(
            W,
            c.position(),
            format!("entries end here, payload table at {table_at:#x}"),
        ));
    }
    let mut spans = Vec::with_capacity(capacity(
        W,
        payload_count,
        16,
        bytes.len().saturating_sub(c.position()),
    )?);
    for _ in 0..payload_count {
        spans.push((c.u64()?, c.u64()?));
    }
    let mut payloads = Vec::with_capacity(payload_count);
    let mut next = 0u64;
    for (i, (offset, size)) in spans.into_iter().enumerate() {
        if offset != next {
            return Err(bad(
                W,
                c.position(),
                format!("payload {i} at {offset:#x}, expected {next:#x}"),
            ));
        }
        next = offset
            .checked_add(size)
            .ok_or_else(|| bad(W, c.position(), format!("payload {i} size")))?;
        let size = usize::try_from(size).map_err(|_| bad(W, c.position(), format!("payload {i} size")))?;
        payloads.push(c.take(size)?.to_vec());
    }
    if c.position() != bytes.len() {
        return Err(bad(
            W,
            c.position(),
            format!("{} bytes after the last payload", bytes.len() - c.position()),
        ));
    }
    let mut out = Vec::with_capacity(entries.len());
    for (entry, sha, at) in entries {
        if let (Some(s), Some(sha)) = (&entry.stored, sha)
            && sha1(&payloads[s.index]) != sha
        {
            return Err(bad(
                W,
                at,
                format!("SHA-1 of payload {} differs: {}", s.index, entry.name),
            ));
        }
        out.push(entry);
    }
    Ok(Fbmod {
        version,
        profile,
        game_version,
        title,
        author,
        category,
        mod_version,
        description,
        link,
        entries: out,
        payloads,
    })
}

struct Out(Vec<u8>);

impl Out {
    fn u32(&mut self, v: u32) {
        self.0.extend(v.to_le_bytes());
    }
    fn string(&mut self, s: &str) -> Result<()> {
        if s.contains('\0') {
            return Err(bad(W, self.0.len(), format!("string {s:?} holds a zero byte")));
        }
        self.0.extend(s.as_bytes());
        self.0.push(0);
        Ok(())
    }
    fn u32s(&mut self, v: &[u32]) -> Result<()> {
        self.u32(u32::try_from(v.len()).map_err(|_| bad(W, self.0.len(), "list length"))?);
        v.iter().for_each(|&x| self.u32(x));
        Ok(())
    }
}

/// The counterpart of [`read`]. Refused: a version other than 6, a payload index that does not exist, text that is
/// not zero-free, a profile over 255 bytes. SHA-1s are computed from the payloads.
pub fn write(m: &Fbmod) -> Result<Vec<u8>> {
    if m.version != VERSION {
        return Err(bad(W, 8, format!("version {}, expected {VERSION}", m.version)));
    }
    let mut o = Out(Vec::new());
    o.0.extend(MAGIC);
    o.u32(m.version);
    o.0.extend(0u64.to_le_bytes());
    o.u32(u32::try_from(m.payloads.len()).map_err(|_| bad(W, 0, "payload count"))?);
    let profile = u8::try_from(m.profile.len()).map_err(|_| bad(W, FIXED - 1, "profile over 255 bytes"))?;
    o.0.push(profile);
    o.0.extend(m.profile.as_bytes());
    o.u32(m.game_version);
    for s in [
        &m.title,
        &m.author,
        &m.category,
        &m.mod_version,
        &m.description,
        &m.link,
    ] {
        o.string(s)?;
    }
    o.u32(u32::try_from(m.entries.len()).map_err(|_| bad(W, 0, "entry count"))?);
    for e in &m.entries {
        o.0.push(e.kind().id());
        match &e.stored {
            Some(s) => {
                let payload = m.payloads.get(s.index).ok_or_else(|| {
                    bad(
                        W,
                        o.0.len(),
                        format!("payload {} of {}: {}", s.index, m.payloads.len(), e.name),
                    )
                })?;
                o.u32(u32::try_from(s.index).map_err(|_| bad(W, o.0.len(), "payload index"))?);
                o.string(&e.name)?;
                o.0.extend(sha1(payload));
                o.0.extend(s.size.to_le_bytes());
                o.0.push(s.flags);
                o.u32(s.handler);
                o.string(&s.user)?;
            }
            None => {
                o.0.extend((-1i32).to_le_bytes());
                o.string(&e.name)?;
            }
        }
        o.u32s(&e.bundles)?;
        match &e.body {
            Body::Embedded | Body::Ebx => {}
            Body::Res(r) => {
                o.u32(r.res_type);
                o.0.extend(r.rid.to_le_bytes());
                o.u32(u32::try_from(r.meta.len()).map_err(|_| bad(W, o.0.len(), "metadata length"))?);
                o.0.extend(&r.meta);
            }
            Body::Chunk(c) => {
                for v in [
                    c.range_start,
                    c.range_end,
                    c.logical_offset,
                    c.logical_size,
                    c.h32,
                ] {
                    o.u32(v);
                }
                o.0.extend(c.first_mip.to_le_bytes());
                o.u32s(&c.added_bundles)?;
            }
        }
    }
    let table_at = o.0.len() as u64;
    let mut offset = 0u64;
    for p in &m.payloads {
        o.0.extend(offset.to_le_bytes());
        o.0.extend((p.len() as u64).to_le_bytes());
        offset += p.len() as u64;
    }
    for p in &m.payloads {
        o.0.extend(p);
    }
    o.0[12..20].copy_from_slice(&table_at.to_le_bytes());
    Ok(o.0)
}

impl Fbmod {
    /// Add `raw` as an uncompressed CAS payload (raw 64 KiB blocks: no encoder or compression settings involved) and
    /// return its index: an identical payload already in the mod is reused.
    pub fn add_payload(&mut self, raw: &[u8]) -> Result<usize> {
        let encoded = CasRaw.compress(raw)?;
        if let Some(i) = self.payloads.iter().position(|p| *p == encoded) {
            return Ok(i);
        }
        self.payloads.push(encoded);
        Ok(self.payloads.len() - 1)
    }

    /// Remove the payloads no entry points at (what an entry given a new payload leaves behind), keeping the others in
    /// order and renumbering the entries' indices; return how many were removed.
    pub fn drop_unused_payloads(&mut self) -> usize {
        let mut used = vec![false; self.payloads.len()];
        for s in self.entries.iter().filter_map(|e| e.stored.as_ref()) {
            if let Some(u) = used.get_mut(s.index) {
                *u = true;
            }
        }
        let mut renumbered = Vec::with_capacity(used.len());
        let mut next = 0;
        for &u in &used {
            renumbered.push(next);
            next += usize::from(u);
        }
        let mut keep = used.iter();
        self.payloads.retain(|_| keep.next().copied().unwrap_or(false));
        for s in self.entries.iter_mut().filter_map(|e| e.stored.as_mut()) {
            if let Some(&n) = renumbered.get(s.index) {
                s.index = n;
            }
        }
        used.len() - next
    }
}
