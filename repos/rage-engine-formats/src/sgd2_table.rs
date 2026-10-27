//! The binding table of a shader program of the Enhanced shader database ([`crate::sgd2::Program::tables`]): what
//! the program's resources are called and where they sit, as the game's own tool stored them. Per program:
//!
//! ```text
//! u16 nA, u32 nB, u16 (kept)
//! nA resource records of 28 bytes:
//!     u16 tag, u16 register, u8 count + u8 class, u16 space,
//!     6 bytes (kept), u32 size, u16 (kept), u32 name hash, u32 second name hash
//! nB constant records of 24 bytes:
//!     u32 flags, u16 offset, u16 id, u32 x, u32 s, u32 name hash, u32 second name hash
//! the names: NUL-terminated ASCII, to the end
//! ```
//!
//! A resource record is a constant buffer, a texture or a sampler of the program. Its register and space are its
//! D3D12 bind point (a constant buffer's size is the stored size, the declared one rounded up to 16 bytes); its
//! `count` is how many constant records are its own, taken from the constant records in order (the counts sum to
//! `nB`: checked), none for a texture or a sampler. A constant record's `offset` is the byte offset in its constant
//! buffer and the low byte of its flags the value's type. The first name hash of every record is the joaat of a name
//! in the table, in the name's own case or lowercase: the program's name for the resource or constant (`HDRSampler`,
//! `specularFresnel`). The second is the hash of the shader's current name for a texture, sampler or material
//! constant (`DiffuseTex`, `Fresnel`), which is the key a drawable's material stores its parameter under; that name is not in the table. An engine constant or buffer has no
//! second name (0, or the first repeated).
//!
//! Everything but the fields named is kept as stored. The counts and sizes are checked against the file; the
//! census of the shader database holds the records to the programs' own interfaces: every constant
//! buffer, texture and sampler a program declares has a record at its register and space, with the buffer's size.
//! No I/O.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use std::collections::{BTreeMap, BTreeSet};

use crate::{Result, invalid};

/// Bytes of a resource record.
const RESOURCE: usize = 28;
/// Bytes of a constant record.
const CONSTANT: usize = 24;
/// Bytes before the first record.
const HEADER: usize = 8;

/// One resource record: a constant buffer, texture or sampler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableResource {
    pub tag: u16,
    pub register: u16,
    /// How many constant records follow for this resource, in record order (0 for a texture or a sampler).
    pub constants: u8,
    /// The byte after the count, as stored.
    pub class: u8,
    pub space: u16,
    /// The six bytes after the space, as stored.
    pub unknown_8: [u8; 6],
    /// A constant buffer's stored size in bytes.
    pub size: u32,
    pub unknown_18: u16,
    /// The hash of the program's name for it (the joaat of a name in the table), and of the shader's current name (0
    /// when it has none; not a name of the table).
    pub name: u32,
    pub second_name: u32,
}

/// One constant record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableConstant {
    /// The low byte the value's type, the rest as stored.
    pub flags: u32,
    /// Byte offset in its constant buffer.
    pub offset: u16,
    pub id: u16,
    pub x: u32,
    pub s: u32,
    pub name: u32,
    pub second_name: u32,
}

impl TableConstant {
    /// The type byte of the flags: 9 a scalar, 10 two, 11 three and 12 four floats (the high 16 bits of the flags the
    /// vec4 count of an array), 14 a matrix; 0 and 5 the engine's flags and counts.
    pub fn kind(&self) -> u8 {
        self.flags.to_le_bytes()[0]
    }
}

/// A program's table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramTable {
    /// The u16 after the counts, as stored.
    pub unknown_6: u16,
    pub resources: Vec<TableResource>,
    /// Every constant record, in the order of the resources that own them.
    pub constants: Vec<TableConstant>,
    /// The names, in order.
    pub names: Vec<String>,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at.checked_add(2)?)?
        .try_into()
        .ok()
        .map(u16::from_le_bytes)
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at.checked_add(4)?)?
        .try_into()
        .ok()
        .map(u32::from_le_bytes)
}

