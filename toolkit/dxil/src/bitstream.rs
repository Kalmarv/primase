//! A bounded reader of LLVM's bitstream container: blocks, abbreviations (literal, fixed, VBR, array, char6,
//! blob) and the BLOCKINFO block. It knows nothing of IR; it returns a tree of blocks and records. Every length is
//! checked against the bits that remain before anything is allocated, and nesting, abbreviations and the total
//! number of operands are capped, so a crafted file errors instead of exhausting memory.
use crate::{Result, error};
use primase_foundations::bytes::View;

/// Limits on what one bitstream may contain.
const MAX_DEPTH: usize = 32;
const MAX_ABBREVS_PER_BLOCK: usize = 4096;
const MAX_ABBREV_OPS: usize = 256;
const MAX_TOTAL_OPERANDS: usize = 1 << 26;

/// A record: its code, operands and, for blob abbreviations, the blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub code: u64,
    pub operands: Vec<u64>,
    pub blob: Option<Vec<u8>>,
    /// Bit offset of the record in the stream, for error locations.
    pub at: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Element {
    Block(Block),
    Record(Record),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub id: u64,
    pub elements: Vec<Element>,
    /// Bit offset of the block's header.
    pub at: usize,
}
impl Block {
    pub fn blocks(&self) -> impl Iterator<Item = &Block> {
        self.elements.iter().filter_map(|e| match e {
            Element::Block(b) => Some(b),
            Element::Record(_) => None,
        })
    }
    pub fn records(&self) -> impl Iterator<Item = &Record> {
        self.elements.iter().filter_map(|e| match e {
            Element::Record(r) => Some(r),
            Element::Block(_) => None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Literal(u64),
    Fixed(u32),
    Vbr(u32),
    Array,
    Char6,
    Blob,
}
type Abbrev = Vec<Op>;

struct Bits<'a> {
    data: &'a [u8],
    /// Next bit to read.
    pos: usize,
}
impl Bits<'_> {
    fn remaining(&self) -> usize {
        self.data.len() * 8 - self.pos
    }
    fn read(&mut self, width: u32) -> Result<u64> {
        if width > 64 {
            return Err(error(self.pos, "field wider than 64 bits"));
        }
        if width as usize > self.remaining() {
            return Err(error(self.pos, "bitstream ends inside a field"));
        }
        let mut value = 0u64;
        for i in 0..width as usize {
            let bit = self.pos + i;
            if self.data[bit / 8] >> (bit % 8) & 1 == 1 {
                value |= 1 << i;
            }
        }
        self.pos += width as usize;
        Ok(value)
    }
    fn vbr(&mut self, width: u32) -> Result<u64> {
        if !(2..=32).contains(&width) {
            return Err(error(self.pos, "VBR width outside 2..32"));
        }
        let at = self.pos;
        let high = 1u64 << (width - 1);
        let mut value = 0u64;
        let mut shift = 0u32;
        loop {
            let chunk = self.read(width)?;
            let payload = chunk & (high - 1);
            if payload != 0 {
                // The payload's bits must all land below bit 64.
                if shift >= 64 || (payload << shift) >> shift != payload {
                    return Err(error(at, "VBR value exceeds 64 bits"));
                }
                value |= payload << shift;
            }
            if chunk & high == 0 {
                return Ok(value);
            }
            shift += width - 1;
            if shift > 64 + 32 {
                return Err(error(at, "VBR value exceeds 64 bits"));
            }
        }
    }
    fn align32(&mut self) -> Result<()> {
        let aligned = self.pos.div_ceil(32) * 32;
        if aligned > self.data.len() * 8 {
            return Err(error(self.pos, "bitstream ends before 32-bit alignment"));
        }
        self.pos = aligned;
        Ok(())
    }
}

struct Reader<'a> {
    bits: Bits<'a>,
    /// Abbreviations BLOCKINFO defines, by block id.
    info: Vec<(u64, Vec<Abbrev>)>,
    operands: usize,
}

fn char6(v: u64) -> u64 {
    let v = v as u8;
    u64::from(match v {
        0..=25 => b'a' + v,
        26..=51 => b'A' + v - 26,
        52..=61 => b'0' + v - 52,
        62 => b'.',
        _ => b'_',
    })
}

