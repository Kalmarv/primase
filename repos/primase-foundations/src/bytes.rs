//! Borrowed byte bounds and the fixed-size numbers read from them. Format, allocation and count policies stay with
//! callers, and so does the byte order: every multi-byte read states its [`Endian`].
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

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

/// A read of a named field that failed: the field, and where its bytes would be ([`Field::field`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldError {
    pub what: &'static str,
    pub bounds: BoundsError,
}
impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (what, at) = (self.what, self.bounds.absolute());
        match self.bounds {
            BoundsError::LengthOverflow { count, stride, .. } => {
                write!(f, "{what} at {at:#x}: {count} entries of {stride} bytes overflow")
            }
            BoundsError::OutOfBounds { length, total, .. } => {
                write!(f, "{what} at {at:#x}: {length} bytes run past the end of {total}")
            }
        }
    }
}
impl std::error::Error for FieldError {}

/// Names the field a failed read was of.
pub trait Field<T> {
    fn field(self, what: &'static str) -> Result<T, FieldError>;
}
impl<T> Field<T> for Result<T, BoundsError> {
    fn field(self, what: &'static str) -> Result<T, FieldError> {
        self.map_err(|bounds| FieldError { what, bounds })
    }
}

/// The byte order of a multi-byte number: the format's, stated at each read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endian {
    Little,
    Big,
}

