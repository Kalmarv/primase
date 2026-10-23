#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::{Result, invalid};

/// A section's absolute location, flags and borrowed payload. Container sections
/// have flags zero; their payload consists entirely of child sections.
#[derive(Debug, Clone, Copy)]
pub struct Section<'a> {
    pub offset: usize,
    pub signature: [u8; 4],
    pub flags: u8,
    pub payload: &'a [u8],
}

impl<'a> Section<'a> {
    pub fn children(self) -> Result<Vec<Self>> {
        if self.flags != 0 {
            return Err(invalid(self.offset, "leaf section has no children"));
        }
        sections_at(self.payload, self.offset.saturating_add(8))
    }
}

/// Parse a complete sequence of sections, rejecting partial headers, lengths
/// below eight bytes, and lengths beyond the supplied parent slice.
pub fn sections(bytes: &[u8]) -> Result<Vec<Section<'_>>> {
    sections_at(bytes, 0)
}

fn sections_at(bytes: &[u8], base: usize) -> Result<Vec<Section<'_>>> {
    let mut result = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        // `rest` is the tail of `bytes`, which starts at `base` in the file
        let offset = base.saturating_add(bytes.len().saturating_sub(rest.len()));
        let Some((&[s0, s1, s2, s3, g0, g1, g2, g3], body)) = rest.split_first_chunk::<8>() else {
            return Err(invalid(offset, "truncated section header"));
        };
        // a big-endian word: the flags in the top two bits, the section's length (its header's 8 bytes with it) below
        let size = u32::from_be_bytes([s0 & 0x3f, s1, s2, s3]);
        let Some(length) = size.checked_sub(8) else {
            return Err(invalid(offset, "section length is less than its header"));
        };
        let Some((payload, after)) = usize::try_from(length)
            .ok()
            .and_then(|n| body.split_at_checked(n))
        else {
            return Err(invalid(offset, "section exceeds its parent"));
        };
        result.push(Section {
            offset,
            signature: [g0, g1, g2, g3],
            flags: s0 >> 6,
            payload,
        });
        rest = after;
    }
    Ok(result)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, reason = "test fixtures built by hand")]
mod tests {
    use super::*;

    #[test]
    fn nested_sections_have_absolute_offsets() {
        let bytes = b"\0\0\0\x18TAG0\x40\0\0\x10SDKV20160200";
        let root = sections(bytes).expect("root");
        assert_eq!(root.len(), 1);
        let children = root[0].children().expect("children");
        assert_eq!(children[0].offset, 8);
        assert_eq!(children[0].signature, *b"SDKV");
        assert_eq!(children[0].payload, b"20160200");
        assert!(children[0].children().is_err());
    }

    #[test]
    fn truncated_or_escaping_sections_fail() {
        let bytes = b"\0\0\0\x18TAG0\x40\0\0\x10SDKV20160200";
        for end in 1..bytes.len() {
            assert!(sections(&bytes[..end]).is_err(), "prefix {end}");
        }
        assert!(sections(b"\0\0\0\x07TAG0").is_err());
        let bad_child = b"\0\0\0\x10TAG0\x40\0\0\x10SDKV";
        assert!(sections(bad_child).expect("root")[0].children().is_err());
    }
}
