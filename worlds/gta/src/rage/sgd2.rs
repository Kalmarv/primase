//! GTA V Enhanced's shader database (`SGD2`, `common/shaders/fxdb/sga_win32_60_final.awc` and its `_init` sibling):
//! every shader program of the game as a Direct3D 12 (DXIL) container, in groups by stage, and the effects that
//! name the programs each uses. No I/O: the reader hands over bytes. Nothing is copied: programs and effects borrow
//! the file.
//!
//! The layout below was worked out from the file itself (the earlier notes only said where the containers are), and
//! the reader holds it to the whole file: it reads to the last byte and refuses what does not fit.
//!
//! ```text
//! 'SGD2'
//! 8 groups, in the order vertex, pixel, geometry, domain, hull, compute, and two more (empty in both files):
//!     u32 count
//!     count programs:
//!         u8 length, name (length bytes, the last a NUL)
//!         u8 tag (0x32, 0x41 or 0x42; see TAGS)
//!         u32 size, the container ('DXBC' magic; its own header repeats the size at +24)
//!         152 bytes of a table whose first eight bytes look like a hash (kept as stored)
//!         u32 n, n bytes (further tables, kept as stored)
//! u32 effect count
//! effect records:
//!     u8 length, name (NUL-terminated)
//!     u32 (not a record size: the offset of something inside the record)
//!     six lists, in the group order without the two empty ones: u32 count then count u32 indices into the group's
//!     programs (vertex, pixel, geometry, domain, hull, compute)
//!     more tables and a string table of parameter names, kept as stored
//! ```
//!
//! The size of an effect record is not stored anywhere found: a record ends where the next one's length byte and
//! name begin. The reader finds that by the shape of a record start (a length byte equal to the printable
//! identifier that follows it plus its NUL) and holds the result to the declared effect count and the end of the
//! file, so a file in which the shape also occurs inside a record is refused, not misread.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::rage::fxc::ProgramStage;
use crate::{Result, invalid};

/// The magic `SGD2`.
pub const MAGIC: [u8; 4] = *b"SGD2";
/// The tag bytes a program carries in the file seen: `2` on 12,291 of its 12,405 programs, `A` on 44 and `B` on 70.
/// What the letter says is not known (the programs it separates are laid out alike).
const TAGS: [u8; 3] = [0x32, 0x41, 0x42];
/// Bytes of the fixed table after a program's container.
const FIXED_TABLE: usize = 152;
/// The stage of each of the eight groups; the last two are not known and must be empty.
pub const GROUP_STAGES: [Option<ProgramStage>; 8] = [
    Some(ProgramStage::Vertex),
    Some(ProgramStage::Pixel),
    Some(ProgramStage::Geometry),
    Some(ProgramStage::Domain),
    Some(ProgramStage::Hull),
    Some(ProgramStage::Compute),
    None,
    None,
];
/// The groups an effect lists programs of, in the order of its lists.
pub const EFFECT_LISTS: [usize; 6] = [0, 1, 2, 3, 4, 5];

/// One shader program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Program<'a> {
    pub name: &'a str,
    pub tag: u8,
    /// The Direct3D 12 container, as stored.
    pub container: &'a [u8],
    /// The fixed table after the container, as stored.
    pub table: &'a [u8],
    /// The further tables after it, as stored.
    pub tables: &'a [u8],
}

/// One effect: the programs it uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Effect<'a> {
    pub name: &'a str,
    /// The word after the name (not a size).
    pub word: u32,
    /// The programs of each stage, as indices into that stage's group: vertex, pixel, geometry, domain, hull,
    /// compute.
    pub programs: [Vec<u32>; 6],
    /// The rest of the record, as stored.
    pub rest: &'a [u8],
}

/// The shader database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sgd2<'a> {
    /// The eight groups, in [`GROUP_STAGES`] order.
    pub groups: [Vec<Program<'a>>; 8],
    pub effects: Vec<Effect<'a>>,
}

impl<'a> Sgd2<'a> {
    /// The programs of a stage.
    pub fn programs(&self, stage: ProgramStage) -> &[Program<'a>] {
        GROUP_STAGES
            .iter()
            .position(|s| *s == Some(stage))
            .and_then(|g| self.groups.get(g))
            .map_or(&[], Vec::as_slice)
    }
}

struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn fail<T>(&self, detail: impl ToString) -> Result<T> {
        Err(invalid("sgd2", self.at as u64, detail))
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n);
        match end.and_then(|e| self.data.get(self.at..e).map(|s| (e, s))) {
            Some((e, s)) => {
                self.at = e;
                Ok(s)
            }
            None => self.fail(format!("{n} bytes past the {} there are", self.data.len())),
        }
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?.first().copied().unwrap_or(0))
    }
    fn u32(&mut self) -> Result<u32> {
        let b: [u8; 4] = self.take(4)?.try_into().unwrap_or([0; 4]);
        Ok(u32::from_le_bytes(b))
    }
    /// A length byte, then that many bytes of name whose last is a NUL.
    fn name(&mut self) -> Result<&'a str> {
        let start = self.at;
        let length = usize::from(self.u8()?);
        let bytes = self.take(length)?;
        let Some((0, text)) = bytes.split_last().map(|(l, t)| (*l, t)) else {
            self.at = start;
            return self.fail("a name that does not end in a NUL");
        };
        if text.is_empty() || !text.iter().all(|b| b.is_ascii_graphic()) {
            self.at = start;
            return self.fail("a name that is not printable ASCII");
        }
        std::str::from_utf8(text).or_else(|_| self.fail("a name that is not ASCII"))
    }
}

