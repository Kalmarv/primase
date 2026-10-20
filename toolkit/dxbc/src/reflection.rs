//! SM5 RDEF and signature metadata; no material or binding policy.
use crate::{Result, chunks, error};
use primase_shader::{Buffer, Member, Reflection, Resource, Semantic, Type, Variable};

struct View<'a>(&'a [u8]);
impl View<'_> {
    fn bytes(&self, at: usize, len: usize) -> Result<&[u8]> {
        let end = at
            .checked_add(len)
            .ok_or_else(|| error(at / 4, "reflection range overflow"))?;
        self.0
            .get(at..end)
            .ok_or_else(|| error(at / 4, "reflection range exceeds chunk"))
    }
    fn u32(&self, at: usize) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.bytes(at, 4)?.try_into().expect("four bytes"),
        ))
    }
    fn u16(&self, at: usize) -> Result<u16> {
        Ok(u16::from_le_bytes(
            self.bytes(at, 2)?.try_into().expect("two bytes"),
        ))
    }
    fn string(&self, at: usize) -> Result<String> {
        let tail = self
            .0
            .get(at..)
            .ok_or_else(|| error(at / 4, "invalid string offset"))?;
        let end = tail
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| error(at / 4, "unterminated reflection string"))?;
        if end > 4096 {
            return Err(error(at / 4, "reflection string exceeds bound"));
        }
        Ok(std::str::from_utf8(&tail[..end])
            .map_err(|_| error(at / 4, "non-UTF8 reflection string"))?
            .to_owned())
    }
    fn table(&self, at: usize, count: usize, stride: usize) -> Result<std::ops::Range<usize>> {
        if count > 65536 {
            return Err(error(at / 4, "reflection table exceeds count bound"));
        }
        self.bytes(
            at,
            count
                .checked_mul(stride)
                .ok_or_else(|| error(at / 4, "table overflow"))?,
        )?;
        Ok(0..count)
    }
    fn ty(&self, at: usize, stack: &mut Vec<usize>, budget: &mut usize) -> Result<Type> {
        if stack.len() >= 32 || stack.contains(&at) || *budget == 0 {
            return Err(error(at / 4, "cyclic or excessive reflection types"));
        }
        *budget -= 1;
        self.bytes(at, 36)?;
        stack.push(at);
        let count = self.u16(at + 10)? as usize;
        let offset = self.u32(at + 12)? as usize;
        let mut members = Vec::new();
        for i in self.table(offset, count, 12)? {
            let p = offset + i * 12;
            members.push(Member {
                name: self.string(self.u32(p)? as usize)?,
                offset: self.u32(p + 8)?,
                ty: self.ty(self.u32(p + 4)? as usize, stack, budget)?,
            });
        }
        stack.pop();
        Ok(Type {
            class: self.u16(at)?,
            scalar: self.u16(at + 2)?,
            rows: self.u16(at + 4)?,
            columns: self.u16(at + 6)?,
            elements: self.u16(at + 8)?,
            members,
        })
    }
}
fn signature(bytes: &[u8]) -> Result<Vec<Semantic>> {
    let v = View(bytes);
    let count = v.u32(0)? as usize;
    if v.u32(4)? != 8 {
        return Err(error(1, "unsupported signature directory offset"));
    }
    let mut result = Vec::new();
    for i in v.table(8, count, 24)? {
        let p = 8 + i * 24;
        let lanes = v.bytes(p + 20, 4)?;
        if lanes[0] & !15 != 0 || lanes[1] & !15 != 0 {
            return Err(error(p / 4, "invalid signature lane mask"));
        }
        result.push(Semantic {
            name: v.string(v.u32(p)? as usize)?,
            index: v.u32(p + 4)?,
            system_value: v.u32(p + 8)?,
            component_type: v.u32(p + 12)?,
            register: v.u32(p + 16)?,
            mask: lanes[0],
            used_mask: lanes[1],
        });
    }
    Ok(result)
}
/// A container's one chunk of kind `tag`.
fn chunk<'a>(directory: &[crate::Chunk<'a>], tag: &[u8; 4]) -> Result<&'a [u8]> {
    let mut matching = directory.iter().filter(|c| &c.kind == tag);
    let data = matching
        .next()
        .ok_or_else(|| {
            error(
                0,
                format!("missing reflection chunk {}", String::from_utf8_lossy(tag)),
            )
        })?
        .data;
    if matching.next().is_some() {
        return Err(error(0, "duplicate reflection chunk"));
    }
    Ok(data)
}