/// Read a program's table.
pub fn parse(bytes: &[u8]) -> Result<ProgramTable> {
    let fail = |at: usize, why: &str| invalid("sgd2 program table", at as u64, why);
    let (Some(n_resources), Some(n_constants), Some(unknown_6)) =
        (u16_at(bytes, 0), u32_at(bytes, 2), u16_at(bytes, 6))
    else {
        return Err(fail(0, "shorter than its header"));
    };
    let n_constants = usize::try_from(n_constants).map_err(|_| fail(2, "a count past the platform"))?;
    let resources_end = usize::from(n_resources)
        .checked_mul(RESOURCE)
        .and_then(|n| n.checked_add(HEADER))
        .ok_or_else(|| fail(0, "size overflow"))?;
    let constants_end = n_constants
        .checked_mul(CONSTANT)
        .and_then(|n| n.checked_add(resources_end))
        .ok_or_else(|| fail(2, "size overflow"))?;
    if constants_end > bytes.len() {
        return Err(fail(
            2,
            &format!(
                "{n_resources} resources and {n_constants} constants in {} bytes",
                bytes.len()
            ),
        ));
    }
    let mut resources = Vec::with_capacity(usize::from(n_resources));
    for i in 0..usize::from(n_resources) {
        let at = HEADER.saturating_add(i.saturating_mul(RESOURCE));
        let (
            Some(tag),
            Some(register),
            Some(space),
            Some(size),
            Some(unknown_18),
            Some(name),
            Some(second_name),
        ) = (
            u16_at(bytes, at),
            u16_at(bytes, at.saturating_add(2)),
            u16_at(bytes, at.saturating_add(6)),
            u32_at(bytes, at.saturating_add(14)),
            u16_at(bytes, at.saturating_add(18)),
            u32_at(bytes, at.saturating_add(20)),
            u32_at(bytes, at.saturating_add(24)),
        )
        else {
            return Err(fail(at, "a short resource record"));
        };
        let counts = bytes
            .get(at.saturating_add(4)..at.saturating_add(6))
            .ok_or_else(|| fail(at, "a short resource record"))?;
        let unknown_8: [u8; 6] = bytes
            .get(at.saturating_add(8)..at.saturating_add(14))
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| fail(at, "a short resource record"))?;
        resources.push(TableResource {
            tag,
            register,
            constants: counts.first().copied().unwrap_or(0),
            class: counts.get(1).copied().unwrap_or(0),
            space,
            unknown_8,
            size,
            unknown_18,
            name,
            second_name,
        });
    }
    let owned: usize = resources.iter().map(|r| usize::from(r.constants)).sum();
    if owned != n_constants {
        return Err(fail(
            0,
            &format!("the resources own {owned} constants, the header says {n_constants}"),
        ));
    }
    let mut constants = Vec::with_capacity(n_constants);
    for i in 0..n_constants {
        let at = resources_end.saturating_add(i.saturating_mul(CONSTANT));
        let (Some(flags), Some(offset), Some(id), Some(x), Some(s), Some(name), Some(second_name)) = (
            u32_at(bytes, at),
            u16_at(bytes, at.saturating_add(4)),
            u16_at(bytes, at.saturating_add(6)),
            u32_at(bytes, at.saturating_add(8)),
            u32_at(bytes, at.saturating_add(12)),
            u32_at(bytes, at.saturating_add(16)),
            u32_at(bytes, at.saturating_add(20)),
        ) else {
            return Err(fail(at, "a short constant record"));
        };
        constants.push(TableConstant {
            flags,
            offset,
            id,
            x,
            s,
            name,
            second_name,
        });
    }
    let tail = bytes.get(constants_end..).unwrap_or_default();
    let Some((&0, text)) = tail.split_last() else {
        if tail.is_empty() {
            return Ok(ProgramTable {
                unknown_6,
                resources,
                constants,
                names: Vec::new(),
            });
        }
        return Err(fail(constants_end, "names that do not end in a NUL"));
    };
    let mut names = Vec::new();
    for part in text.split(|&b| b == 0) {
        if !part.is_ascii() {
            return Err(fail(constants_end, "a name that is not ASCII"));
        }
        names.push(String::from_utf8_lossy(part).into_owned());
    }
    Ok(ProgramTable {
        unknown_6,
        resources,
        constants,
        names,
    })
}

impl ProgramTable {
    /// The constant records the `i`th resource owns. The records are stored in the order of the names: each buffer's
    /// constants are named in a block that ends with the buffer's own name, so the records go by where the buffers'
    /// names stand, which is not always the order of the resource records (a buffer of space 1 can stand between those
    /// of space 0, and a buffer the program reads through root constants after the others).
    pub fn constants_of(&self, i: usize) -> &[TableConstant] {
        let Some(mine) = self.resources.get(i) else {
            return &[];
        };
        let key = |k: usize, r: &TableResource| (self.name_position(r.name).unwrap_or(usize::MAX), k);
        let before: usize = self
            .resources
            .iter()
            .enumerate()
            .filter(|&(k, r)| key(k, r) < key(i, mine))
            .map(|(_, r)| usize::from(r.constants))
            .sum();
        self.constants
            .get(before..before.saturating_add(usize::from(mine.constants)))
            .unwrap_or_default()
    }

