//! MTD material definitions, with bounded nested blocks and strict Shift-JIS.
use primase_foundations::bytes::Endian;

use crate::{Result, invalid};

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub enum Value {
    Bool(bool),
    Int(Vec<i32>),
    Float(Vec<f32>),
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Parameter {
    pub name: String,
    pub value: Value,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Texture {
    pub kind: String,
    pub uv_number: i32,
    pub shader_data_index: i32,
    pub path: String,
    pub extra: Vec<f32>,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct BlockLength {
    pub start: usize,
    pub kind: u32,
    pub declared: usize,
    pub consumed: usize,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct MaterialDefinition {
    pub shader: String,
    pub description: String,
    pub parameters: Vec<Parameter>,
    pub textures: Vec<Texture>,
    pub block_lengths: Vec<BlockLength>,
}
struct Cursor<'a> {
    /// The file, read in order (little-endian).
    c: primase_foundations::bytes::Cursor<'a>,
    stack: Vec<(usize, usize, u32)>,
    blocks: Vec<BlockLength>,
}
impl Cursor<'_> {
    fn bytes(&mut self, n: usize) -> Result<&[u8]> {
        let at = self.c.position();
        self.c
            .take(n)
            .map_err(|_| invalid("MTD", at, "read beyond block"))
    }
    fn u32(&mut self) -> Result<u32> {
        let at = self.c.position();
        self.c
            .u32(Endian::Little)
            .map_err(|_| invalid("MTD", at, "read beyond block"))
    }
    fn expect(&mut self, n: u32) -> Result<()> {
        let at = self.c.position();
        if self.u32()? != n {
            return Err(invalid("MTD", at, "unexpected field"));
        }
        Ok(())
    }
    fn count(&mut self, stride: usize) -> Result<usize> {
        let at = self.c.position();
        self.c
            .count(Endian::Little, stride)
            .map_err(|_| invalid("MTD", at, "count exceeds block"))
    }
    fn marker(&mut self, expected: u8) -> Result<()> {
        let at = self.c.position();
        if self.bytes(1)?[0] != expected {
            return Err(invalid("MTD", at, "marker"));
        }
        let padding = (4 - self.c.position() % 4) % 4;
        if self.bytes(padding)?.iter().any(|b| *b != 0) {
            return Err(invalid("MTD", at, "nonzero alignment"));
        }
        Ok(())
    }
    fn string(&mut self, marker: u8) -> Result<String> {
        let n = self.count(1)?;
        let at = self.c.position();
        let s = encoding_rs::SHIFT_JIS
            .decode_without_bom_handling_and_without_replacement(self.bytes(n)?)
            .ok_or_else(|| invalid("MTD", at, "malformed Shift-JIS string"))?
            .into_owned();
        self.marker(marker)?;
        Ok(s)
    }
    fn block(&mut self, kind: u32, versions: &[u32], marker: u8) -> Result<u32> {
        self.expect(0)?;
        let len = self.u32()? as usize;
        self.stack.push((self.c.position(), len, kind));
        self.expect(kind)?;
        let version = self.u32()?;
        if !versions.contains(&version) {
            return Err(invalid("MTD", self.c.position() - 4, "block version"));
        }
        self.marker(marker)?;
        Ok(version)
    }
    fn end(&mut self) -> Result<()> {
        let (start, declared, kind) = self
            .stack
            .pop()
            .ok_or_else(|| invalid("MTD", self.c.position(), "block nesting"))?;
        self.blocks.push(BlockLength {
            start,
            kind,
            declared,
            consumed: self.c.position() - start,
        });
        Ok(())
    }
    fn float(&mut self) -> Result<f32> {
        let at = self.c.position();
        let x = f32::from_bits(self.u32()?);
        if !x.is_finite() {
            return Err(invalid("MTD", at, "non-finite parameter"));
        }
        Ok(x)
    }
}
impl MaterialDefinition {
    /// Structure is authoritative; declared block lengths are retained as diagnostics.
    /// Stock files can overstate these lengths. No consumer control-flow claim is
    /// made: typed counts/markers determine boundaries, always within the file.
    pub fn read(data: &[u8]) -> Result<Self> {
        let mut c = Cursor {
            c: primase_foundations::bytes::Cursor::new(data),
            stack: vec![],
            blocks: vec![],
        };
        c.block(0, &[3], 1)?;
        c.block(1, &[2], 0xb0)?;
        if c.string(0x34)? != "MTD " {
            return Err(invalid("MTD", c.c.position(), "magic"));
        }
        c.expect(1000)?;
        c.end()?;
        c.marker(1)?;
        c.block(2, &[4], 0xa3)?;
        let shader = c.string(0xa3)?;
        let description = c.string(3)?;
        c.expect(1)?;
        c.block(3, &[4], 0xa3)?;
        c.expect(0)?;
        c.marker(3)?;
        let mut parameters = Vec::new();
        for _ in 0..c.count(20)? {
            c.block(4, &[4], 0xa3)?;
            let name = c.string(0xa3)?;
            let kind = c.string(4)?;
            c.expect(1)?;
            let (block, marker, n) = match kind.as_str() {
                "bool" => (0x1000, 0xc0, 1),
                "int" => (0x1001, 0xc5, 1),
                "int2" => (0x1001, 0xc5, 2),
                "float" => (0x1002, 0xca, 1),
                "float2" => (0x1002, 0xca, 2),
                "float3" => (0x1002, 0xca, 3),
                "float4" => (0x1002, 0xca, 4),
                _ => return Err(invalid("MTD", c.c.position(), format!("parameter type {kind}"))),
            };
            c.block(block, &[1], marker)?;
            c.expect(n)?;
            let value = if block == 0x1000 {
                match c.bytes(1)?[0] {
                    0 => Value::Bool(false),
                    1 => Value::Bool(true),
                    _ => return Err(invalid("MTD", c.c.position() - 1, "boolean")),
                }
            } else if block == 0x1001 {
                Value::Int((0..n).map(|_| c.u32().map(|x| x as i32)).collect::<Result<_>>()?)
            } else {
                Value::Float((0..n).map(|_| c.float()).collect::<Result<_>>()?)
            };
            c.end()?;
            c.marker(4)?;
            c.expect(0)?;
            c.end()?;
            parameters.push(Parameter { name, value });
        }
        c.marker(3)?;
        let mut textures = Vec::new();
        for _ in 0..c.count(20)? {
            let version = c.block(0x2000, &[3, 5], 0xa3)?;
            let kind = c.string(0x35)?;
            let uv_number = c.u32()? as i32;
            c.marker(0x35)?;
            let shader_data_index = c.u32()? as i32;
            let (path, extra) = if version == 5 {
                c.expect(0xa3)?;
                let path = c.string(0xba)?;
                let n = c.count(4)?;
                (path, (0..n).map(|_| c.float()).collect::<Result<Vec<_>>>()?)
            } else {
                (String::new(), vec![])
            };
            c.end()?;
            textures.push(Texture {
                kind,
                uv_number,
                shader_data_index,
                path,
                extra,
            });
        }
        c.marker(4)?;
        c.expect(0)?;
        c.end()?;
        c.marker(4)?;
        c.expect(0)?;
        c.end()?;
        c.marker(4)?;
        c.expect(0)?;
        c.end()?;
        if c.c.position() != data.len() {
            return Err(invalid("MTD", c.c.position(), "trailing bytes"));
        }
        Ok(Self {
            shader,
            description,
            parameters,
            textures,
            block_lengths: c.blocks,
        })
    }
}
