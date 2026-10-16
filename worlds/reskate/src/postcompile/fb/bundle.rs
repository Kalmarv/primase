//! A bundle: its region in the TOC (the file table: where each of its files lives) and its binary manifest (the
//! first file: names, SHA-1s and sizes of the bundle's EBX, resources and chunks, plus the chunk metadata DB).

use crate::postcompile::fb::toc::Loc;
use crate::postcompile::{Rd, Result, bad, capacity, latin1};

const R: &str = "bundle region";
const M: &str = "bundle manifest";
pub const BUNDLE_SALT: u32 = 0x7065_636E;
pub const BUNDLE_MAGIC: u32 = 0xED1C_EDB8;

/// One file of a bundle: archive, offset, stored size.
pub type FileRef = (Loc, u32, u32);

/// The region's file table and its optional inline manifest.
pub fn read_region(r: &[u8]) -> Result<(Vec<FileRef>, Vec<u8>)> {
    let rd = Rd::new(r, R);
    let int = |i: usize| rd.i32be(4 * i);
    let (mo, ms, lo, total, dato, ro, co, total2) = (
        int(0)?,
        int(1)?,
        rd.u32be(8)?,
        int(3)?,
        rd.u32be(16)?,
        rd.u32be(20)?,
        rd.u32be(24)?,
        int(8)?,
    );
    if mo < 0 || ms < 0 || total < 0 || total != total2 || ro != dato || co != dato {
        return Err(bad(R, 0, "header is invalid"));
    }
    let inline = if ms > 0 {
        rd.bytes(mo as usize, ms as usize)?.to_vec()
    } else {
        Vec::new()
    };
    // each file has a flag byte and an 8-byte offset and size
    let mut files = Vec::with_capacity(capacity(R, total as usize, 9, r.len())?);
    let mut pos = dato as usize;
    let mut cur: Option<Loc> = None;
    for i in 0..total as usize {
        let flag = rd.u8(lo as usize + i)?;
        if flag == 1 {
            let enc = rd.u32be(pos)?;
            pos += 4;
            cur = Some(Loc {
                patch: enc & 0x00FF_0000 != 0,
                chunk: (enc >> 8) & 0xFF,
                archive: (enc & 0xFF) as u16,
            });
        } else if flag != 0 {
            cur = Some(Loc::decode(rd.u64be(pos)?));
            pos += 8;
        }
        let loc = cur.ok_or_else(|| bad(R, pos, "first file has no archive identifier"))?;
        files.push((loc, rd.u32be(pos)?, rd.u32be(pos + 4)?));
        pos += 8;
    }
    Ok((files, inline))
}

/// The counterpart of [`read_region`] for the layout Studio and ReSkate write (8-byte identifiers).
pub fn write_region(files: &[FileRef], inline: &[u8]) -> Vec<u8> {
    let mut table = Vec::new();
    let mut flags = Vec::with_capacity(files.len());
    let mut prev = None;
    for &(loc, off, size) in files {
        if prev == Some(loc) {
            flags.push(0u8);
        } else {
            flags.push(0x80);
            table.extend(loc.encode().to_be_bytes());
            prev = Some(loc);
        }
        table.extend(off.to_be_bytes());
        table.extend(size.to_be_bytes());
    }
    if let Some(f) = flags.first_mut() {
        *f |= 0x04;
    }
    let data_at = 0x24u32;
    let meta_at = if inline.is_empty() {
        0
    } else {
        data_at + table.len() as u32
    };
    let loc_at = if inline.is_empty() {
        data_at + table.len() as u32
    } else {
        meta_at + inline.len() as u32
    };
    let n = files.len() as u32;
    let mut out = Vec::with_capacity(0x24 + table.len() + inline.len() + flags.len());
    for v in [
        meta_at,
        inline.len() as u32,
        loc_at,
        n,
        data_at,
        data_at,
        data_at,
        0,
        n,
    ] {
        out.extend(v.to_be_bytes());
    }
    out.extend(table);
    out.extend(inline);
    out.extend(flags);
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Ebx,
    Res,
    Chunk,
}