impl Reader<'_> {
    fn count(&mut self, n: usize) -> Result<()> {
        self.operands = self
            .operands
            .checked_add(n)
            .filter(|&t| t <= MAX_TOTAL_OPERANDS)
            .ok_or_else(|| error(self.bits.pos, "too many record operands"))?;
        Ok(())
    }
    fn scalar(&mut self, op: Op) -> Result<u64> {
        match op {
            Op::Literal(v) => Ok(v),
            Op::Fixed(w) => self.bits.read(w),
            Op::Vbr(w) => self.bits.vbr(w),
            Op::Char6 => Ok(char6(self.bits.read(6)?)),
            Op::Array | Op::Blob => Err(error(self.bits.pos, "array or blob as an element")),
        }
    }
    fn define_abbrev(&mut self) -> Result<Abbrev> {
        let at = self.bits.pos;
        let n = self.bits.vbr(5)? as usize;
        if n == 0 || n > MAX_ABBREV_OPS {
            return Err(error(at, "abbreviation operand count"));
        }
        let mut ops = Vec::with_capacity(n);
        while ops.len() < n {
            if self.bits.read(1)? == 1 {
                ops.push(Op::Literal(self.bits.vbr(8)?));
                continue;
            }
            ops.push(match self.bits.read(3)? {
                1 => {
                    let w = self.bits.vbr(5)?;
                    if w > 64 {
                        return Err(error(at, "fixed width above 64"));
                    }
                    Op::Fixed(w as u32)
                }
                2 => {
                    let w = self.bits.vbr(5)?;
                    if !(2..=32).contains(&w) {
                        return Err(error(at, "VBR width outside 2..32"));
                    }
                    Op::Vbr(w as u32)
                }
                3 => Op::Array,
                4 => Op::Char6,
                5 => Op::Blob,
                other => return Err(error(at, format!("abbreviation encoding {other}"))),
            });
        }
        // An array takes the one operand after it as its element; a blob or array ends the abbreviation.
        for (i, op) in ops.iter().enumerate() {
            match op {
                Op::Array if i + 2 != ops.len() || matches!(ops[i + 1], Op::Array | Op::Blob) => {
                    return Err(error(at, "array not followed by one element encoding"));
                }
                Op::Blob if i + 1 != ops.len() => return Err(error(at, "blob not last")),
                _ => {}
            }
        }
        Ok(ops)
    }
    fn abbreviated(&mut self, abbrev: &Abbrev, at: usize) -> Result<Record> {
        let mut values: Vec<u64> = Vec::new();
        let mut blob = None;
        let mut i = 0;
        while i < abbrev.len() {
            match abbrev[i] {
                Op::Array => {
                    let element = abbrev[i + 1];
                    let len = self.bits.vbr(6)? as usize;
                    // Each element takes at least one bit unless it is a literal or zero-width field; those are
                    // capped by the operand budget instead.
                    let min_bits = match element {
                        Op::Literal(_) | Op::Fixed(0) => 0,
                        Op::Fixed(w) | Op::Vbr(w) => w as usize,
                        Op::Char6 => 6,
                        _ => 1,
                    };
                    if min_bits > 0 && len > self.bits.remaining() / min_bits {
                        return Err(error(at, "array longer than the bits that remain"));
                    }
                    self.count(len)?;
                    values.reserve(len);
                    for _ in 0..len {
                        values.push(self.scalar(element)?);
                    }
                    i += 2;
                }
                Op::Blob => {
                    let len = self.bits.vbr(6)? as usize;
                    self.bits.align32()?;
                    if len > self.bits.remaining() / 8 {
                        return Err(error(at, "blob longer than the bits that remain"));
                    }
                    let start = self.bits.pos / 8;
                    let bytes = View::new(self.bits.data)
                        .slice(start, len)
                        .map_err(|e| error(at, e.to_string()))?;
                    blob = Some(bytes.to_vec());
                    self.bits.pos += len * 8;
                    self.bits.align32()?;
                    i += 1;
                }
                op => {
                    self.count(1)?;
                    values.push(self.scalar(op)?);
                    i += 1;
                }
            }
        }
        if values.is_empty() {
            return Err(error(at, "abbreviated record without a code"));
        }
        let code = values.remove(0);
        Ok(Record {
            code,
            operands: values,
            blob,
            at,
        })
    }
    /// Read a block's body after its header, up to and including END_BLOCK.
    fn block(&mut self, id: u64, width: u32, end: usize, depth: usize, at: usize) -> Result<Block> {
        if depth > MAX_DEPTH {
            return Err(error(at, "blocks nested too deeply"));
        }
        let mut abbrevs: Vec<Abbrev> = self
            .info
            .iter()
            .find(|(b, _)| *b == id)
            .map(|(_, a)| a.clone())
            .unwrap_or_default();
        let mut elements = Vec::new();
        // In BLOCKINFO, the block id that following abbreviations apply to.
        let mut info_target: Option<u64> = None;
        loop {
            if self.bits.pos >= end {
                return Err(error(at, "block ends without END_BLOCK"));
            }
            let here = self.bits.pos;
            match self.bits.read(width)? {
                0 => {
                    self.bits.align32()?;
                    if self.bits.pos != end {
                        return Err(error(here, "END_BLOCK before the declared block length"));
                    }
                    return Ok(Block { id, elements, at });
                }
                1 => {
                    let child = self.bits.vbr(8)?;
                    let child_width = self.bits.vbr(4)? as u32;
                    if !(1..=32).contains(&child_width) {
                        return Err(error(here, "abbreviation width outside 1..32"));
                    }
                    self.bits.align32()?;
                    let words = self.bits.read(32)? as usize;
                    let child_end = words
                        .checked_mul(32)
                        .and_then(|b| b.checked_add(self.bits.pos))
                        .filter(|&e| e <= end)
                        .ok_or_else(|| error(here, "block longer than its parent"))?;
                    let b = self.block(child, child_width, child_end, depth + 1, here)?;
                    elements.push(Element::Block(b));
                }
                2 => {
                    let abbrev = self.define_abbrev()?;
                    if id == 0 {
                        let target =
                            info_target.ok_or_else(|| error(here, "BLOCKINFO abbreviation before SETBID"))?;
                        let entry = match self.info.iter().position(|(b, _)| *b == target) {
                            Some(i) => &mut self.info[i].1,
                            None => {
                                self.info.push((target, Vec::new()));
                                &mut self.info.last_mut().expect("pushed").1
                            }
                        };
                        if entry.len() >= MAX_ABBREVS_PER_BLOCK {
                            return Err(error(here, "too many abbreviations"));
                        }
                        entry.push(abbrev);
                    } else {
                        if abbrevs.len() >= MAX_ABBREVS_PER_BLOCK {
                            return Err(error(here, "too many abbreviations"));
                        }
                        abbrevs.push(abbrev);
                    }
                }
                3 => {
                    let code = self.bits.vbr(6)?;
                    let n = self.bits.vbr(6)? as usize;
                    if n > self.bits.remaining() / 2 {
                        return Err(error(here, "record longer than the bits that remain"));
                    }
                    self.count(n)?;
                    let mut operands = Vec::with_capacity(n);
                    for _ in 0..n {
                        operands.push(self.bits.vbr(6)?);
                    }
                    if id == 0 && code == 1 {
                        info_target = Some(
                            *operands
                                .first()
                                .ok_or_else(|| error(here, "SETBID without a block id"))?,
                        );
                    }
                    elements.push(Element::Record(Record {
                        code,
                        operands,
                        blob: None,
                        at: here,
                    }));
                }
                n => {
                    let abbrev = abbrevs
                        .get(n as usize - 4)
                        .cloned()
                        .ok_or_else(|| error(here, format!("undefined abbreviation {n}")))?;
                    let record = self.abbreviated(&abbrev, here)?;
                    if id == 0 && record.code == 1 {
                        info_target = record.operands.first().copied();
                    }
                    elements.push(Element::Record(record));
                }
            }
        }
    }
}

