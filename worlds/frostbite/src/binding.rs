//! Binding sets and pass bindings (types [`BINDING_SET`] and [`PASS_BINDING`], named `expressionshader/bindingset/<id>`
//! and `.../passbinding/<id>`): the parameters a shader reads and where. A set holds groups; a group is either the
//! layout of one constant buffer (each parameter at a byte offset) or a list of resources (each at an index). The
//! parameters are the same (name hash, type hash, count) records a ShaderBlockDepot block holds ([`crate::depot`]), so
//! a material's block names the values the group's parameters read.
//!
//! Layout (relocatable; the 16-byte metadata holds the payload size, the relocation size and the resource's id):
//!
//! ```text
//! header (0x20)   a set: u32 group count, u32 0x20, u64 0, u64 its id (the metadata's), u32 1, u32 parameter count
//!                 a pass binding: u32 version (1 or 0x101), u32 group count, u32 0x20, u32 0, u64 its id, u64 0
//! group (0x40 each, from 0x20), then an optional block of 0x20 bytes per extra pointer some sets carry (kept)
//!   +0x00 u64 name hash        +0x08 u32 size or count      +0x0c 4 flag bytes
//!   +0x10 u16 parameter count  +0x12 three more u16 (kept)  +0x18 u64 0
//!   +0x20 u64 four offsets: the parameters of the whole set, this group's parameters, this group's entry table, the
//!         end of the tables (where the relocation list starts)
//! parameters      16 bytes each: u64 name hash, u32 type hash, u16 count, u16 extra
//! entry tables    8 bytes per parameter of a group: u32 offset or index, u16 marker (0xffff on every entry of a
//!                 constant-buffer group; 0 or 1 elsewhere), u16 kind
//! trailer         bytes after the last entry table (0 to 40 seen), kept as stored
//! relocations     u32 offsets of every pointer in the blob (the groups' four each, and the extra block's), then one or
//!                 two words (4, 8 or other values), kept as stored
//! ```
//!
//! For a constant-buffer group an entry's offset is the parameter's byte offset in the buffer and its kind its size in
//! bytes times four; the group's size field is the buffer's size rounded up to 16. For a resource group the first
//! field is the resource's index and the kind a code kept as stored. Everything is kept as stored; the reader walks to
//! the exact end of the blob and refuses what does not fit.

use crate::{Rd, Result, bad, capacity};

const W: &str = "binding set";
/// ExpressionShaderBindingSet.
pub const BINDING_SET: u32 = 0xA19B_025D;
/// ExpressionShaderPassBinding.
pub const PASS_BINDING: u32 = 0x589B_B36E;
const HEADER: usize = 0x20;
const GROUP: usize = 0x40;

/// One parameter of a group: the same record a depot block's parameter starts with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Param {
    pub name: u64,
    pub ty: u32,
    pub count: u16,
    pub extra: u16,
}

/// A parameter's entry: where it lies (a constant buffer's byte offset, a resource group's index) and its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub at: u32,
    pub marker: u16,
    pub kind: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub name: u64,
    /// The word at +0x08: the buffer size rounded up to 16 for a constant-buffer group, a count for a resource group.
    pub size: u32,
    pub flags: [u8; 4],
    /// The u16 words at +0x12, +0x14, +0x16 and +0x18's low word, kept as stored.
    pub words: [u16; 4],
    pub params: Vec<Param>,
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Set {
    /// The resource's own id, as the blob states it.
    pub id: u64,
    /// A set's word at 0x18 (1) and 0x1c (its parameter count); a pass binding has neither (0, 0).
    pub head: [u32; 2],
    /// Whether it is a pass binding, and the version word it states (1 or 0x101); a set has none (0).
    pub pass: bool,
    pub version: u32,
    pub groups: Vec<Group>,
    /// The bytes between the last entry table and the relocation list.
    pub trailer: Vec<u8>,
    /// The bytes between the group descriptors and the first parameter (an extra block some sets carry).
    pub lead: Vec<u8>,
    /// The relocation list as stored.
    pub relocations: Vec<u32>,
}

/// The size of a constant-buffer group: the end of its last parameter, from its entries.
impl Group {
    pub fn buffer_end(&self) -> u32 {
        (self.entries.iter())
            .filter(|e| e.at != u32::MAX)
            .map(|e| e.at + u32::from(e.kind) / 4)
            .max()
            .unwrap_or(0)
    }
}