fn program<'a>(c: &mut Cursor<'a>, group: usize) -> Result<Program<'a>> {
    let name = c.name()?;
    let tag = c.u8()?;
    if !TAGS.contains(&tag) {
        return c.fail(format!("{name}: tag {tag:#04x}, not one of {TAGS:02x?}"));
    }
    let size = c.u32()? as usize;
    let container = c.take(size)?;
    let magic = container.get(..4);
    let stored = container
        .get(24..28)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map(u32::from_le_bytes);
    if magic != Some(b"DXBC") || stored != u32::try_from(size).ok() {
        return c.fail(format!(
            "{name} (group {group}): not a container of its stated size"
        ));
    }
    let table = c.take(FIXED_TABLE)?;
    let n = c.u32()? as usize;
    let tables = c.take(n)?;
    Ok(Program {
        name,
        tag,
        container,
        table,
        tables,
    })
}

/// Whether a record starts at `p`: a length byte, an identifier of that many bytes less a NUL, and a nonzero word.
fn record_start(data: &[u8], p: usize) -> Option<usize> {
    let length = usize::from(*data.get(p)?);
    let name = data.get(p.checked_add(1)?..p.checked_add(1)?.checked_add(length)?)?;
    let (nul, text) = name.split_last().map(|(l, t)| (*l, t))?;
    let ok = nul == 0
        && text.len() >= 3
        && text
            .first()
            .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
        && text.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_');
    ok.then_some(length)
}

/// Read a shader database. The whole file is consumed.
pub fn parse(data: &[u8]) -> Result<Sgd2<'_>> {
    let mut c = Cursor { data, at: 0 };
    if c.take(4)? != MAGIC {
        return invalid_magic(data);
    }
    let mut groups: Vec<Vec<Program<'_>>> = Vec::new();
    for (g, stage) in GROUP_STAGES.iter().enumerate() {
        let count = c.u32()? as usize;
        if stage.is_none() && count != 0 {
            return c.fail(format!("group {g} has {count} programs: its stage is not known"));
        }
        // each program takes more than a name's worth of bytes: refuse a count the file cannot hold
        if count > data.len().saturating_div(40) {
            return c.fail(format!("group {g}: {count} programs in {} bytes", data.len()));
        }
        let mut list = Vec::with_capacity(count);
        for _ in 0..count {
            list.push(program(&mut c, g)?);
        }
        groups.push(list);
    }
    let groups: [Vec<Program<'_>>; 8] = groups
        .try_into()
        .map_err(|_| invalid("sgd2", c.at as u64, "eight groups"))?;

    let declared = c.u32()? as usize;
    // each record: its name and word, the lists, the rest
    let mut starts = Vec::new();
    let mut p = c.at;
    while p < data.len() {
        if let Some(length) = record_start(data, p) {
            starts.push(p);
            // the next record starts after at least this name, word and six counts
            p = p
                .saturating_add(1)
                .saturating_add(length)
                .saturating_add(4)
                .saturating_add(24);
        } else {
            p = p.saturating_add(1);
        }
    }
    if starts.len() != declared {
        return c.fail(format!(
            "{declared} effects declared, {} record starts found",
            starts.len()
        ));
    }
    if starts.first().copied() != Some(c.at) && declared != 0 {
        return c.fail("bytes between the effect count and the first effect");
    }
    let mut effects = Vec::with_capacity(declared);
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i.saturating_add(1)).copied().unwrap_or(data.len());
        let record = data
            .get(start..end)
            .ok_or_else(|| invalid("sgd2", start as u64, "an effect past the file"))?;
        let mut r = Cursor { data: record, at: 0 };
        let name = r.name().map_err(|e| invalid("sgd2", start as u64, e))?;
        let word = r.u32()?;
        let mut programs: [Vec<u32>; 6] = Default::default();
        for (list, &g) in programs.iter_mut().zip(EFFECT_LISTS.iter()) {
            let n = r.u32()? as usize;
            if n > record.len().saturating_div(4) {
                return r.fail(format!("{name}: a list of {n} programs"));
            }
            let available = groups.get(g).map_or(0, Vec::len);
            for _ in 0..n {
                let index = r.u32()?;
                if index as usize >= available {
                    return r.fail(format!("{name}: program {index} of a group of {available}"));
                }
                list.push(index);
            }
        }
        let rest = record
            .get(r.at..)
            .ok_or_else(|| invalid("sgd2", start as u64, "an effect without its rest"))?;
        effects.push(Effect {
            name,
            word,
            programs,
            rest,
        });
    }
    Ok(Sgd2 { groups, effects })
}