    /// Where in the names the one a hash is the joaat of stands.
    fn name_position(&self, hash: u32) -> Option<usize> {
        self.names.iter().position(|n| {
            primase_foundations::hash::joaat(n.as_bytes()) == hash
                || primase_foundations::hash::joaat_exact(n.as_bytes()) == hash
        })
    }

    /// The name a hash is the joaat of, in the name's own case or lowercase.
    pub fn name_of(&self, hash: u32) -> Option<&str> {
        self.names.iter().map(String::as_str).find(|n| {
            primase_foundations::hash::joaat(n.as_bytes()) == hash
                || primase_foundations::hash::joaat_exact(n.as_bytes()) == hash
        })
    }
}

/// A material parameter the shader names twice: its record in an effect's parameter table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct EffectParameter {
    pub flags: u32,
    /// Byte offset in the constant buffer of the program that reads it.
    pub offset: u16,
    pub id: u16,
    /// The program's name for it (hash) and the shader's current name: the key a drawable's material stores it by.
    pub name: u32,
    pub second_name: u32,
}

/// The parameter records of an effect's record (`rest`, [`crate::sgd2::Effect::rest`]) that rename a
/// parameter of one of its programs. `known` is what the effect's programs read: each constant's name hash with the
/// type bytes it has. The record's structure around its tables is not read: a parameter record is found by its
/// shape, a 24-byte record `flags, offset and id, two words, name hash, second name hash` at a 4-byte position whose
/// name hash is a constant of one of the programs and whose type byte is that constant's. A hash of 32 bits that
/// two unrelated words share would need a second program constant with the same type byte beside it; the census
/// holds the result to CodeWalker's parameter list (every parameter it names that a program reads is found).
/// Records whose second name is 0 or the first are not returned.
pub fn effect_parameters(rest: &[u8], known: &BTreeMap<u32, BTreeSet<u8>>) -> Vec<EffectParameter> {
    let mut out = BTreeSet::new();
    // `at` is where the two name hashes of a record are, 16 bytes into it
    let mut at = 16usize;
    while at.saturating_add(8) <= rest.len() {
        if let (Some(name), Some(second)) = (u32_at(rest, at), u32_at(rest, at.saturating_add(4)))
            && second != 0
            && second != name
            && let Some(types) = known.get(&name)
            && let Some(start) = at.checked_sub(16)
            && let (Some(flags), Some(offset), Some(id)) = (
                u32_at(rest, start),
                u16_at(rest, start.saturating_add(4)),
                u16_at(rest, start.saturating_add(6)),
            )
            && types.contains(&flags.to_le_bytes()[0])
        {
            out.insert(EffectParameter {
                flags,
                offset,
                id,
                name,
                second_name: second,
            });
        }
        at = at.saturating_add(4);
    }
    out.into_iter().collect()
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, reason = "hand-built fixtures")]
mod tests {
    use super::*;

    fn resource(register: u16, constants: u8, space: u16, size: u32, name: u32, second: u32) -> Vec<u8> {
        let mut r = Vec::new();
        r.extend(0x430u16.to_le_bytes());
        r.extend(register.to_le_bytes());
        r.extend([constants, 2]);
        r.extend(space.to_le_bytes());
        r.extend([1, 2, 3, 4, 5, 6]);
        r.extend(size.to_le_bytes());
        r.extend(9u16.to_le_bytes());
        r.extend(name.to_le_bytes());
        r.extend(second.to_le_bytes());
        r
    }

    fn constant(flags: u32, offset: u16, name: u32, second: u32) -> Vec<u8> {
        let mut c = Vec::new();
        c.extend(flags.to_le_bytes());
        c.extend(offset.to_le_bytes());
        c.extend(77u16.to_le_bytes());
        c.extend([0; 8]);
        c.extend(name.to_le_bytes());
        c.extend(second.to_le_bytes());
        c
    }

    /// Two resources, a cbuffer of two constants at register 10 space 1 and a texture, and the names.
    fn table() -> Vec<u8> {
        let (a, b, t) = (
            primase_foundations::hash::joaat(b"specularfresnel"),
            primase_foundations::hash::joaat(b"bumpiness"),
            primase_foundations::hash::joaat(b"diffusetex"),
        );
        let mut x = Vec::new();
        x.extend(2u16.to_le_bytes());
        x.extend(2u32.to_le_bytes());
        x.extend(0x15b1u16.to_le_bytes());
        x.extend(resource(
            10,
            2,
            1,
            48,
            primase_foundations::hash::joaat(b"megashader_locals"),
            0,
        ));
        x.extend(resource(0, 0, 1, 0, t, t));
        x.extend(constant(9, 0, a, primase_foundations::hash::joaat(b"fresnel")));
        x.extend(constant(9, 28, b, b));
        x.extend(b"megashader_locals\0specularFresnel\0bumpiness\0DiffuseTex\0");
        x
    }