/// Read a binding set or a pass binding from its payload and metadata. Refused: a size or relocation size that does
/// not match, an id that is not the metadata's, a group whose parameters or entries lie outside the payload or
/// overlap another's, a pointer list that is not the groups' four pointers each.
pub fn read(rtype: u32, d: &[u8], meta: &[u8; 16]) -> Result<Set> {
    let m = Rd::new(meta, W);
    let (size, reloc, id) = (m.u32le(0)? as usize, m.u32le(4)? as usize, m.u64le(8)?);
    if size.checked_add(reloc) != Some(d.len()) {
        return Err(bad(
            W,
            0,
            format!("size {size} + relocations {reloc} != {}", d.len()),
        ));
    }
    let r = Rd::new(d, W);
    let pass = match rtype {
        PASS_BINDING => true,
        BINDING_SET => false,
        t => return Err(bad(W, 0, format!("type {t:#010x} is not a binding set"))),
    };
    let first = r.u32le(0)?;
    let (count, own, head, version) = if pass {
        (r.u32le(4)? as usize, r.u64le(0x10)?, [0, 0], first)
    } else {
        (
            first as usize,
            r.u64le(0x10)?,
            [r.u32le(0x18)?, r.u32le(0x1c)?],
            0,
        )
    };
    if own != id {
        return Err(bad(W, 0x10, format!("id {own:#x}, the metadata's {id:#x}")));
    }
    if (if pass { r.u32le(8)? } else { r.u32le(4)? }) != 0x20 {
        return Err(bad(W, 4, "groups do not start at 0x20"));
    }
    let tables_end = size;
    let mut groups = Vec::with_capacity(capacity(W, count, GROUP, size.saturating_sub(HEADER))?);
    let mut end_of_data = HEADER + GROUP * count;
    let mut pointers = Vec::new();
    let mut first_params = usize::MAX;
    for g in 0..count {
        let at = HEADER + GROUP * g;
        let n = usize::from(r.u16le(at + 0x10)?);
        let (all, own_params, table, end) = (
            r.u64le(at + 0x20)? as usize,
            r.u64le(at + 0x28)? as usize,
            r.u64le(at + 0x30)? as usize,
            r.u64le(at + 0x38)? as usize,
        );
        pointers.extend([at + 0x20, at + 0x28, at + 0x30, at + 0x38]);
        first_params = first_params.min(own_params).min(all);
        if end > size {
            return Err(bad(
                W,
                at + 0x38,
                format!("tables end at {end:#x} in a payload of {size:#x}"),
            ));
        }
        let params_end = own_params.checked_add(16 * n);
        let table_end = table.checked_add(8 * n);
        if params_end.is_none_or(|e| e > table) || table_end.is_none_or(|e| e > end.min(tables_end)) {
            return Err(bad(
                W,
                at,
                format!("group {g}: {n} parameters at {own_params:#x} and entries at {table:#x} do not fit"),
            ));
        }
        if own_params < HEADER + GROUP * count {
            return Err(bad(
                W,
                at,
                format!("group {g}: parameters at {own_params:#x} before the set's"),
            ));
        }
        let params = (0..n)
            .map(|i| {
                let p = own_params + 16 * i;
                Ok(Param {
                    name: r.u64le(p)?,
                    ty: r.u32le(p + 8)?,
                    count: r.u16le(p + 12)?,
                    extra: r.u16le(p + 14)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let entries = (0..n)
            .map(|i| {
                let e = table + 8 * i;
                Ok(Entry {
                    at: r.u32le(e)?,
                    marker: r.u16le(e + 4)?,
                    kind: r.u16le(e + 6)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        end_of_data = end_of_data.max(table_end.unwrap_or(0));
        groups.push(Group {
            name: r.u64le(at)?,
            size: r.u32le(at + 8)?,
            flags: r.arr(at + 12)?,
            words: [
                r.u16le(at + 0x12)?,
                r.u16le(at + 0x14)?,
                r.u16le(at + 0x16)?,
                r.u16le(at + 0x18)?,
            ],
            params,
            entries,
        });
    }
    // the relocation list: the offsets of the blob's pointers and a word or two; the groups' pointers are among them
    let words = (0..reloc / 4)
        .map(|i| r.u32le(size + 4 * i))
        .collect::<Result<Vec<_>>>()?;
    if !reloc.is_multiple_of(4) || reloc < 4 * (pointers.len() + 1) {
        return Err(bad(
            W,
            size,
            format!("{reloc} relocation bytes for {} groups", groups.len()),
        ));
    }
    if let Some(p) = pointers.iter().find(|&&p| !words.contains(&(p as u32))) {
        return Err(bad(
            W,
            size,
            format!("the relocation list lacks the pointer at {p:#x}"),
        ));
    }
    if end_of_data > size || size - end_of_data > 64 {
        return Err(bad(
            W,
            end_of_data,
            format!(
                "{} bytes between the last entry table and the relocations",
                size as i64 - end_of_data as i64
            ),
        ));
    }
    Ok(Set {
        id,
        head,
        pass,
        version,
        groups,
        trailer: r.bytes(end_of_data, size - end_of_data)?.to_vec(),
        lead: r
            .bytes(
                HEADER + GROUP * count,
                first_params.saturating_sub(HEADER + GROUP * count),
            )?
            .to_vec(),
        relocations: words,
    })
}

/// A permutation's binding sets, by the slot the shared data names them in, and the rule that says which group a
/// shader's register reads.
///
/// ```text
/// constant buffers   b0 -> set 1, group 0        b1 -> set 0, group 0        b4 -> pass binding, group 0
/// resources          t0..t31 -> set 1, group 1, the entry whose index is the register
///                    t32..t63 -> set 0, group 1, the entry whose index is the register - 32
///                    t64 and up -> the pass binding, group 1, the entry whose index is the register - 64
/// ```
///
/// Evidence (`tests/binding.rs`, the 2,931 raster permutations of the game configuration bundle and the programs
/// they run, counting each declared register in space 0): the constant buffers at b4 have the size the pass binding's
/// group 0 lays out in 5,707 of 5,707; at b1 the size set 0's group 0 lays out in 2,867 of 2,887; at b0 the size set
/// 1's group 0 lays out in 4,069 of 4,613 (the other 544 are not told apart: b0 may have another source there); the
/// resources at t0 and above have an entry by the rule in 31,589 of 31,639 (99.8%). A program's b2 (1,866 declared),
/// b3 (38) and samplers (4,277) have no source in these sets.
#[derive(Clone, Copy, Debug)]
pub struct Bindings<'a> {
    pub pass: Option<&'a Set>,
    pub sets: [Option<&'a Set>; 3],
}

impl<'a> Bindings<'a> {
    /// The group a constant buffer at `register` (space 0) reads, if the rule has one and the set is there.
    pub fn constant_buffer(&self, register: u32) -> Option<&'a Group> {
        let set = match register {
            0 => self.sets[1],
            1 => self.sets[0],
            4 => self.pass,
            _ => None,
        }?;
        set.groups.first()
    }

    /// The group a shader resource at `register` (space 0) reads and the index of its parameter in the group: the
    /// parameter's record, `params[i]` and `entries[i]`.
    pub fn resource(&self, register: u32) -> Option<(&'a Group, usize)> {
        let (set, base) = match register {
            0..=31 => (self.sets[1], 0),
            32..=63 => (self.sets[0], 32),
            _ => (self.pass, 64),
        };
        let group = set?.groups.get(1)?;
        let want = register - base;
        let i = group.entries.iter().position(|e| e.at == want)?;
        Some((group, i))
    }

    /// Where the parameter named `name` (the hash a depot block's parameter carries) lies in the permutation's own
    /// sets (the pass binding's parameters are the engine's, not a material's): the first of sets 0, 1 and 2 that
    /// names it, in groups 0 and 1 and so on.
    ///
    /// A material's depot block fills its parameters by name: on a sample of 39 materials of `Win32/items.toc` (the
    /// permutations their programs run, `tests/binding.rs`) every parameter of set 0, both groups (34,756 of the
    /// constant-buffer group, 4,052 of the resource group), is named by the material's block with the same type and
    /// count; of set 1's resource group 1,824 of 3,888 are, of set 2's group 0 1,152 of 4,548, and none of set 1's
    /// group 0 or the pass binding's (the engine fills those).
    pub fn parameter(&self, name: u64) -> Option<Slot<'a>> {
        self.sets.iter().enumerate().find_map(|(set, s)| {
            s.as_ref()?.groups.iter().enumerate().find_map(|(group, g)| {
                let index = g.params.iter().position(|p| p.name == name)?;
                Some(Slot {
                    set,
                    group,
                    index,
                    param: &g.params[index],
                    entry: g.entries.get(index)?,
                })
            })
        })
    }
}

/// A parameter's place in a permutation's sets ([`Bindings::parameter`]): the set (0 to 2), the group in it, the index in
/// the group, the parameter's record and its entry (a constant buffer's byte offset, a resource group's index).
#[derive(Clone, Copy, Debug)]
pub struct Slot<'a> {
    pub set: usize,
    pub group: usize,
    pub index: usize,
    pub param: &'a Param,
    pub entry: &'a Entry,
}
