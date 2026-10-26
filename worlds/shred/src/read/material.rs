//! MBP/MDP bank records. All offsets are BE and MDP pointers are relative to +0x40.
//! Opaque header/state fields are explicit data, not interpreted enums. New structural
//! layouts, encoding words, sentinels, gaps, overlaps and nonzero padding are errors.
use crate::components::{MaterialKey, MaterialParameter, MaterialSampler, SamplerNameSource, ShredMaterial};
use crate::{Result, invalid};
use primase_bytes::{Endian::Big, Field, View};
use primase_core::{Entity, World};
use std::collections::{BTreeMap, BTreeSet};

/// Reflected names from the bound fragment program, indexed by the source hash.
pub type UniformNames = BTreeMap<u32, String>;

/// Recovered from the shipped executable's own NUL-terminated strings. The
/// exact sdbm matches identify names only, not texture-unit or shading semantics.
/// Source executable SHA256 and the exhaustive dictionary census are documented
/// in the crate README. No compiled Cg parameter table supplied these names.
pub fn sampler_name(hash: u32) -> Option<&'static str> {
    match hash {
        0x95591c40 => Some("Color_Map"),
        0xd79d6be4 => Some("Normal_Map"),
        0xffc34d98 => Some("Spec_Map"),
        0x2538db2e => Some("Effect_Map"),
        _ => None,
    }
}

fn word(b: &[u8], at: usize) -> Result<u32> {
    View::new(b)
        .u32(at, Big)
        .field("material word")
        .map_err(Into::into)
}
fn expect(b: &[u8], at: usize, value: u32) -> Result<()> {
    if word(b, at)? != value {
        return Err(invalid(at, format!("expected {value:#x}")));
    }
    Ok(())
}
fn pointer(b: &[u8], at: usize, expected: usize) -> Result<()> {
    if u64::from(word(b, at)?) + 64 != expected as u64 {
        return Err(invalid(at, "material pointer gap, overlap or unexpected layout"));
    }
    Ok(())
}
fn string(b: &[u8], pos: &mut usize) -> Result<String> {
    let tail = b
        .get(*pos..)
        .ok_or_else(|| invalid(*pos, "string outside record"))?;
    let n = tail
        .iter()
        .position(|&x| x == 0)
        .ok_or_else(|| invalid(*pos, "unterminated string"))?;
    if !tail[..n].is_ascii() {
        return Err(invalid(*pos, "non-ASCII source name"));
    }
    let out = String::from_utf8(tail[..n].to_vec()).map_err(|e| invalid(*pos, e))?;
    *pos += n + 1;
    Ok(out)
}