    #[test]
    fn a_table_worked_by_hand_reads_field_by_field() {
        let t = parse(&table()).unwrap();
        assert_eq!(
            (t.resources.len(), t.constants.len(), t.unknown_6),
            (2, 2, 0x15b1)
        );
        let cb = t.resources[0];
        assert_eq!(
            (cb.register, cb.space, cb.size, cb.constants, cb.class),
            (10, 1, 48, 2, 2)
        );
        assert_eq!(cb.unknown_8, [1, 2, 3, 4, 5, 6]);
        assert_eq!(t.constants_of(0).len(), 2);
        assert!(t.constants_of(1).is_empty());
        assert_eq!(
            (
                t.constants[0].offset,
                t.constants[0].kind(),
                t.constants[1].offset
            ),
            (0, 9, 28)
        );
        assert_eq!(
            t.names,
            ["megashader_locals", "specularFresnel", "bumpiness", "DiffuseTex"]
        );
        // a hash names its name in the name's own case or lowercase
        assert_eq!(t.name_of(t.constants[0].name), Some("specularFresnel"));
        assert_eq!(t.name_of(t.resources[1].name), Some("DiffuseTex"));
        assert_eq!(t.name_of(0), None);
    }

    #[test]
    fn constants_follow_the_names_not_the_records() {
        // a buffer "late" listed before one "early": the records of the one whose names come first are first
        let (early, late) = (
            primase_foundations::hash::joaat(b"early"),
            primase_foundations::hash::joaat(b"late"),
        );
        let mut x = Vec::new();
        x.extend(2u16.to_le_bytes());
        x.extend(3u32.to_le_bytes());
        x.extend(0u16.to_le_bytes());
        x.extend(resource(9, 2, 0, 64, late, late));
        x.extend(resource(8, 1, 1, 16, early, early));
        x.extend(constant(12, 0, early, early));
        x.extend(constant(12, 0, late, late));
        x.extend(constant(12, 16, late, late));
        x.extend(b"a\0early\0b\0c\0late\0");
        let t = parse(&x).unwrap();
        assert_eq!(t.constants_of(1).len(), 1);
        assert_eq!(t.constants_of(1)[0].offset, 0);
        assert_eq!(t.constants_of(0).len(), 2);
        assert_eq!(t.constants_of(0)[1].offset, 16);
    }

    #[test]
    fn what_does_not_fit_is_refused_with_where() {
        let good = table();
        assert!(parse(&good[..5]).is_err());
        // counts the bytes cannot hold
        let mut b = good.clone();
        b[2] = 200;
        assert!(parse(&b).unwrap_err().to_string().contains("constants in"));
        // resources that own another number of constants than the header says
        let mut b = good.clone();
        b[8 + 4] = 1;
        assert!(parse(&b).unwrap_err().to_string().contains("own 1 constants"));
        // names that do not end in a NUL, or are not ASCII
        let mut b = good.clone();
        b.pop();
        assert!(parse(&b).unwrap_err().to_string().contains("NUL"));
        let mut b = good;
        let n = b.len();
        b[n - 3] = 0xC3;
        assert!(parse(&b).unwrap_err().to_string().contains("ASCII"));
    }

    /// An effect's record: noise, a parameter record naming `specularFresnel` `Fresnel`, an engine record (second
    /// name repeated: not returned), a record whose name no program has, and one of another type byte.
    #[test]
    fn an_effects_parameter_records_are_found_by_their_shape() {
        let (old, new) = (
            primase_foundations::hash::joaat(b"specularfresnel"),
            primase_foundations::hash::joaat(b"fresnel"),
        );
        let mut rest = vec![0xAB; 12];
        rest.extend(constant(0x0409, 0, old, new));
        rest.extend(constant(9, 4, 0x1111_1111, 0x1111_1111));
        rest.extend(constant(9, 8, 0x2222_2222, 0x3333_3333));
        rest.extend(constant(12, 16, old, 0x4444_4444));
        rest.extend([0xCD; 6]);
        let known = BTreeMap::from([(old, BTreeSet::from([9u8]))]);
        let found = effect_parameters(&rest, &known);
        assert_eq!(
            found,
            [EffectParameter {
                flags: 0x0409,
                offset: 0,
                id: 77,
                name: old,
                second_name: new
            }]
        );
    }
}
