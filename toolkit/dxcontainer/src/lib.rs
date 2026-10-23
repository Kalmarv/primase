//! The DXBC container: what Shader Model 4 and 5 programs (`SHDR`, `SHEX`, signatures, `RDEF`) and Shader Model 6 programs
//! (`DXIL`, `PSV0`, `RDAT`, ...) are stored in. [`chunks`] reads its part directory and [`checksum`] computes the digest the
//! compiler stores in it.
//!
//! The layout, which every one of the 77,956 containers of the project's shader corpora and fixtures has: a header (`DXBC`, a
//! 16-byte digest, version 1, the container's size, the number of parts), the directory of the parts' offsets right after it,
//! then the parts one after another in the directory's order, each its four-character kind, its length and its bytes, the last
//! ending where the container does. Shader Model 6 parts are not padded to 4 bytes, so an offset need not be a multiple of 4.
//! The reader reads to the exact end: a part anywhere else, a gap, a byte after the last part, a kind twice, or a size
//! other than the container's is an error naming where. The digest is not verified (it is not an authenticity check);
//! [`checksum`] is for a caller that wants to know it.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use primase_bytes::View;

/// What is wrong with a container, and where (a byte offset in it).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("DXBC container at byte {offset}: {detail}")]
pub struct Error {
    pub offset: usize,
    pub detail: String,
}

fn error(offset: usize, detail: impl Into<String>) -> Error {
    Error {
        offset,
        detail: detail.into(),
    }
}

/// A part of a container: its four-character kind and its bytes.
#[derive(Clone, Copy, Debug)]
pub struct Chunk<'a> {
    pub kind: [u8; 4],
    pub data: &'a [u8],
}

/// The part directory of the container `bytes`, in the container's order. See the module documentation for what is refused.
pub fn chunks(bytes: &[u8]) -> Result<Vec<Chunk<'_>>, Error> {
    let view = View::new(bytes);
    let word = |at: usize| -> Result<usize, Error> {
        let word = view.array::<4>(at).map_err(|e| error(at, e.to_string()))?;
        usize::try_from(u32::from_le_bytes(word)).map_err(|_| error(at, "a word past the address space"))
    };
    if bytes.get(..4) != Some(b"DXBC") {
        return Err(error(0, "the magic is not DXBC"));
    }
    if word(20)? != 1 {
        return Err(error(20, "the container's version is not 1"));
    }
    if word(24)? != bytes.len() {
        return Err(error(
            24,
            format!(
                "the container states a size of {}, it has {} bytes",
                word(24)?,
                bytes.len()
            ),
        ));
    }
    let count = word(28)?;
    let directory_end = count
        .checked_mul(4)
        .and_then(|n| n.checked_add(32))
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| {
            error(
                28,
                format!("a directory of {count} parts does not fit the container"),
            )
        })?;
    let mut parts: Vec<Chunk<'_>> = Vec::new();
    let mut next = directory_end;
    for i in 0..count {
        let entry = i
            .checked_mul(4)
            .and_then(|n| n.checked_add(32))
            .ok_or_else(|| error(28, "a directory past the address space"))?;
        let start = word(entry)?;
        if start != next {
            let shown = view.array::<4>(start).map_or_else(
                |_| "past the end".to_owned(),
                |kind| String::from_utf8_lossy(&kind).into_owned(),
            );
            return Err(error(
                entry,
                if start < next {
                    format!(
                        "overlapping parts: part {i} ({shown}) starts at {start}, before the previous part ends at {next}"
                    )
                } else {
                    format!(
                        "{} bytes between parts: part {i} ({shown}) starts at {start}, where the previous part ends at {next}",
                        start.saturating_sub(next)
                    )
                },
            ));
        }
        let kind = view.array::<4>(start).map_err(|e| error(start, e.to_string()))?;
        let shown = String::from_utf8_lossy(&kind).into_owned();
        let length = word(
            start
                .checked_add(4)
                .ok_or_else(|| error(start, "a part past the address space"))?,
        )?;
        let data_at = start
            .checked_add(8)
            .ok_or_else(|| error(start, "a part past the address space"))?;
        let data = view
            .slice(data_at, length)
            .map_err(|e| error(start, format!("part {i} ({shown}): {e}")))?;
        if parts.iter().any(|p| p.kind == kind) {
            return Err(error(start, format!("a part kind twice: {shown}")));
        }
        next = data_at
            .checked_add(length)
            .ok_or_else(|| error(start, "a part past the address space"))?;
        parts.push(Chunk { kind, data });
    }
    if next != bytes.len() {
        return Err(error(
            next,
            format!("{} bytes after the last part", bytes.len().saturating_sub(next)),
        ));
    }
    Ok(parts)
}

/// The digest a container stores at bytes 4 to 20: MD5's block function over its bytes from 20 on, the last block padded as the
/// compiler does (the bit count first, then the rest, `0x80`, zeros, and at 60 the bit count over 4, or 1; a rest of 56 bytes
/// or more padded in a block of its own first). `None` for fewer than 20 bytes.
pub fn checksum(container: &[u8]) -> Option<[u8; 16]> {
    let data = container.get(20..)?;
    let bits = u32::try_from(data.len()).ok()?.wrapping_mul(8);
    let (blocks, rest) = data.as_chunks::<64>();
    let mut state = primase_hash::MD5_INIT;
    for block in blocks {
        primase_hash::md5_block(&mut state, block);
    }
    let mut last = [0u8; 64];
    if rest.len() < 56 {
        last.get_mut(4..)?.get_mut(..rest.len())?.copy_from_slice(rest);
        *last.get_mut(rest.len().checked_add(4)?)? = 0x80;
    } else {
        let mut first = [0u8; 64];
        first.get_mut(..rest.len())?.copy_from_slice(rest);
        *first.get_mut(rest.len())? = 0x80;
        primase_hash::md5_block(&mut state, &first);
    }
    last.get_mut(..4)?.copy_from_slice(&bits.to_le_bytes());
    last.get_mut(60..)?
        .copy_from_slice(&((bits >> 2) | 1).to_le_bytes());
    primase_hash::md5_block(&mut state, &last);
    Some(primase_hash::md5_bytes(state))
}
