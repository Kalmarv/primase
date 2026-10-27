//! A bounds-checked cursor over the bytes of a SerializedFile or of a script's serialized fields.

use crate::{Result, invalid};

/// A bounds-checked little- or big-endian reader over a byte slice.
#[derive(Clone, Debug)]
pub struct Reader<'a> {
    pub data: &'a [u8],
    pub pos: usize,
    pub big_endian: bool,
}

macro_rules! read_num {
    ($($f:ident: $t:ty),*) => {$(
        pub fn $f(&mut self) -> Result<$t> {
            let b = self.take(std::mem::size_of::<$t>())?;
            let a = b.try_into().expect("sized");
            Ok(if self.big_endian { <$t>::from_be_bytes(a) } else { <$t>::from_le_bytes(a) })
        }
    )*};
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], big_endian: bool) -> Self {
        Reader {
            data,
            pos: 0,
            big_endian,
        }
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            return Err(invalid(format!(
                "{n} bytes wanted at offset {}, {} left",
                self.pos,
                self.remaining()
            )));
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn bool(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            b => Err(invalid(format!("bool byte {b} at offset {}", self.pos - 1))),
        }
    }

    read_num!(i8: i8, i16: i16, u16: u16, i32: i32, u32: u32, i64: i64, u64: u64, f32: f32, f64: f64);

    /// Align the position to `n` bytes (from the start of the data).
    pub fn align(&mut self, n: usize) -> Result<()> {
        let pad = (n - self.pos % n) % n;
        self.take(pad).map(|_| ())
    }

    /// A NUL-terminated UTF-8 string.
    pub fn cstr(&mut self) -> Result<String> {
        let rest = &self.data[self.pos..];
        let n = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| invalid(format!("unterminated string at offset {}", self.pos)))?;
        let s = std::str::from_utf8(&rest[..n])
            .map_err(|_| invalid(format!("string at offset {} is not UTF-8", self.pos)))?
            .to_owned();
        self.pos += n + 1;
        Ok(s)
    }
}
