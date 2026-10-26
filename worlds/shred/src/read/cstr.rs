//! CSTR v1: exact block framing, stored blocks or RFC 1951 raw DEFLATE.
use crate::{Result, invalid};
use primase_bytes::{Endian::Big, Field, View};
use primase_compress::{Decompressor, Deflate};

/// Decoded bytes and block counts (including stored blocks).
#[derive(Debug)]
pub struct Decoded {
    pub bytes: Vec<u8>,
    pub blocks: usize,
    pub stored_blocks: usize,
}

/// Decode exactly one stream, checking its total against the enclosing entry size.
/// Errors use stream-relative offsets.
pub fn decode(bytes: &[u8], expected: usize) -> Result<Decoded> {
    let v = View::new(bytes);
    if v.slice(0, 4).field("CSTR magic")? != b"CSTR" {
        return Err(invalid(0, "expected CSTR"));
    }
    for (at, want, what) in [
        (4, 0xfeed, "CSTR endian tag"),
        (6, 1, "CSTR version"),
        (10, 0xf32f, "CSTR threshold"),
    ] {
        if v.u16(at, Big).field(what)? != want {
            return Err(invalid(at, what));
        }
    }
    let count = usize::from(v.u16(8, Big).field("block count")?);
    if v.u32(12, Big).field("stream size")? as usize != expected {
        return Err(invalid(12, "stream/entry size mismatch"));
    }
    let mut out = Vec::new();
    let mut pos = 16;
    let mut stored = 0;
    for _ in 0..count {
        let csize = usize::from(v.u16(pos, Big).field("block compressed size")?);
        let usize = usize::from(v.u16(pos + 2, Big).field("block output size")?);
        if out.len().checked_add(usize).is_none_or(|n| n > expected) {
            return Err(invalid(pos + 2, "block exceeds stream output size"));
        }
        pos += 4;
        let payload = v.slice(pos, csize).field("block payload")?;
        out.try_reserve(usize).map_err(|e| invalid(pos, e))?;
        if csize == usize {
            stored += 1;
            out.extend_from_slice(payload);
        } else {
            let decoded = Deflate.decompress(payload, usize).map_err(|e| invalid(pos, e))?;
            out.extend_from_slice(&decoded);
        }
        pos += csize;
    }
    if pos != bytes.len() {
        return Err(invalid(pos, "trailing bytes after CSTR blocks"));
    }
    if out.len() != expected {
        return Err(invalid(pos, "block output sum differs from stream size"));
    }
    Ok(Decoded {
        bytes: out,
        blocks: count,
        stored_blocks: stored,
    })
}