/// Parse one complete record. `names` must come from this record's bound PSP, not
/// a global dictionary: the same name hash in another program is not proof of binding.
pub fn record(bank: &str, b: &[u8], names: &UniformNames) -> Result<ShredMaterial> {
    for (at, value) in [
        (0, 0xaaaa0003),
        (4, 64),
        (8, 0x00400000),
        (12, 0xbeefface),
        (0x14, 0xffff000e),
        (0x18, u32::MAX),
        (0x20, u32::MAX),
        (0x24, u32::MAX),
        (0x30, u32::MAX),
        (0x34, u32::MAX),
        (0x38, 0),
        (0x40, u32::MAX),
    ] {
        expect(b, at, value)?;
    }
    let mut pos = 96;
    pointer(b, 0x44, pos)?;
    let vertex_shader = string(b, &mut pos)?;
    pointer(b, 0x48, pos)?;
    let fragment_shader = string(b, &mut pos)?;
    pointer(b, 0x50, pos)?;
    let np = word(b, 0x4c)? as usize;
    let nt = word(b, 0x54)? as usize;
    // Bound counts before arithmetic or allocation (also on 32-bit hosts).
    if np > b.len() / 32 || nt > b.len() / 32 {
        return Err(invalid(0x4c, "material counts exceed record"));
    }
    let descriptors = pos;
    pos += np * 16;
    let mut parameters = Vec::new();
    for i in 0..np {
        let at = descriptors + 16 * i;
        let hash = word(b, at)?;
        let encoding = word(b, at + 4)?;
        if !matches!(encoding, 0x000100ad | 0x020100ad) {
            return Err(invalid(at + 4, "unknown material parameter encoding"));
        }
        expect(b, at + 8, u32::MAX)?;
        pointer(b, at + 12, pos)?;
        let mut value = [0.; 4];
        for (j, v) in value.iter_mut().enumerate() {
            *v = View::new(b).f32(pos + 4 * j, Big).field("material value")?;
            if !v.is_finite() {
                return Err(invalid(pos + 4 * j, "non-finite material value"));
            }
        }
        pos += 16;
        parameters.push(MaterialParameter {
            hash,
            name: names.get(&hash).cloned(),
            source_encoding: encoding,
            value,
        });
    }
    pointer(b, 0x58, pos)?;
    let mut samplers = Vec::new();
    for i in 0..nt {
        let at = pos;
        expect(b, at + 24, 0xffff000e)?;
        let name = sampler_name(word(b, at + 16)?);
        samplers.push(MaterialSampler {
            slot: i as u32,
            hash: word(b, at + 16)?,
            name: name.map(str::to_owned),
            name_source: name.map(|_| SamplerNameSource::ExecutableStringSdbm),
            texture: String::new(),
            unknown_state: [
                word(b, at)?,
                word(b, at + 4)?,
                word(b, at + 8)?,
                word(b, at + 12)?,
                word(b, at + 20)?,
            ],
        });
        pos += 28;
    }
    pointer(b, 0x5c, pos)?;
    let paths = pos;
    pos += nt * 4;
    for (i, sampler) in samplers.iter_mut().enumerate() {
        pointer(b, paths + i * 4, pos)?;
        sampler.texture = string(b, &mut pos)?;
    }
    let aligned = pos
        .checked_add(15)
        .ok_or_else(|| invalid(pos, "alignment overflow"))?
        & !15;
    if b.len() != aligned {
        return Err(invalid(pos, "record must end at next 16-byte boundary"));
    }
    if let Some(i) = b[pos..].iter().position(|&x| x != 0) {
        return Err(invalid(pos + i, "nonzero record padding"));
    }
    Ok(ShredMaterial {
        key: MaterialKey {
            bank: bank.into(),
            id: word(b, 0x10)?,
        },
        unknown_header: [word(b, 0x1c)?, word(b, 0x28)?, word(b, 0x2c)?, word(b, 0x3c)?],
        vertex_shader,
        fragment_shader,
        parameters,
        samplers,
    })
}

/// Parse the full index and payload. The callback resolves only the requested PSP
/// stem; return an empty table if absent. Names and empty bindings are never invented.
pub fn bank(
    name: &str,
    index: &[u8],
    data: &[u8],
    mut names: impl FnMut(&str) -> Result<UniformNames>,
) -> Result<Vec<ShredMaterial>> {
    for (at, value) in [(0, 0xfadd0001), (8, 0x00180000), (12, 0xbeefface), (20, 24)] {
        expect(index, at, value)?;
    }
    if word(index, 4)? as usize != index.len() {
        return Err(invalid(4, "MBP size"));
    }
    let n = word(index, 16)? as usize;
    if n > index.len() / 12 || 24 + n * 12 != index.len() {
        return Err(invalid(16, "MBP table does not end at EOF"));
    }
    let mut out = Vec::new();
    let mut pos = 0;
    let mut ids = BTreeSet::new();
    for i in 0..n {
        let at = 24 + i * 12;
        let id = word(index, at)?;
        if !ids.insert(id) {
            return Err(invalid(at, "duplicate material ID within bank"));
        }
        if word(index, at + 4)? as usize != pos {
            return Err(invalid(at + 4, "MDP slices must tile"));
        }
        let packed = word(index, at + 8)?;
        if packed & 0xffc00000 != 0x00400000 {
            return Err(invalid(at + 8, "unknown packed size flags"));
        }
        let size = (packed & 0x003fffff) as usize;
        let b = View::new(data).slice(pos, size).field("MDP record")?;
        let mut s = (word(b, 0x48)? as usize)
            .checked_add(64)
            .ok_or_else(|| invalid(pos + 0x48, "shader offset overflow"))?;
        let shader = string(b, &mut s)?;
        let m = record(name, b, &names(&shader)?)
            .map_err(|e| invalid(pos, format!("bank {name} id {id}: {e}")))?;
        if m.key.id != id {
            return Err(invalid(pos + 16, "MBP/MDP ID mismatch"));
        }
        out.push(m);
        pos += size;
    }
    if pos != data.len() {
        return Err(invalid(pos, "MDP trailing bytes"));
    }
    Ok(out)
}