/// Read a container's ISGN/OSGN signatures alone, for a container whose RDEF was stripped (as some engines ship
/// them): no buffers or resources, so the caller states the constant layout from its own records.
pub fn signatures(bytes: &[u8]) -> Result<Reflection> {
    let directory = chunks(bytes)?;
    Ok(Reflection {
        buffers: Vec::new(),
        resources: Vec::new(),
        inputs: signature(chunk(&directory, b"ISGN")?)?,
        outputs: signature(chunk(&directory, b"OSGN")?)?,
    })
}

/// Read a container's RDEF resource and constant-buffer metadata and its ISGN/OSGN signatures.
pub fn reflection(bytes: &[u8]) -> Result<Reflection> {
    {
        let directory = chunks(bytes)?;
        let get = |tag: &[u8; 4]| chunk(&directory, tag);
        let v = View(get(b"RDEF")?);
        // SM5.0 pixel shader and RD11 record-size directory.
        if v.u32(16)? != 0xffff0500 || v.bytes(28, 4)? != b"RD11" {
            return Err(error(4, "expected SM5.0 pixel RDEF"));
        }
        for (i, size) in [60, 24, 32, 40, 36, 12].into_iter().enumerate() {
            if v.u32(32 + i * 4)? != size {
                return Err(error(8 + i, "unsupported RDEF record size"));
            }
        }
        let mut resources = Vec::new();
        let offset = v.u32(12)? as usize;
        for i in v.table(offset, v.u32(8)? as usize, 32)? {
            let p = offset + i * 32;
            resources.push(Resource {
                name: v.string(v.u32(p)? as usize)?,
                kind: v.u32(p + 4)?,
                return_type: v.u32(p + 8)?,
                dimension: v.u32(p + 12)?,
                slot: v.u32(p + 20)?,
                count: v.u32(p + 24)?,
            });
        }
        let mut buffers = Vec::new();
        let offset = v.u32(4)? as usize;
        let mut budget = 65536;
        for i in v.table(offset, v.u32(0)? as usize, 24)? {
            let p = offset + i * 24;
            let size = v.u32(p + 12)?;
            if size > 65536 {
                return Err(error(p / 4, "constant-buffer size exceeds bound"));
            }
            let mut variables = Vec::new();
            let entries = v.u32(p + 8)? as usize;
            for j in v.table(entries, v.u32(p + 4)? as usize, 40)? {
                let q = entries + j * 40;
                let start = v.u32(q + 4)?;
                let len = v.u32(q + 8)?;
                if start.checked_add(len).is_none_or(|end| end > size) {
                    return Err(error(q / 4, "variable exceeds constant buffer"));
                }
                let default = v.u32(q + 20)? as usize;
                variables.push(Variable {
                    name: v.string(v.u32(q)? as usize)?,
                    offset: start,
                    size: len,
                    ty: v.ty(v.u32(q + 16)? as usize, &mut Vec::new(), &mut budget)?,
                    default: if default == 0 {
                        None
                    } else {
                        Some(v.bytes(default, len as usize)?.to_vec())
                    },
                });
            }
            buffers.push(Buffer {
                name: v.string(v.u32(p)? as usize)?,
                size,
                variables,
            });
        }
        Ok(Reflection {
            buffers,
            resources,
            inputs: signature(get(b"ISGN")?)?,
            outputs: signature(get(b"OSGN")?)?,
        })
    }
}
