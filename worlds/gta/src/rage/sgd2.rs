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
//!     tables, kept as stored (the state and technique tables of [`crate::rage::sgd2_effect`] among them)
//!     'proplst', NUL
//!     u32 count, then count sections: u32 size, size bytes (the root signature tables, kept as stored)
//!     u32 size, size bytes: the name block, the NUL-terminated names of parameters and techniques (it starts with an
//!         empty name and ends with a NUL)
//! ```
//!
//! The size of an effect record is not stored anywhere found, but its end is structural: a record ends with its
//! name block, which follows the `proplst` marker of the root signature table and the sections after it, each
//! sized. The reader finds the marker in the bytes after the lists (the state tables before it are not
//! walked), takes the sections and the name block by their sizes, and the next record starts there, so no record
//! start is searched for. The result is held to the declared effect count and the end of the file: a record without
//! its marker, whose sections or name block do not fit, or a file that does not end with the last record is refused.
//! Both files seen (`sga_win32_60_final.awc` with 388 effects, and its `_init` with 9) read to their last byte this way.
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

/// The marker of the root signature table, which the sections and the name block follow.
const ROOT_SIGNATURES: &[u8] = b"proplst\0";

/// One effect record, from its name to the end of its name block (see the module documentation).
fn effect<'a>(c: &mut Cursor<'a>, groups: &[Vec<Program<'a>>; 8]) -> Result<Effect<'a>> {
    let name = c.name()?;
    let word = c.u32()?;
    let mut programs: [Vec<u32>; 6] = Default::default();
    for (list, &g) in programs.iter_mut().zip(EFFECT_LISTS.iter()) {
        let n = c.u32()? as usize;
        if n > c.data.len().saturating_sub(c.at).saturating_div(4) {
            return c.fail(format!("{name}: a list of {n} programs"));
        }
        let available = groups.get(g).map_or(0, Vec::len);
        for _ in 0..n {
            let index = c.u32()?;
            if index as usize >= available {
                return c.fail(format!("{name}: program {index} of a group of {available}"));
            }
            list.push(index);
        }
    }
    let rest_start = c.at;
    let tail = c.data.get(rest_start..).unwrap_or_default();
    let marker = tail
        .windows(ROOT_SIGNATURES.len())
        .position(|w| w == ROOT_SIGNATURES)
        .ok_or_else(|| {
            invalid(
                "sgd2",
                rest_start as u64,
                format!("{name}: no root signature table"),
            )
        })?;
    c.at = rest_start
        .saturating_add(marker)
        .saturating_add(ROOT_SIGNATURES.len());
    let sections = c.u32()? as usize;
    // each section takes at least its size word
    if sections > c.data.len().saturating_sub(c.at).saturating_div(4) {
        return c.fail(format!("{name}: {sections} root signature sections"));
    }
    for _ in 0..sections {
        let size = c.u32()? as usize;
        c.take(size)?;
    }
    let size = c.u32()? as usize;
    let names = c.take(size)?;
    if names.first() != Some(&0) || names.last() != Some(&0) {
        return c.fail(format!(
            "{name}: a name block that does not start and end with a NUL"
        ));
    }
    let rest = c
        .data
        .get(rest_start..c.at)
        .ok_or_else(|| invalid("sgd2", rest_start as u64, "an effect without its rest"))?;
    Ok(Effect {
        name,
        word,
        programs,
        rest,
    })
}

/// Read a shader database. The whole file is consumed.
pub fn parse(data: &[u8]) -> Result<Sgd2<'_>> {
    let mut c = Cursor { data, at: 0 };
    if c.take(4)? != MAGIC {
        return invalid_magic(data);
    }
    let mut groups: Vec<Vec<Program<'_>>> = Vec::with_capacity(8);
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
    // each record takes more than a name, a word and six counts
    if declared > data.len().saturating_sub(c.at).saturating_div(40) {
        return c.fail(format!("{declared} effects in {} bytes", data.len()));
    }
    let mut effects = Vec::with_capacity(declared);
    for _ in 0..declared {
        effects.push(effect(&mut c, &groups)?);
    }
    if c.at != data.len() {
        return c.fail(format!(
            "{} bytes after the last of {declared} effects",
            data.len().saturating_sub(c.at)
        ));
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
            tail(&mut b);
        }
        b
    }
    /// What follows an effect's lists: a table kept as stored, the root signature marker, two sections and the name block.
    fn tail(b: &mut Vec<u8>) {
        b.extend([0u8, 0, 1, 5, 0x40, 0x41]);
        b.extend(ROOT_SIGNATURES);
        b.extend(2u32.to_le_bytes());
        for section in [&[1u8, 2, 3][..], &[][..]] {
            b.extend(u32::try_from(section.len()).unwrap().to_le_bytes());
            b.extend(section);
        }
        b.extend(7u32.to_le_bytes());
        b.extend(b"\0Param\0");
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
        // a record ends with its name block, so a cut anywhere, the last record's tail included, is detected
        for n in 0..good.len() {
            assert!(parse(&good[..n]).is_err(), "a database cut at {n}");
        }
        // and so is anything after the last record
        let mut more = good.clone();
        more.extend([0u8; 3]);
        assert!(
            parse(&more)
                .unwrap_err()
                .to_string()
                .contains("3 bytes after the last of 2 effects")
        );
        let mut tag = good.clone();
        tag[4 + 4 + 1 + "P0_0_Wrapped".len() + 1] = 0x33;
        assert!(parse(&tag).unwrap_err().to_string().contains("tag 0x33"));
        let mut wrong = good.clone();
        wrong[0] = b'X';
        assert!(parse(&wrong).unwrap_err().to_string().contains("not SGD2"));
    }

    /// A record's end is where its name block ends, whatever its name looks like: an effect named with two characters
    /// (the `_init` database has one) reads, and a record whose marker, sections or name block are wrong is refused
    /// instead of taking the next record's.
    #[test]
    fn a_record_ends_with_its_name_block() {
        let mut bytes = database();
        let first = bytes.windows(6).position(|w| w == b"alpha\0").unwrap();
        // rename the first effect to "im" (length byte, two characters, NUL), keeping the file's length by dropping bytes
        let mut renamed = bytes[..first - 1].to_vec();
        renamed.extend([3, b'i', b'm', 0]);
        renamed.extend(&bytes[first + "alpha\0".len()..]);
        let db = parse(&renamed).unwrap();
        assert_eq!((db.effects[0].name, db.effects[1].name), ("im", "beta_x"));
        // no marker in the first record: it would take the second's, and the count is then one short
        let marker = bytes.windows(8).position(|w| w == ROOT_SIGNATURES).unwrap();
        bytes[marker] = b'P';
        assert!(parse(&bytes).is_err());
        // a name block that is not a string table
        let mut blocks = database();
        let end = blocks.len();
        blocks[end - 1] = b'x';
        assert!(
            parse(&blocks)
                .unwrap_err()
                .to_string()
                .contains("a name block that does not start and end with a NUL")
        );
        // more sections than the file has bytes for
        let mut sections = database();
        let at = sections.windows(8).position(|w| w == ROOT_SIGNATURES).unwrap() + 8;
        sections[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(
            parse(&sections)
                .unwrap_err()
                .to_string()
                .contains("root signature sections")
        );
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
