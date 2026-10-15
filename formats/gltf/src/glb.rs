//! The GLB container (glTF 2.0 §4.4): a 12-byte header, a JSON chunk, an optional BIN chunk.

use crate::{Error, Result};

const MAGIC: u32 = 0x4654_6C67; // "glTF"
const CHUNK_JSON: u32 = 0x4E4F_534A; // "JSON"
const CHUNK_BIN: u32 = 0x004E_4942; // "BIN\0"

/// The two chunks of a GLB, borrowed from its bytes. The JSON chunk's trailing space padding is kept (JSON parsers
/// skip it); `bin` is the whole BIN chunk including its zero padding.
#[derive(Clone, Copy, Debug)]
pub struct Chunks<'a> {
    pub json: &'a [u8],
    pub bin: Option<&'a [u8]>,
}

/// Split a GLB into its chunks, strictly: the header's length is the file's, chunk lengths are multiples of 4, the
/// JSON chunk comes first, at most one BIN chunk follows, and nothing else is in the file.
pub fn parse(bytes: &[u8]) -> Result<Chunks<'_>> {
    let bad = |offset: usize, detail: &str| Error::Glb {
        offset,
        detail: detail.to_owned(),
    };
    if bytes.len() < 12 {
        return Err(bad(0, "shorter than the 12-byte header"));
    }
    if u32_at(bytes, 0) != MAGIC {
        return Err(bad(0, "not a GLB (magic is not glTF)"));
    }
    if u32_at(bytes, 4) != 2 {
        return Err(bad(4, &format!("container version {}, not 2", u32_at(bytes, 4))));
    }
    if u32_at(bytes, 8) as usize != bytes.len() {
        return Err(bad(
            8,
            &format!(
                "header says {} bytes, the file has {}",
                u32_at(bytes, 8),
                bytes.len()
            ),
        ));
    }
    let mut off = 12;
    let mut json = None;
    let mut bin = None;
    while off < bytes.len() {
        if bytes.len() - off < 8 {
            return Err(bad(off, "truncated chunk header"));
        }
        let len = u32_at(bytes, off) as usize;
        let kind = u32_at(bytes, off + 4);
        let start = off + 8;
        if !len.is_multiple_of(4) {
            return Err(bad(off, &format!("chunk length {len} is not a multiple of 4")));
        }
        if len > bytes.len() - start {
            return Err(bad(off, "chunk runs past the end of the file"));
        }
        let data = &bytes[start..start + len];
        match (kind, json.is_some(), bin.is_some()) {
            (CHUNK_JSON, false, _) => json = Some(data),
            (CHUNK_BIN, true, false) => bin = Some(data),
            (CHUNK_JSON, true, _) => return Err(bad(off, "a second JSON chunk")),
            (CHUNK_BIN, false, _) => return Err(bad(off, "BIN chunk before the JSON chunk")),
            (CHUNK_BIN, true, true) => return Err(bad(off, "a second BIN chunk")),
            _ => return Err(bad(off, &format!("unknown chunk type 0x{kind:08x}"))),
        }
        off = start + len;
    }
    let json = json.ok_or_else(|| bad(12, "no JSON chunk"))?;
    Ok(Chunks { json, bin })
}

/// Assemble a GLB from the JSON text and the binary buffer, padding the JSON with spaces and the buffer with zeros
/// to 4 bytes. An empty `bin` writes no BIN chunk.
pub fn assemble(json: &[u8], bin: &[u8]) -> Result<Vec<u8>> {
    let json_len = json.len().next_multiple_of(4);
    let bin_len = bin.len().next_multiple_of(4);
    let total = 12 + 8 + json_len as u64 + if bin.is_empty() { 0 } else { 8 + bin_len as u64 };
    if total > u64::from(u32::MAX) {
        return Err(Error::TooLarge { bytes: total });
    }
    let mut out = Vec::with_capacity(total as usize);
    for v in [MAGIC, 2, total as u32, json_len as u32, CHUNK_JSON] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(json);
    out.resize(out.len() + json_len - json.len(), b' ');
    if !bin.is_empty() {
        out.extend_from_slice(&(bin_len as u32).to_le_bytes());
        out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
        out.extend_from_slice(bin);
        out.resize(out.len() + bin_len - bin.len(), 0);
    }
    debug_assert_eq!(out.len() as u64, total);
    Ok(out)
}

fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assemble_then_parse() {
        let glb = assemble(br#"{"asset":{"version":"2.0"}}"#, &[1, 2, 3, 4, 5]).unwrap();
        assert_eq!(glb.len() % 4, 0);
        let c = parse(&glb).unwrap();
        assert_eq!(c.json, b"{\"asset\":{\"version\":\"2.0\"}} ");
        assert_eq!(c.bin.unwrap(), [1, 2, 3, 4, 5, 0, 0, 0]);
        let no_bin = assemble(b"{}", &[]).unwrap();
        assert!(parse(&no_bin).unwrap().bin.is_none());
    }

    #[test]
    fn strict_container_errors() {
        let good = assemble(b"{}", &[9; 4]).unwrap();
        let mut long = good.clone();
        long.extend_from_slice(&[0; 4]);
        assert!(parse(&long).is_err(), "bytes after the declared length");
        let mut wrong_len = good.clone();
        wrong_len[8] ^= 4;
        assert!(parse(&wrong_len).is_err());
        let mut unknown = good.clone();
        unknown[28..32].copy_from_slice(b"XTRA");
        assert!(parse(&unknown).is_err(), "unknown chunk type");
        assert!(parse(&good[..10]).is_err());
    }
}
