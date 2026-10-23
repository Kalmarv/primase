//! The GLB container (glTF 2.0 §4.4): a 12-byte header, a JSON chunk, an optional BIN chunk.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

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
    let Some((&[m0, m1, m2, m3, v0, v1, v2, v3, l0, l1, l2, l3], mut rest)) = bytes.split_first_chunk::<12>()
    else {
        return Err(bad(0, "shorter than the 12-byte header"));
    };
    if u32::from_le_bytes([m0, m1, m2, m3]) != MAGIC {
        return Err(bad(0, "not a GLB (magic is not glTF)"));
    }
    let version = u32::from_le_bytes([v0, v1, v2, v3]);
    if version != 2 {
        return Err(bad(4, &format!("container version {version}, not 2")));
    }
    let length = u32::from_le_bytes([l0, l1, l2, l3]);
    if usize::try_from(length).ok() != Some(bytes.len()) {
        return Err(bad(
            8,
            &format!("header says {length} bytes, the file has {}", bytes.len()),
        ));
    }
    let mut json = None;
    let mut bin = None;
    while !rest.is_empty() {
        // where this chunk starts in the file: `rest` is the file's tail
        let off = bytes.len().saturating_sub(rest.len());
        let Some((&[l0, l1, l2, l3, k0, k1, k2, k3], tail)) = rest.split_first_chunk::<8>() else {
            return Err(bad(off, "truncated chunk header"));
        };
        let len = u32::from_le_bytes([l0, l1, l2, l3]);
        let kind = u32::from_le_bytes([k0, k1, k2, k3]);
        if !len.is_multiple_of(4) {
            return Err(bad(off, &format!("chunk length {len} is not a multiple of 4")));
        }
        let Some((data, after)) = usize::try_from(len).ok().and_then(|n| tail.split_at_checked(n)) else {
            return Err(bad(off, "chunk runs past the end of the file"));
        };
        match (kind, json.is_some(), bin.is_some()) {
            (CHUNK_JSON, false, _) => json = Some(data),
            (CHUNK_BIN, true, false) => bin = Some(data),
            (CHUNK_JSON, true, _) => return Err(bad(off, "a second JSON chunk")),
            (CHUNK_BIN, false, _) => return Err(bad(off, "BIN chunk before the JSON chunk")),
            (CHUNK_BIN, true, true) => return Err(bad(off, "a second BIN chunk")),
            _ => return Err(bad(off, &format!("unknown chunk type 0x{kind:08x}"))),
        }
        rest = after;
    }
    let json = json.ok_or_else(|| bad(12, "no JSON chunk"))?;
    Ok(Chunks { json, bin })
}

/// The length of a GLB's JSON chunk from the file's first 20 bytes (the header and the first chunk's header), for a
/// reader that reads the JSON alone: an error unless they start a GLB whose first chunk is JSON. That the chunk fits
/// in the file is the caller's to check.
pub(crate) fn json_chunk_length(head: &[u8; 20]) -> Result<u32> {
    let bad = |offset: usize, detail: &str| Error::Glb {
        offset,
        detail: detail.to_owned(),
    };
    let [
        m0,
        m1,
        m2,
        m3,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        l0,
        l1,
        l2,
        l3,
        k0,
        k1,
        k2,
        k3,
    ] = *head;
    if u32::from_le_bytes([m0, m1, m2, m3]) != MAGIC {
        return Err(bad(0, "not a GLB"));
    }
    if u32::from_le_bytes([k0, k1, k2, k3]) != CHUNK_JSON {
        return Err(bad(16, "the first chunk is not JSON"));
    }
    Ok(u32::from_le_bytes([l0, l1, l2, l3]))
}

/// Assemble a GLB from the JSON text and the binary buffer, padding the JSON with spaces and the buffer with zeros
/// to 4 bytes. An empty `bin` writes no BIN chunk.
pub fn assemble(json: &[u8], bin: &[u8]) -> Result<Vec<u8>> {
    let json_len = json.len().next_multiple_of(4);
    let bin_len = bin.len().next_multiple_of(4);
    // the header, the JSON chunk's header and its data, the BIN chunk's when there is one
    let parts = [
        20,
        json_len,
        if bin.is_empty() {
            0
        } else {
            bin_len.saturating_add(8)
        },
    ];
    let total = parts.iter().fold(0u64, |t, &p| t.saturating_add(p as u64));
    let too_large = || Error::TooLarge { bytes: total };
    let header = |n: usize| u32::try_from(n).map_err(|_| too_large());
    let total32 = u32::try_from(total).map_err(|_| too_large())?;
    let mut out = Vec::with_capacity(total32 as usize);
    for v in [MAGIC, 2, total32, header(json_len)?, CHUNK_JSON] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    // the headers before each chunk's data are 4-byte multiples, so padding the file pads the chunk
    out.extend_from_slice(json);
    out.resize(out.len().next_multiple_of(4), b' ');
    if !bin.is_empty() {
        out.extend_from_slice(&header(bin_len)?.to_le_bytes());
        out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
        out.extend_from_slice(bin);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    debug_assert_eq!(out.len() as u64, total);
    Ok(out)
}

/// A GLB held as its two parts, to be changed by several stages and assembled once: the JSON document and the
/// binary chunk, 4-byte padded with zeros as [`parse`] returns a GLB's (so a stage sees what it would see after an
/// [`assemble`] and a [`parse`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub json: serde_json::Value,
    pub bin: Vec<u8>,
}

impl Document {
    /// The parts, the binary chunk padded.
    pub fn new(json: serde_json::Value, mut bin: Vec<u8>) -> Self {
        bin.resize(bin.len().next_multiple_of(4), 0);
        Document { json, bin }
    }
    /// A GLB's parts ([`parse`]).
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let chunks = parse(bytes)?;
        Ok(Document::new(
            serde_json::from_slice(chunks.json)?,
            chunks.bin.unwrap_or(&[]).to_vec(),
        ))
    }
    /// The GLB ([`assemble`]).
    pub fn to_glb(&self) -> Result<Vec<u8>> {
        assemble(&serde_json::to_vec(&self.json)?, &self.bin)
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    reason = "test fixtures built by hand"
)]
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
        // a chunk longer than the file, and a chunk header cut short (the file's length agreeing)
        let mut past = good.clone();
        past[12..16].copy_from_slice(&64u32.to_le_bytes());
        let e = parse(&past).unwrap_err().to_string();
        assert!(e.contains("chunk runs past the end of the file"), "{e}");
        let mut cut = good.clone();
        cut.extend_from_slice(&[0; 4]);
        let n = cut.len() as u32;
        cut[8..12].copy_from_slice(&n.to_le_bytes());
        let e = parse(&cut).unwrap_err().to_string();
        assert!(e.contains("truncated chunk header"), "{e}");
    }

    /// The first 20 bytes give the JSON chunk's length; anything but a GLB whose first chunk is JSON is refused.
    #[test]
    fn the_json_chunk_length_is_read_from_the_first_twenty_bytes() {
        let glb = assemble(b"{}", &[1; 4]).unwrap();
        let head: [u8; 20] = glb[..20].try_into().unwrap();
        assert_eq!(json_chunk_length(&head).unwrap(), 4);
        let mut bin_first = head;
        bin_first[16..20].copy_from_slice(&CHUNK_BIN.to_le_bytes());
        assert!(json_chunk_length(&bin_first).is_err());
        let mut not_glb = head;
        not_glb[0] = b'x';
        assert!(json_chunk_length(&not_glb).is_err());
    }
}
