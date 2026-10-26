//! D3DX's CTAB comment: offsets are relative to the header following the FourCC.
use crate::{Result, error};

/// A CTAB type. Numeric class/type values are D3DXPARAMETER_CLASS/TYPE, kept alongside their dimensions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Type {
    pub class: u16,
    pub kind: u16,
    pub rows: u16,
    pub columns: u16,
    pub elements: u16,
    pub members: Vec<TypeMember>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeMember {
    pub name: String,
    pub ty: Type,
}

/// A named allocation, including the original type even when optimization shortened its register range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Constant {
    pub name: String,
    /// D3DXREGISTER_SET: BOOL=0, INT4=1, FLOAT4=2, SAMPLER=3.
    pub register_set: u16,
    pub register: u16,
    pub count: u16,
    /// Compiler-reserved word of D3DXSHADER_CONSTANTINFO. Compilers write nonzero values;
    /// it is retained verbatim and does not change the explicit register allocation.
    pub reserved: u16,
    pub ty: Type,
    /// Raw register defaults (BOOL rows are 4 bytes, other numeric rows 16); no defaults on samplers.
    pub default: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstantTable {
    pub creator: String,
    pub target: String,
    pub version: u32,
    pub flags: u32,
    pub constants: Vec<Constant>,
}

struct Reader<'a> {
    data: &'a [u8],
    word: usize,
}
impl Reader<'_> {
    fn range(&self, at: usize, len: usize) -> Result<&[u8]> {
        at.checked_add(len)
            .and_then(|end| self.data.get(at..end))
            .ok_or_else(|| {
                error(
                    self.word + at / 4,
                    format!("CTAB range {at}+{len} outside comment"),
                )
            })
    }
    fn u16(&self, at: usize) -> Result<u16> {
        let b = self.range(at, 2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&self, at: usize) -> Result<u32> {
        let b = self.range(at, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn offset(&self, at: usize) -> Result<usize> {
        Ok(self.u32(at)? as usize)
    }
    fn string(&self, at: usize) -> Result<String> {
        let bytes = self.range(at, self.data.len().saturating_sub(at))?;
        let end = bytes
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| error(self.word + at / 4, "CTAB string has no NUL"))?;
        std::str::from_utf8(&bytes[..end])
            .map(str::to_owned)
            .map_err(|_| error(self.word + at / 4, "CTAB string is not UTF-8"))
    }
    fn ty(&self, at: usize, depth: usize, budget: &mut usize) -> Result<Type> {
        if depth > 32 || *budget == 0 {
            return Err(error(
                self.word + at / 4,
                "CTAB type nesting/cycle or 4096-node limit",
            ));
        }
        *budget -= 1;
        self.range(at, 16)?;
        let count = self.u16(at + 10)? as usize;
        let members_at = self.offset(at + 12)?;
        self.range(members_at, count * 8)?;
        let mut members = Vec::new();
        for n in 0..count {
            members.push(TypeMember {
                name: self.string(self.offset(members_at + n * 8)?)?,
                ty: self.ty(self.offset(members_at + n * 8 + 4)?, depth + 1, budget)?,
            });
        }
        Ok(Type {
            class: self.u16(at)?,
            kind: self.u16(at + 2)?,
            rows: self.u16(at + 4)?,
            columns: self.u16(at + 6)?,
            elements: self.u16(at + 8)?,
            members,
        })
    }
}

pub(crate) fn read(data: &[u8], word: usize) -> Result<ConstantTable> {
    let r = Reader { data, word };
    if r.u32(0)? != 28 {
        return Err(error(word, "CTAB header size is not 28"));
    }
    let version = r.u32(8)?;
    if version != 0xffff_0300 {
        return Err(error(word + 2, "CTAB version disagrees with ps_3_0"));
    }
    let count = r.offset(12)?;
    if count > 1024 {
        return Err(error(word + 3, "CTAB exceeds 1024 constants"));
    }
    let base = r.offset(16)?;
    r.range(base, count * 20)?;
    let mut constants: Vec<Constant> = Vec::new();
    let mut budget = 4096;
    for n in 0..count {
        let at = base + n * 20;
        let name = r.string(r.offset(at)?)?;
        let register_set = r.u16(at + 4)?;
        let register = r.u16(at + 6)?;
        let count = r.u16(at + 8)?;
        let limit = match register_set {
            0 | 1 | 3 => 16,
            2 => 224,
            _ => return Err(error(word + at / 4, format!("CTAB register set {register_set}"))),
        };
        if count == 0 || u32::from(register) + u32::from(count) > limit {
            return Err(error(word + at / 4, "CTAB empty/out-of-bank register range"));
        }
        if constants.iter().any(|c| {
            c.name == name
                || (c.register_set == register_set
                    && c.register < register + count
                    && register < c.register + c.count)
        }) {
            return Err(error(
                word + at / 4,
                "CTAB overlapping allocations or duplicate name",
            ));
        }
        let default_at = r.offset(at + 16)?;
        let default = if default_at == 0 {
            None
        } else {
            if register_set == 3 {
                return Err(error(word + at / 4, "CTAB sampler default"));
            }
            Some(
                r.range(
                    default_at,
                    usize::from(count) * if register_set == 0 { 4 } else { 16 },
                )?
                .to_vec(),
            )
        };
        constants.push(Constant {
            name,
            register_set,
            register,
            count,
            reserved: r.u16(at + 10)?,
            ty: r.ty(r.offset(at + 12)?, 0, &mut budget)?,
            default,
        });
    }
    Ok(ConstantTable {
        creator: r.string(r.offset(4)?)?,
        version,
        flags: r.u32(20)?,
        target: r.string(r.offset(24)?)?,
        constants,
    })
}
