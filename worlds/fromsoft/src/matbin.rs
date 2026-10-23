//! Elden Ring material binaries (`MAB\0` version 2): the shader, typed parameters and texture samplers that a FLVER
//! material's `.matxml` name refers to. Values are kept as stored; nothing is interpreted.
//!
//! A value's stored extent runs to the next stored item (string or value), so it is measured rather than assumed
//! from its type: float3 colours are stored with three floats or five, and a few vectors are stored shorter than
//! their type (kept as [`Value::Truncated`]).
use crate::{
    Result,
    bytes::{View, coverage},
    invalid,
};
use std::collections::BTreeSet;

const HEADER: usize = 0x38;
const PARAM: usize = 0x28;
const SAMPLER: usize = 0x30;

/// One stored parameter value. Type codes: 0 bool, 4 int, 5 int2, 8 float, 9 float2, 10 float3, 11 float4,
/// 12 float5.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub enum Value {
    Bool(bool),
    Int(i32),
    Int2([i32; 2]),
    Float(f32),
    Float2([f32; 2]),
    /// Three floats; some files store two more after them (kept, not interpreted).
    Float3([f32; 3], Option<[f32; 2]>),
    Float4([f32; 4]),
    Float5([f32; 5]),
    /// A value stored with fewer bytes than its type `code` declares (the next item starts sooner): the stored
    /// bytes, uninterpreted.
    Truncated {
        code: u32,
        bytes: Vec<u8>,
    },
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Param {
    pub name: String,
    pub key: u32,
    pub value: Value,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Sampler {
    /// The shader's sampler name, for example `..._snp_Texture2D_2_AlbedoMap_0`.
    pub kind: String,
    /// Source texture path; empty when the material leaves the slot to the FLVER or the shader default.
    pub path: String,
    pub key: u32,
    /// Two floats stored after the key, kept uninterpreted.
    pub unknown: [f32; 2],
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Material {
    /// Shader source path (`.spx`).
    pub shader: String,
    /// The material's own source path (`.matxml`).
    pub source: String,
    pub key: u32,
    pub params: Vec<Param>,
    pub samplers: Vec<Sampler>,
    /// Bytes after the last item the file references, as stored (empty in most files; some end with strings of
    /// an earlier revision of the material that nothing points at).
    pub unreferenced_tail: Vec<u8>,
}
/// A NUL-terminated UTF-16 string and the bytes it occupies including the terminator.
fn string(v: &View<'_>, o: usize) -> Result<(String, usize)> {
    let mut units = Vec::new();
    let mut p = o;
    loop {
        let c = v.u16(p)?;
        p += 2;
        if c == 0 {
            break;
        }
        units.push(c);
    }
    Ok((
        String::from_utf16(&units).map_err(|e| invalid(v.format, o, e))?,
        p - o,
    ))
}
fn float(v: &View<'_>, o: usize) -> Result<f32> {
    let f = f32::from_bits(v.u32(o)?);
    if !f.is_finite() {
        return Err(invalid(v.format, o, "non-finite value"));
    }
    Ok(f)
}
fn floats<const N: usize>(v: &View<'_>, o: usize) -> Result<[f32; N]> {
    let mut out = [0.; N];
    for (i, f) in out.iter_mut().enumerate() {
        *f = float(v, o + i * 4)?;
    }
    Ok(out)
}
/// The value of type `code` stored at `o` with `extent` bytes available before the next item, and the bytes it
/// occupies.
fn value(v: &View<'_>, at: usize, code: u32, o: usize, extent: usize) -> Result<(Value, usize)> {
    let declared = match code {
        0 => 1,
        4 | 8 => 4,
        5 | 9 => 8,
        10 => 12,
        11 => 16,
        12 => 20,
        _ => return Err(invalid(v.format, at, format!("parameter type {code}"))),
    };
    if extent < declared {
        return Ok((
            Value::Truncated {
                code,
                bytes: v.slice(o, extent)?.to_vec(),
            },
            extent,
        ));
    }
    Ok(match code {
        0 => (Value::Bool(v.boolean(o)?), 1),
        4 => (Value::Int(v.u32(o)? as i32), 4),
        5 => (Value::Int2([v.u32(o)? as i32, v.u32(o + 4)? as i32]), 8),
        8 => (Value::Float(float(v, o)?), 4),
        9 => (Value::Float2(floats(v, o)?), 8),
        10 if extent >= 20 => (Value::Float3(floats(v, o)?, Some(floats(v, o + 12)?)), 20),
        10 => (Value::Float3(floats(v, o)?, None), 12),
        11 => (Value::Float4(floats(v, o)?), 16),
        _ => (Value::Float5(floats(v, o)?), 20),
    })
}
impl Material {
    /// Read a decompressed MATBIN. Every byte up to the last referenced item belongs to the header, a record, a
    /// string or a value, with at most one zero byte realigning UTF-16 strings after an odd-length value.
    pub fn read(data: &[u8]) -> Result<Self> {
        let v = View {
            data,
            format: "MATBIN",
            be: false,
        };
        v.expect(0, b"MAB\0")?;
        if v.u32(4)? != 2 {
            return Err(invalid(v.format, 4, "version"));
        }
        v.expect(0x24, &[0; 0x14])?;
        let params = v.u32(0x1c)? as usize;
        let samplers = v.u32(0x20)? as usize;
        let params_bytes = params
            .checked_mul(PARAM)
            .ok_or_else(|| invalid(v.format, 0x1c, "count overflow"))?;
        let samplers_bytes = samplers
            .checked_mul(SAMPLER)
            .ok_or_else(|| invalid(v.format, 0x20, "count overflow"))?;
        let records_end = HEADER + params_bytes + samplers_bytes;
        v.slice(HEADER, params_bytes + samplers_bytes)?;
        let mut spans = vec![(0, records_end)];
        let mut text = |o: usize| -> Result<String> {
            if o < records_end {
                return Err(invalid(v.format, o, "string inside the record tables"));
            }
            let (s, len) = string(&v, o)?;
            spans.push((o, len));
            Ok(s)
        };
        let shader = text(v.size(8)?)?;
        let source = text(v.size(16)?)?;
        let mut stored = Vec::with_capacity(params);
        for i in 0..params {
            let r = HEADER + i * PARAM;
            v.expect(r + 0x18, &[0; 0x10])?;
            let name = text(v.size(r)?)?;
            let o = v.size(r + 8)?;
            if o < records_end || o >= data.len() {
                return Err(invalid(v.format, r + 8, "value outside the data area"));
            }
            stored.push((name, v.u32(r + 0x10)?, r + 0x14, v.u32(r + 0x14)?, o));
        }
        let mut sampler_list = Vec::with_capacity(samplers);
        for i in 0..samplers {
            let r = HEADER + params_bytes + i * SAMPLER;
            v.expect(r + 0x1c, &[0; 0x14])?;
            sampler_list.push(Sampler {
                kind: text(v.size(r)?)?,
                path: text(v.size(r + 8)?)?,
                key: v.u32(r + 0x10)?,
                unknown: floats(&v, r + 0x14)?,
            });
        }
        let starts: BTreeSet<usize> = spans
            .iter()
            .map(|s| s.0)
            .chain(stored.iter().map(|s| s.4))
            .collect();
        let mut param_list = Vec::with_capacity(params);
        for (name, key, at, code, o) in stored {
            let next = starts.range(o + 1..).next().copied().unwrap_or(data.len());
            let (value, len) = value(&v, at, code, o, next - o)?;
            spans.push((o, len));
            param_list.push(Param { name, key, value });
        }
        let referenced = spans.iter().map(|&(o, n)| o + n).max().unwrap_or(0);
        let tail_start = referenced + usize::from(referenced % 2 == 1 && data.get(referenced) == Some(&0));
        coverage(&v, spans, tail_start.min(data.len()), 1)?;
        Ok(Self {
            shader,
            source,
            key: v.u32(0x18)?,
            params: param_list,
            samplers: sampler_list,
            unreferenced_tail: data.get(tail_start..).unwrap_or_default().to_vec(),
        })
    }
    /// The parameter called `name`, if the material stores it.
    pub fn param(&self, name: &str) -> Option<&Value> {
        self.params.iter().find(|p| p.name == name).map(|p| &p.value)
    }
}