/// The fixed-size numbers [`View`] and [`Cursor`] read, in either byte order.
macro_rules! numbers {
    ($($name:ident: $t:ty),* $(,)?) => {
        impl View<'_> {
            $(
                #[doc = concat!("The `", stringify!($t), "` at `offset`, in byte order `e`.")]
                pub fn $name(&self, offset: usize, e: Endian) -> Result<$t, BoundsError> {
                    let b = self.array(offset)?;
                    Ok(match e {
                        Endian::Little => <$t>::from_le_bytes(b),
                        Endian::Big => <$t>::from_be_bytes(b),
                    })
                }
            )*
        }
        impl Cursor<'_> {
            $(
                #[doc = concat!("The next `", stringify!($t), "`, in byte order `e`.")]
                pub fn $name(&mut self, e: Endian) -> Result<$t, BoundsError> {
                    let b = self.array()?;
                    Ok(match e {
                        Endian::Little => <$t>::from_le_bytes(b),
                        Endian::Big => <$t>::from_be_bytes(b),
                    })
                }
            )*
        }
    };
}
numbers!(u16: u16, i16: i16, u32: u32, i32: i32, u64: u64, i64: i64, f32: f32, f64: f64);

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
    /// The byte at `offset`.
    pub fn u8(&self, offset: usize) -> Result<u8, BoundsError> {
        self.array::<1>(offset).map(|[b]| b)
    }
    /// The byte at `offset`, signed.
    pub fn i8(&self, offset: usize) -> Result<i8, BoundsError> {
        self.array::<1>(offset).map(i8::from_le_bytes)
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
    /// All the bytes it reads, from its start (a parser that looks back or ahead by offset).
    pub fn bytes(&self) -> &'a [u8] {
        self.view.data
    }
    pub fn remaining(&self) -> usize {
        self.view.data.len() - self.position
    }
    pub fn take(&mut self, length: usize) -> Result<&'a [u8], BoundsError> {
        let bytes = self.view.slice(self.position, length)?;
        self.position += length;
        Ok(bytes)
    }
    /// The bytes from the position up to `end` (an offset in the view), the cursor left at `end`: refused when `end`
    /// is before the position or past the view.
    pub fn take_to(&mut self, end: usize) -> Result<&'a [u8], BoundsError> {
        let within = View::with_origin(self.view.slice(0, end)?, self.view.origin);
        let bytes = within.slice(self.position, end.saturating_sub(self.position))?;
        self.position = end;
        Ok(bytes)
    }
    pub fn seek(&mut self, offset: usize) -> Result<(), BoundsError> {
        self.view.slice(offset, 0)?;
        self.position = offset;
        Ok(())
    }
    /// The next `N` bytes.
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], BoundsError> {
        let bytes = self.view.array(self.position)?;
        self.position += N;
        Ok(bytes)
    }
    /// The next byte.
    pub fn u8(&mut self) -> Result<u8, BoundsError> {
        self.array::<1>().map(|[b]| b)
    }
    /// The next byte, signed.
    pub fn i8(&mut self) -> Result<i8, BoundsError> {
        self.array::<1>().map(i8::from_le_bytes)
    }
    /// A `u32` count, in byte order `e`, of records of `each` bytes that the rest of the view holds; a count it
    /// can't hold is refused, its span's error naming where the records would start, and the cursor stays before the
    /// count. A zero `each` bounds nothing (`checked_span`).
    pub fn count(&mut self, e: Endian, each: usize) -> Result<usize, BoundsError> {
        let start = self.position;
        let n = self.u32(e)?;
        // past usize (a 16-bit target), the count is beyond any view: the largest is refused the same way
        let n = usize::try_from(n).unwrap_or(usize::MAX);
        match checked_span(self.position, n, each, self.view.data.len()) {
            Ok(_) => Ok(n),
            Err(err) => {
                self.position = start;
                Err(err.with_origin(self.view.origin))
            }
        }
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
    /// Numbers read in the stated byte order, positionally and in sequence; a failed read leaves the cursor where it
    /// was; a field's error names it and its exact offset.
    #[test]
    fn numbers_in_either_order_and_named_fields() {
        let bytes = [0x01, 0x02, 0x03, 0x04, 0x00, 0x00, 0x80, 0x3f];
        let view = View::with_origin(&bytes, 0x100);
        assert_eq!(view.u16(0, Endian::Little), Ok(0x0201));
        assert_eq!(view.u16(0, Endian::Big), Ok(0x0102));
        assert_eq!(view.u32(0, Endian::Little), Ok(0x0403_0201));
        assert_eq!(view.i8(2), Ok(3));
        assert_eq!(view.f32(4, Endian::Little), Ok(1.0));
        assert_eq!(view.u64(0, Endian::Big), Ok(0x0102_0304_0000_803f));
        assert!(view.u32(6, Endian::Little).is_err());
        let mut c = Cursor::with_origin(&bytes, 0x100);
        assert_eq!(c.u8(), Ok(1));
        assert_eq!(c.u16(Endian::Big), Ok(0x0203));
        assert_eq!(c.position(), 3);
        let err = c.u64(Endian::Little).field("a header's size").unwrap_err();
        assert_eq!(c.position(), 3);
        assert_eq!(
            err.to_string(),
            "a header's size at 0x103: 8 bytes run past the end of 8"
        );
        assert_eq!(c.array::<5>(), Ok([0x04, 0x00, 0x00, 0x80, 0x3f]));
        // up to an end: not before the position, not past the view
        let mut c = Cursor::new(&bytes);
        c.seek(2).unwrap();
        assert_eq!(c.take_to(4), Ok(&[0x03, 0x04][..]));
        assert_eq!(c.position(), 4);
        assert!(c.take_to(3).is_err());
        assert!(c.take_to(9).is_err());
        assert_eq!(c.position(), 4);
    }

    /// A count the rest of the view holds is read; one it can't (or whose bytes overflow) is refused without
    /// advancing, naming where its records would start.
    #[test]
    fn counts_are_held_to_the_rest_of_the_view() {
        // a count of 2 records of 2 bytes, then 4 bytes
        let bytes = [2, 0, 0, 0, 9, 9, 9, 9];
        let mut c = Cursor::new(&bytes);
        assert_eq!(c.count(Endian::Little, 2), Ok(2));
        assert_eq!(c.position(), 4);
        let mut c = Cursor::with_origin(&bytes, 0x10);
        assert_eq!(
            c.count(Endian::Little, 3),
            Err(BoundsError::OutOfBounds {
                offset: 4,
                origin: 0x10,
                length: 6,
                total: 8,
            })
        );
        assert_eq!(c.position(), 0);
        let huge = [0xff, 0xff, 0xff, 0xff];
        assert!(matches!(
            Cursor::new(&huge).count(Endian::Little, usize::MAX),
            Err(BoundsError::LengthOverflow { .. })
        ));
        assert!(Cursor::new(&huge[..2]).count(Endian::Little, 1).is_err());
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