/// One manifest row. EBX and resources have a name and original size; resources a type, 16 metadata bytes and an id;
/// chunks a GUID and a logical offset / size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub kind: Kind,
    pub name: String,
    pub sha1: [u8; 20],
    pub size: u32,
    pub rtype: u32,
    pub rmeta: [u8; 16],
    pub rid: u64,
    pub guid: [u8; 16],
    pub loff: u32,
    pub lsize: u32,
}

impl Asset {
    fn new(kind: Kind) -> Self {
        Self {
            kind,
            name: String::new(),
            sha1: [0; 20],
            size: 0,
            rtype: 0,
            rmeta: [0; 16],
            rid: 0,
            guid: [0; 16],
            loff: 0,
            lsize: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub ebx: Vec<Asset>,
    pub res: Vec<Asset>,
    pub chunks: Vec<Asset>,
    /// The chunk metadata DB, as stored.
    pub meta: Vec<u8>,
    pub big_endian: bool,
}

impl Manifest {
    pub fn count(&self) -> usize {
        self.ebx.len() + self.res.len() + self.chunks.len()
    }
    /// Index in the region's file list (manifest first) of resource `i` / chunk `i`.
    pub fn res_file(&self, i: usize) -> usize {
        1 + self.ebx.len() + i
    }
    pub fn chunk_file(&self, i: usize) -> usize {
        1 + self.ebx.len() + self.res.len() + i
    }
    pub fn ebx_file(&self, i: usize) -> usize {
        1 + i
    }
}

/// Whether `m` has a manifest's header (declared size and salted magic, either endianness).
pub fn is_manifest(m: &[u8]) -> bool {
    if m.len() < 36 {
        return false;
    }
    let declared = u32::from_be_bytes([m[0], m[1], m[2], m[3]]) as usize;
    let be = u32::from_be_bytes([m[4], m[5], m[6], m[7]]);
    let le = u32::from_le_bytes([m[4], m[5], m[6], m[7]]);
    declared + 4 == m.len() && (be ^ BUNDLE_SALT == BUNDLE_MAGIC || le ^ BUNDLE_SALT == BUNDLE_MAGIC)
}

pub fn read_manifest(m: &[u8]) -> Result<Manifest> {
    let rd = Rd::new(m, M);
    if m.len() < 36 {
        return Err(bad(M, 0, "header is truncated"));
    }
    if rd.u32be(0)? as usize + 4 != m.len() {
        return Err(bad(M, 0, "declared size does not match"));
    }
    let big_endian = rd.u32be(4)? ^ BUNDLE_SALT == BUNDLE_MAGIC;
    if !big_endian && rd.u32le(4)? ^ BUNDLE_SALT != BUNDLE_MAGIC {
        return Err(bad(M, 4, "magic is unsupported"));
    }
    let u32e = |at: usize| if big_endian { rd.u32be(at) } else { rd.u32le(at) };
    let u64e = |at: usize| if big_endian { rd.u64be(at) } else { rd.u64le(at) };
    let (total, ne, nr, nc) = (
        u32e(8)? as usize,
        u32e(12)? as usize,
        u32e(16)? as usize,
        u32e(20)? as usize,
    );
    let (strings, mo, ms) = (u32e(24)? as usize + 4, u32e(28)? as usize, u32e(32)? as usize);
    if ne + nr + nc != total {
        return Err(bad(M, 8, "asset counts do not add up"));
    }
    let mut pos = 36;
    let mut hashes = Vec::with_capacity(capacity(M, total, 20, m.len() - 36)?);
    for _ in 0..total {
        hashes.push(rd.arr::<20>(pos)?);
        pos += 20;
    }
    let mut b = Manifest {
        ebx: Vec::new(),
        res: Vec::new(),
        chunks: Vec::new(),
        meta: Vec::new(),
        big_endian,
    };
    for (n, kind) in [(ne, Kind::Ebx), (nr, Kind::Res)] {
        for _ in 0..n {
            let mut a = Asset::new(kind);
            a.name = rd.cstr(strings + u32e(pos)? as usize)?;
            a.size = u32e(pos + 4)?;
            pos += 8;
            if kind == Kind::Ebx {
                b.ebx.push(a)
            } else {
                b.res.push(a)
            }
        }
    }
    for (i, a) in b.ebx.iter_mut().chain(b.res.iter_mut()).enumerate() {
        a.sha1 = hashes[i];
    }
    for a in &mut b.res {
        a.rtype = u32e(pos)?;
        pos += 4;
    }
    for a in &mut b.res {
        a.rmeta = rd.arr(pos)?;
        pos += 16;
    }
    for a in &mut b.res {
        a.rid = u64e(pos)?;
        pos += 8;
    }
    for i in 0..nc {
        let mut a = Asset::new(Kind::Chunk);
        a.guid = rd.arr(pos)?;
        if big_endian {
            a.guid.reverse();
        }
        a.loff = u32e(pos + 16)?;
        a.lsize = u32e(pos + 20)?;
        pos += 24;
        a.sha1 = hashes[ne + nr + i];
        a.size = (a.loff & 0xFFFF) | a.lsize;
        b.chunks.push(a);
    }
    if ms > 0 {
        b.meta = rd.bytes(4 + mo, ms)?.to_vec();
    }
    Ok(b)
}

/// Little endian, as Studio and ReSkate write.
pub fn write_manifest(b: &Manifest) -> Result<Vec<u8>> {
    if b.big_endian {
        return Err(bad(M, 0, "only little-endian manifests are written"));
    }
    let total = b.count();
    let mut strings = Vec::new();
    let mut offsets = Vec::with_capacity(b.ebx.len() + b.res.len());
    for a in b.ebx.iter().chain(&b.res) {
        offsets.push(strings.len() as u32);
        strings.extend(latin1(&a.name, M)?);
        strings.push(0);
    }
    let string_at =
        36 + 20 * total + 8 * (b.ebx.len() + b.res.len()) + 28 * b.res.len() + 24 * b.chunks.len();
    let meta_at = string_at + strings.len();
    let len = meta_at + b.meta.len();
    let mut out = Vec::with_capacity(len);
    out.extend(((len - 4) as u32).to_be_bytes());
    out.extend((BUNDLE_MAGIC ^ BUNDLE_SALT).to_le_bytes());
    let meta_field = if b.meta.is_empty() {
        0
    } else {
        (meta_at - 4) as u32
    };
    for v in [
        total as u32,
        b.ebx.len() as u32,
        b.res.len() as u32,
        b.chunks.len() as u32,
        (string_at - 4) as u32,
        meta_field,
        b.meta.len() as u32,
    ] {
        out.extend(v.to_le_bytes());
    }
    for a in b.ebx.iter().chain(&b.res).chain(&b.chunks) {
        out.extend(a.sha1);
    }
    for (a, off) in b.ebx.iter().chain(&b.res).zip(&offsets) {
        out.extend(off.to_le_bytes());
        out.extend(a.size.to_le_bytes());
    }
    for a in &b.res {
        out.extend(a.rtype.to_le_bytes());
    }
    for a in &b.res {
        out.extend(a.rmeta);
    }
    for a in &b.res {
        out.extend(a.rid.to_le_bytes());
    }
    for a in &b.chunks {
        out.extend(a.guid);
        out.extend(a.loff.to_le_bytes());
        out.extend(a.lsize.to_le_bytes());
    }
    out.extend(strings);
    out.extend(&b.meta);
    debug_assert_eq!(out.len(), len);
    Ok(out)
}
