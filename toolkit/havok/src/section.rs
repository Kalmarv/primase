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
        sections_at(self.payload, self.offset + 8)
    }
}

/// Parse a complete sequence of sections, rejecting partial headers, lengths
/// below eight bytes, and lengths beyond the supplied parent slice.
pub fn sections(bytes: &[u8]) -> Result<Vec<Section<'_>>> {
    sections_at(bytes, 0)
}

fn sections_at(bytes: &[u8], base: usize) -> Result<Vec<Section<'_>>> {
    let mut result = Vec::new();
    let mut position = 0;
    while position < bytes.len() {
        let offset = base + position;
        let header = bytes
            .get(position..position + 8)
            .ok_or_else(|| invalid(offset, "truncated section header"))?;
        let size_flags = u32::from_be_bytes(header[..4].try_into().expect("four bytes"));
        let size = (size_flags & 0x3fff_ffff) as usize;
        if size < 8 {
            return Err(invalid(offset, "section length is less than its header"));
        }
        let end = position
            .checked_add(size)
            .filter(|&end| end <= bytes.len())
            .ok_or_else(|| invalid(offset, "section exceeds its parent"))?;
        result.push(Section {
            offset,
            signature: header[4..8].try_into().expect("four bytes"),
            flags: (size_flags >> 30) as u8,
            payload: &bytes[position + 8..end],
        });
        position = end;
    }
    Ok(result)
}

#[cfg(test)]
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
