use crate::{Result, invalid};
pub(crate) struct View<'a> {
    pub data: &'a [u8],
    pub format: &'static str,
    pub be: bool,
}
impl<'a> View<'a> {
    pub fn slice(&self, o: usize, n: usize) -> Result<&'a [u8]> {
        primase_bytes::View::new(self.data).slice(o, n).map_err(|e| {
            let detail = match e {
                primase_bytes::BoundsError::LengthOverflow { .. } => "size overflow",
                primase_bytes::BoundsError::OutOfBounds { .. } => "short input",
            };
            invalid(self.format, o, detail)
        })
    }
    pub fn u8(&self, o: usize) -> Result<u8> {
        Ok(self.slice(o, 1)?[0])
    }
    pub fn u16(&self, o: usize) -> Result<u16> {
        let b = self.slice(o, 2)?;
        Ok(if self.be {
            u16::from_be_bytes([b[0], b[1]])
        } else {
            u16::from_le_bytes([b[0], b[1]])
        })
    }
    pub fn u32(&self, o: usize) -> Result<u32> {
        let b: [u8; 4] = self
            .slice(o, 4)?
            .try_into()
            .map_err(|_| invalid(self.format, o, "u32"))?;
        Ok(if self.be {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    }
    pub fn u64(&self, o: usize) -> Result<u64> {
        let b: [u8; 8] = self
            .slice(o, 8)?
            .try_into()
            .map_err(|_| invalid(self.format, o, "u64"))?;
        Ok(if self.be {
            u64::from_be_bytes(b)
        } else {
            u64::from_le_bytes(b)
        })
    }
    pub fn size(&self, o: usize) -> Result<usize> {
        usize::try_from(self.u64(o)?).map_err(|_| invalid(self.format, o, "size overflow"))
    }
    pub fn expect(&self, o: usize, b: &[u8]) -> Result<()> {
        if self.slice(o, b.len())? != b {
            Err(invalid(self.format, o, "unexpected field"))
        } else {
            Ok(())
        }
    }
    pub fn boolean(&self, o: usize) -> Result<bool> {
        match self.u8(o)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(invalid(self.format, o, "invalid boolean")),
        }
    }
}
pub(crate) fn coverage(
    v: &View<'_>,
    mut spans: Vec<(usize, usize)>,
    end: usize,
    padding: usize,
) -> Result<()> {
    spans.sort_unstable();
    spans.dedup();
    let mut pos = 0;
    for (start, len) in spans {
        v.slice(start, len)?;
        if start < pos {
            return Err(invalid(v.format, start, "overlapping sections"));
        }
        if start - pos > padding || v.slice(pos, start - pos)?.iter().any(|&b| b != 0) {
            return Err(invalid(v.format, pos, "unaccounted bytes"));
        }
        pos = start
            .checked_add(len)
            .ok_or_else(|| invalid(v.format, start, "size overflow"))?;
    }
    if pos > end || end - pos > padding || v.slice(pos, end - pos)?.iter().any(|&b| b != 0) {
        return Err(invalid(v.format, pos, "unaccounted trailing bytes"));
    }
    Ok(())
}

#[cfg(test)]
mod bounds_tests {
    use super::*;
    #[test]
    fn wrapper_retains_bounds_and_error_context() {
        let view = View {
            data: &[1, 2, 3, 4],
            format: "bounds fixture",
            be: false,
        };
        assert!(view.slice(4, 0).unwrap().is_empty());
        assert_eq!(view.u32(0).unwrap(), 0x04030201);
        for (offset, length, detail) in [
            (4, 1, "short input"),
            (5, 0, "short input"),
            (usize::MAX, 1, "size overflow"),
        ] {
            assert_eq!(
                view.slice(offset, length).unwrap_err().to_string(),
                invalid(view.format, offset, detail).to_string()
            );
        }
    }
}
