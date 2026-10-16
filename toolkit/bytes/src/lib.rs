//! Borrowed byte bounds; format, endian, allocation and count policies stay with callers.
use std::{fmt, ops::Range};

/// Local byte operands and their source origin, retained without lossy arithmetic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundsError {
    LengthOverflow {
        offset: usize,
        origin: usize,
        count: usize,
        stride: usize,
    },
    OutOfBounds {
        offset: usize,
        origin: usize,
        length: usize,
        total: usize,
    },
}
impl BoundsError {
    fn with_origin(self, origin: usize) -> Self {
        match self {
            Self::LengthOverflow {
                offset,
                count,
                stride,
                ..
            } => Self::LengthOverflow {
                offset,
                origin,
                count,
                stride,
            },
            Self::OutOfBounds {
                offset,
                length,
                total,
                ..
            } => Self::OutOfBounds {
                offset,
                origin,
                length,
                total,
            },
        }
    }
    /// Exact absolute byte offset, including an origin beyond the local view.
    pub fn absolute(&self) -> u128 {
        let (offset, origin) = match *self {
            Self::LengthOverflow { offset, origin, .. } | Self::OutOfBounds { offset, origin, .. } => {
                (offset, origin)
            }
        };
        offset as u128 + origin as u128
    }
}
impl fmt::Display for BoundsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let offset = self.absolute();
        match self {
            Self::LengthOverflow { count, stride, .. } => write!(
                f,
                "byte span at {offset:#x}: {count} entries of {stride} bytes overflow"
            ),
            Self::OutOfBounds { length, total, .. } => write!(
                f,
                "byte span at {offset:#x}: {length} bytes outside a {total}-byte view"
            ),
        }
    }
}
impl std::error::Error for BoundsError {}

/// Check multiplication, addition, and bounds in that order. A zero stride does
/// not authorize allocating or processing `count` records; that is parser policy.
pub fn checked_span(
    offset: usize,
    count: usize,
    stride: usize,
    total: usize,
) -> Result<Range<usize>, BoundsError> {
    let overflow = || BoundsError::LengthOverflow {
        offset,
        origin: 0,
        count,
        stride,
    };
    let length = count.checked_mul(stride).ok_or_else(overflow)?;
    let end = offset.checked_add(length).ok_or_else(overflow)?;
    if end > total {
        return Err(BoundsError::OutOfBounds {
            offset,
            origin: 0,
            length,
            total,
        });
    }
    Ok(offset..end)
}

#[derive(Clone, Copy, Debug)]
pub struct View<'a> {
    data: &'a [u8],
    origin: usize,
}
impl<'a> View<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self::with_origin(data, 0)
    }
    /// `origin` is the source byte offset of `data[0]`; slicing offsets remain local.
    pub fn with_origin(data: &'a [u8], origin: usize) -> Self {
        Self { data, origin }
    }
    pub fn slice(&self, offset: usize, length: usize) -> Result<&'a [u8], BoundsError> {
        let span =
            checked_span(offset, length, 1, self.data.len()).map_err(|e| e.with_origin(self.origin))?;
        Ok(&self.data[span])
    }
    pub fn array<const N: usize>(&self, offset: usize) -> Result<[u8; N], BoundsError> {
        let mut bytes = [0; N];
        bytes.copy_from_slice(self.slice(offset, N)?);
        Ok(bytes)
    }
}

/// A cursor advances only after a successful checked read or seek.
#[derive(Clone, Debug)]
pub struct Cursor<'a> {
    view: View<'a>,
    position: usize,
}
impl<'a> Cursor<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self::with_origin(data, 0)
    }
    pub fn with_origin(data: &'a [u8], origin: usize) -> Self {
        Self {
            view: View::with_origin(data, origin),
            position: 0,
        }
    }
    pub fn position(&self) -> usize {
        self.position
    }
    pub fn remaining(&self) -> usize {
        self.view.data.len() - self.position
    }
    pub fn take(&mut self, length: usize) -> Result<&'a [u8], BoundsError> {
        let bytes = self.view.slice(self.position, length)?;
        self.position += length;
        Ok(bytes)
    }
    pub fn seek(&mut self, offset: usize) -> Result<(), BoundsError> {
        self.view.slice(offset, 0)?;
        self.position = offset;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spans_and_arithmetic_boundaries() {
        assert_eq!(checked_span(0, 0, 1, 0), Ok(0..0));
        assert_eq!(checked_span(4, 0, 7, 4), Ok(4..4));
        assert!(matches!(
            checked_span(5, 0, 1, 4),
            Err(BoundsError::OutOfBounds { .. })
        ));
        assert_eq!(checked_span(1, usize::MAX, 0, 1), Ok(1..1));
        assert!(matches!(
            checked_span(0, usize::MAX, 2, usize::MAX),
            Err(BoundsError::LengthOverflow { .. })
        ));
        assert!(matches!(
            checked_span(usize::MAX, 1, 1, usize::MAX),
            Err(BoundsError::LengthOverflow { .. })
        ));
        assert_eq!(View::new(&[1, 2, 3]).array::<2>(1), Ok([2, 3]));
        assert!(View::new(&[]).array::<1>(0).is_err());
    }
    #[test]
    fn origins_are_exact_even_past_address_space() {
        let view = View::with_origin(&[], usize::MAX);
        assert_eq!(view.slice(1, 0).unwrap_err().absolute(), usize::MAX as u128 + 1);
        assert_eq!(
            view.slice(1, 0),
            Err(BoundsError::OutOfBounds {
                offset: 1,
                origin: usize::MAX,
                length: 0,
                total: 0,
            })
        );
        assert_eq!(
            view.slice(usize::MAX, 1),
            Err(BoundsError::LengthOverflow {
                offset: usize::MAX,
                origin: usize::MAX,
                count: 1,
                stride: 1,
            })
        );
    }
    #[test]
    fn failed_cursor_operations_do_not_advance() {
        let mut cursor = Cursor::with_origin(&[1, 2, 3], 10);
        assert_eq!(cursor.take(2), Ok(&[1, 2][..]));
        assert!(cursor.take(2).is_err());
        assert!(cursor.take(usize::MAX).is_err());
        assert!(cursor.seek(4).is_err());
        assert_eq!(cursor.position(), 2);
        assert_eq!(cursor.remaining(), 1);
        assert_eq!(cursor.take(1), Ok(&[3][..]));
        assert_eq!(cursor.take(0), Ok(&[][..]));
        assert_eq!(cursor.seek(0), Ok(()));
        assert_eq!(cursor.remaining(), 3);
    }
}