/// Read a bitstream whose 32-bit magic has been checked: the top-level blocks (abbreviation width 2).
pub fn read(data: &[u8], magic: [u8; 4]) -> Result<Vec<Block>> {
    if data.get(..4) != Some(&magic[..]) || !data.len().is_multiple_of(4) {
        return Err(error(0, "bitstream magic or length"));
    }
    let mut r = Reader {
        bits: Bits { data, pos: 32 },
        info: Vec::new(),
        operands: 0,
    };
    let end = data.len() * 8;
    let mut blocks = Vec::new();
    while r.bits.pos < end {
        let here = r.bits.pos;
        // Trailing zero padding after the last block is allowed.
        if data[here / 8..].iter().all(|&b| b == 0) {
            break;
        }
        if r.bits.read(2)? != 1 {
            return Err(error(here, "top level holds only blocks"));
        }
        let id = r.bits.vbr(8)?;
        let width = r.bits.vbr(4)? as u32;
        if !(1..=32).contains(&width) {
            return Err(error(here, "abbreviation width outside 1..32"));
        }
        r.bits.align32()?;
        let words = r.bits.read(32)? as usize;
        let block_end = words
            .checked_mul(32)
            .and_then(|b| b.checked_add(r.bits.pos))
            .filter(|&e| e <= end)
            .ok_or_else(|| error(here, "block longer than the stream"))?;
        blocks.push(r.block(id, width, block_end, 1, here)?);
    }
    Ok(blocks)
}