/// Attach one entity per (bank, id). P3's mesh ID-to-bank resolution is a later join.
/// Reject duplicate keys before mutating the world.
pub fn attach(world: &mut World, materials: Vec<ShredMaterial>) -> Result<BTreeMap<MaterialKey, Entity>> {
    let mut keys = BTreeSet::new();
    for m in &materials {
        if !keys.insert(m.key.clone()) {
            return Err(invalid(0, "duplicate (bank,id) material"));
        }
    }
    if world.iter::<ShredMaterial>().any(|(_, m)| keys.contains(&m.key)) {
        return Err(invalid(0, "material key already attached"));
    }
    let mut out = BTreeMap::new();
    for m in materials {
        let key = m.key.clone();
        let e = world.spawn();
        world.insert(e, m);
        out.insert(key, e);
    }
    Ok(out)
}

/// Read only CgBinaryProgram's named-parameter table from every 64-byte-aligned
/// fragment permutation. This is reflection, not an instruction/shader decoder.
pub fn fragment_uniform_names(b: &[u8]) -> Result<UniformNames> {
    let mut out = UniformNames::new();
    let mut pos = 0;
    while pos < b.len() {
        let tail = &b[pos..];
        expect(tail, 0, 0x1b5c)?;
        expect(tail, 4, 6)?;
        let size = word(tail, 8)? as usize;
        if size < 32 {
            return Err(invalid(pos + 8, "Cg program too short"));
        }
        let program = View::new(b).slice(pos, size).field("Cg program")?;
        let count = word(program, 12)? as usize;
        let at = word(program, 16)? as usize;
        if count > size / 48 {
            return Err(invalid(pos + 12, "Cg parameter count"));
        }
        View::new(program).slice(at, count * 48).field("Cg parameters")?;
        for i in 0..count {
            // Cg variability 4102 is uniform; other interface classes cannot name
            // a material constant, even if their string happens to hash the same.
            if word(program, at + i * 48 + 8)? != 4102 {
                continue;
            }
            let offset = word(program, at + i * 48 + 16)? as usize;
            if offset == 0 {
                continue;
            }
            let mut offset = offset;
            let name = string(program, &mut offset)?;
            if name.is_empty() {
                continue;
            }
            for n in [name.as_str(), name.split('[').next().unwrap_or(&name)] {
                let h = primase_hash::sdbm(n.as_bytes());
                if let Some(old) = out.insert(h, n.into())
                    && old != n
                {
                    return Err(invalid(pos + at + i * 48, "Cg parameter name hash collision"));
                }
            }
        }
        let next = pos
            .checked_add(size)
            .and_then(|n| n.checked_add(63))
            .ok_or_else(|| invalid(pos, "Cg size overflow"))?
            & !63;
        let padding = View::new(b)
            .slice(pos + size, next - pos - size)
            .field("Cg padding")?;
        if let Some(i) = padding.iter().position(|&x| x != 0) {
            return Err(invalid(pos + size + i, "Cg nonzero padding"));
        }
        pos = next;
    }
    Ok(out)
}