fn invalid_magic<T>(data: &[u8]) -> Result<T> {
    Err(invalid(
        "sgd2",
        0,
        format!("magic {:02x?}, not SGD2", data.get(..4).unwrap_or_default()),
    ))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
mod tests {
    use super::*;

    fn container() -> Vec<u8> {
        let mut c = b"DXBC".to_vec();
        c.extend([0u8; 20]);
        c.extend(40u32.to_le_bytes());
        c.extend([0u8; 12]);
        c
    }
    fn name(out: &mut Vec<u8>, s: &str) {
        out.push(u8::try_from(s.len() + 1).unwrap());
        out.extend(s.as_bytes());
        out.push(0);
    }
    fn program(out: &mut Vec<u8>, n: &str, trailer: usize) {
        name(out, n);
        out.push(TAGS[0]);
        let c = container();
        out.extend(u32::try_from(c.len()).unwrap().to_le_bytes());
        out.extend(c);
        out.extend([7u8; FIXED_TABLE]);
        out.extend(u32::try_from(trailer).unwrap().to_le_bytes());
        out.extend(vec![9u8; trailer]);
    }
    /// A database by hand: two vertex programs, one pixel program, one compute program, two effects.
    fn database() -> Vec<u8> {
        let mut b = b"SGD2".to_vec();
        let counts = [2u32, 1, 0, 0, 0, 1, 0, 0];
        for (g, n) in counts.iter().enumerate() {
            b.extend(n.to_le_bytes());
            for k in 0..*n {
                program(&mut b, &format!("P{g}_{k}_Wrapped"), (k as usize) * 5 + 1);
            }
        }
        b.extend(2u32.to_le_bytes());
        for (effect, vs, ps, cs) in [
            ("alpha", vec![0u32, 1], vec![0u32], vec![]),
            ("beta_x", vec![1], vec![], vec![0u32]),
        ] {
            name(&mut b, effect);
            b.extend(1234u32.to_le_bytes());
            for list in [&vs, &ps, &vec![], &vec![], &vec![], &cs] {
                b.extend(u32::try_from(list.len()).unwrap().to_le_bytes());
                for i in list {
                    b.extend(i.to_le_bytes());
                }
            }
            // a table and a string table, kept as stored
            b.extend([0u8, 0, 1, 5, 0x40, 0x41]);
            b.extend(b"\0Param\0");
        }
        b
    }

    #[test]
    fn a_database_written_by_hand_reads_back() {
        let bytes = database();
        let db = parse(&bytes).unwrap();
        assert_eq!(
            db.groups.iter().map(Vec::len).collect::<Vec<_>>(),
            [2, 1, 0, 0, 0, 1, 0, 0]
        );
        assert_eq!(db.programs(ProgramStage::Vertex)[1].name, "P0_1_Wrapped");
        assert_eq!(db.programs(ProgramStage::Vertex)[1].tables.len(), 6);
        assert_eq!(db.programs(ProgramStage::Compute)[0].name, "P5_0_Wrapped");
        assert!(
            db.programs(ProgramStage::Vertex)[0]
                .container
                .starts_with(b"DXBC")
        );
        assert_eq!(db.effects.len(), 2);
        assert_eq!(db.effects[0].name, "alpha");
        assert_eq!(db.effects[0].word, 1234);
        assert_eq!(db.effects[0].programs[0], [0, 1]);
        assert_eq!(db.effects[0].programs[1], [0]);
        assert_eq!(db.effects[1].programs[5], [0]);
        assert!(db.effects[0].rest.ends_with(b"\0Param\0"));
    }

    #[test]
    fn what_the_layout_does_not_hold_is_refused() {
        let good = database();
        // the last effect's rest runs to the end of the file, so a cut inside it is not detectable
        for n in 0..good.len() - "\u{0}\u{0}\u{1}\u{5}@A\0Param\0".len() {
            assert!(parse(&good[..n]).is_err(), "a database cut at {n}");
        }
        // (bytes after the last effect belong to its rest: a record's size is not stored)
        let mut tag = good.clone();
        tag[4 + 4 + 1 + "P0_0_Wrapped".len() + 1] = 0x33;
        assert!(parse(&tag).unwrap_err().to_string().contains("tag 0x33"));
        let mut wrong = good.clone();
        wrong[0] = b'X';
        assert!(parse(&wrong).unwrap_err().to_string().contains("not SGD2"));
    }

    #[test]
    fn an_effect_naming_a_program_that_is_not_there_is_refused() {
        let mut bytes = database();
        // alpha's first vertex program index 0 -> 7
        let at = bytes.windows(5).rposition(|w| w == b"alpha").unwrap() + "alpha\0".len() + 4 + 4;
        bytes[at] = 7;
        assert!(
            parse(&bytes)
                .unwrap_err()
                .to_string()
                .contains("program 7 of a group of 2")
        );
    }
}
