//! A BuildTable (`0x22ECBE63`, the CRC-32 of `BuildTable`) read to its last byte. The game's customization and settings
//! tables are BuildTables: `MAD_CHR_ICON_BUNDLE` lists the slot tables of a character (`MAD_CHR_SLOT_UBODY`, ...), a
//! slot table's rows name the part tables (`MAD_CHR_UBODY_001_U_1001`), a part table's properties name the meshes,
//! addon skeletons and `_Materials` tables of the part, and the `T###_V###` tables the variants. RRAssetStudio calls
//! the type a Selector; what a property *means* is its claim, and this reader keeps what the bytes state.
//!
//! **Layout** (every field in the registry's order, a class deriving from `ManagedObject` storing one byte first;
//! established on all 10,730 bodies of Riders Republic's forges, each read to its last byte):
//! - the `01` byte, a u32 column count, the columns;
//! - a column (`BuildColumn`, class `0x36839608`): u64 id and the class; an `ApplyPassBitField` (u64 id, class
//!   `0x913E9298`, a byte); a `PropertyPath` (id 0, class `0x3C00B2E0`, a u32 node count, the nodes, two bytes); three
//!   u32 (the first the column's number, 1 to the count); a pointer to a BuildTable (a tag byte and a u64: the table
//!   itself on 32,250 of the 32,289 columns); then the value the column declares: two u32 (1 and `0x2CECF817`) and a
//!   typed value;
//! - a node (`PropertyPathNode`, id 0, class `0x1A8750BA`): 41 bytes of fields as stored, then a pointer to a solver
//!   (`03` none; or `00`, a u64, the class and the solver's body, whose length is fixed by its class:
//!   `MaterialPropertyPathNodeSolver` 59, `VisualPropertyNodeSolver` 34, `ShaderConstantLayoutNodeSolver` 6);
//! - a typed value: a u32 class (the class a reference or link refers to), a u16, a u32 kind, a u16, then the bytes of
//!   the kind `kind & 0x3F`: scalars 1 to 64 bytes, a link (`0x12`) 9, a reference (`0x1C`) 10, an object or pointer
//!   (`0x13` to `0x16`) by its tag and class (the five classes the tables hold have fixed bodies, below);
//! - u32 row count, the rows; a row (`BuildRow`, class `0x348B28D6`): u64 id and the class, a pointer to a BuildTable,
//!   an f32 weight, a `BuildTag` (id 0, class `0xB332698E`, a u32 hash), a `BuildTags` (u64 id, class `0x11BD5345`, a
//!   u32 count of tags, 0 on every row), a u32 count of properties and each as a u32 column number and a typed value;
//! - four bytes (`00 01 00 00` on 8,663 tables, 0 on 2,065, two others once), and a u32 count of `DynamicResources`
//!   (u64 id, class `0xDED3E212`, a u32 count, a link each).
//!
//! The object classes the property values hold (all fixed-length): `SoundSwitch` `0x731CE65C` (body 33 bytes),
//! `KinoRTCPSetterEnum` `0x2D7B69F6` (19), `KinoRTCPSetterBasicType` `0xA9CC15E8` (28), `KinoReplaceIdentifier`
//! `0xD9606976` (28), `KinoLoadTagIdentifier` `0x1B679142` (28), measured on every occurrence (the registry lists no
//! variable-length field in them). A class not here is refused.
//!
//! Census (Riders Republic build 25394987, tests/build_table.rs): 10,730 tables, 32,289 columns, 22,728 rows, 43,035
//! properties, 14,632 `DynamicResources` holding 24,709 links.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::components::{
    BuildColumn, BuildLink, BuildProperty, BuildResources, BuildRow, BuildValue, PathNode, StoredBuildTable,
};
use crate::read::fields::{BodyError, LE, body_error, count, expect};
use crate::read::reference::{self, Reference};
use primase_bytes::{Field, View};

/// A record type: the CRC-32 of `BuildTable`.
pub const BUILD_TABLE: u32 = 0x22EC_BE63;
const BUILD_COLUMN: u32 = 0x3683_9608;
const APPLY_PASS_BIT_FIELD: u32 = 0x913E_9298;
const PROPERTY_PATH: u32 = 0x3C00_B2E0;
const PROPERTY_PATH_NODE: u32 = 0x1A87_50BA;
const BUILD_ROW: u32 = 0x348B_28D6;
const BUILD_TAG: u32 = 0xB332_698E;
const BUILD_TAGS: u32 = 0x11BD_5345;
const DYNAMIC_RESOURCES: u32 = 0xDED3_E212;
/// The bytes of a node's fields before its solver pointer.
const NODE_FIELDS: usize = 41;

/// The solver classes of a path node and the length of their bodies.
const SOLVERS: [(u32, usize); 3] = [(0xBC5B_87AF, 59), (0x1D6F_0AE7, 34), (0x360B_69A4, 6)];
/// The object classes a property value holds and the length of their bodies.
const OBJECTS: [(u32, usize); 5] = [
    (0x731C_E65C, 33),
    (0x2D7B_69F6, 19),
    (0xA9CC_15E8, 28),
    (0xD960_6976, 28),
    (0x1B67_9142, 28),
];

/// Why a BuildTable body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("build table {0}")]
    Body(#[from] BodyError),
    #[error("build table value of kind {kind:#x} at {at} isn't read")]
    Kind { kind: u32, at: usize },
    #[error("build table object of class {class:#010x} at {at} isn't read")]
    Class { class: u32, at: usize },
    #[error("build table: {0} bytes after its last resources")]
    Trailing(usize),
    #[error("a value of kind {kind:#x} isn't a {want}")]
    NotThat { kind: u32, want: &'static str },
    #[error(transparent)]
    Reference(#[from] reference::Error),
}
body_error!(Error, Error::Body);

/// The body's cursor ([`primase_bytes::Cursor`]), with this record's grammar as methods.
struct Cursor<'a>(primase_bytes::Cursor<'a>);

impl<'a> Cursor<'a> {
    fn new(b: &'a [u8]) -> Self {
        Cursor(primase_bytes::Cursor::new(b))
    }
}

impl<'a> std::ops::Deref for Cursor<'a> {
    type Target = primase_bytes::Cursor<'a>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Cursor<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'a> Cursor<'a> {
    fn link(&mut self, what: &'static str) -> Result<BuildLink, Error> {
        Ok(BuildLink {
            tag: self.u8().field(what)?,
            id: self.u64(LE).field(what)?,
        })
    }
    /// The bytes of an object value's body by its class.
    fn body(&mut self, class: u32, what: &'static str) -> Result<&'a [u8], Error> {
        let at = self.position();
        let (_, n) = OBJECTS
            .iter()
            .find(|(c, _)| *c == class)
            .ok_or(Error::Class { class, at })?;
        Ok(self.take(*n).field(what)?)
    }
    /// A typed value: class, u16, kind, u16 and the kind's bytes.
    fn typed(&mut self, what: &'static str) -> Result<BuildValue, Error> {
        let class = self.u32(LE).field(what)?;
        let before = self.u16(LE).field(what)?;
        let kind_at = self.position();
        let kind = self.u32(LE).field(what)?;
        let after = self.u16(LE).field(what)?;
        let start = self.position();
        match kind & 0x3F {
            0x00..=0x03 => {
                self.take(1).field(what)?;
            }
            0x04 | 0x05 => {
                self.take(2).field(what)?;
            }
            0x06 | 0x07 | 0x0A | 0x19 => {
                self.take(4).field(what)?;
            }
            0x08 | 0x09 | 0x0B | 0x11 | 0x1F => {
                self.take(8).field(what)?;
            }
            0x0C => {
                self.take(12).field(what)?;
            }
            0x0D | 0x0E => {
                self.take(16).field(what)?;
            }
            0x10 => {
                self.take(64).field(what)?;
            }
            0x12 => {
                self.take(9).field(what)?;
            }
            0x1C => {
                self.take(reference::LENGTH).field(what)?;
            }
            // an embedded object: id, class, body
            0x13 | 0x16 => {
                self.u64(LE).field(what)?;
                let c = self.u32(LE).field(what)?;
                self.body(c, what)?;
            }
            // a pointer: `03` none, `00` or `04` an object, other tags a reference of 9 bytes
            0x14 | 0x15 => match self.u8().field(what)? {
                3 => {}
                0 | 4 => {
                    self.u64(LE).field(what)?;
                    let c = self.u32(LE).field(what)?;
                    self.body(c, what)?;
                }
                1 | 2 | 5 => {
                    self.take(8).field(what)?;
                }
                other => {
                    return Err(BodyError::Value {
                        what,
                        at: self.position().saturating_sub(1),
                        found: u64::from(other),
                        read: "a pointer tag",
                    }
                    .into());
                }
            },
            _ => return Err(Error::Kind { kind, at: kind_at }),
        }
        let value = self
            .bytes()
            .get(start..self.position())
            .unwrap_or_default()
            .to_vec();
        Ok(BuildValue {
            class,
            before,
            after,
            kind,
            value,
        })
    }
}

/// The condition (a `BuildTag` hash) of the rows of the rider's part tables that hold the male parts: every part
/// table's male row names a mesh whose name has `_M_` in it and the female row one with `_F_` (tests/rider.rs).
/// RRAssetStudio reads the pair as gender conditions; the hashes are the tag descriptors' ids.
pub const MALE: u32 = 0x0B27_268E;
/// The condition of the female rows (see [`MALE`]).
pub const FEMALE: u32 = 0xE2B6_0126;

impl StoredBuildTable {
    /// The rows whose condition is `condition`, in stored order.
    pub fn rows_with(&self, condition: u32) -> impl Iterator<Item = &BuildRow> {
        self.rows.iter().filter(move |r| r.condition == condition)
    }
}

impl BuildRow {
    /// The resources this row's properties name, as (column number, ClassID): the links (kind `0x12`) and the
    /// non-null references (kind `0x1C`), in stored order. Properties of other kinds are not listed.
    pub fn links(&self) -> Result<Vec<(u32, u64)>, Error> {
        let mut out = Vec::new();
        for p in &self.properties {
            match p.value.kind & 0x3F {
                0x12 => out.push((p.column, p.value.link()?.id)),
                0x1C => {
                    if let Some(r) = p.value.reference()? {
                        out.push((p.column, r.id));
                    }
                }
                _ => {}
            }
        }
        Ok(out)
    }
}

/// The solver class `MaterialPropertyPathNodeSolver`: its 59-byte body ends in two links, the generic Material the
/// column's property replaces (at 41, a tag byte and a u64) and a MaterialTemplate (at 50; null on every column read).
const MATERIAL_SOLVER: u32 = 0xBC5B_87AF;

impl BuildColumn {
    /// The generic Material this column's properties replace: the Material link of the one path node's
    /// `MaterialPropertyPathNodeSolver`; `None` for a column of another kind. On the variant tables of Riders Republic
    /// the generic Materials are the ones the part's own mesh names (`MAD_CHR_UBody_MF_ALL` for an upper body's
    /// `_LODM`, and a LOD atlas Material for its lower levels).
    pub fn generic_material(&self) -> Result<Option<u64>, Error> {
        let [node] = self.path.as_slice() else {
            return Ok(None);
        };
        let Some((class, body)) = &node.solver else {
            return Ok(None);
        };
        if *class != MATERIAL_SOLVER {
            return Ok(None);
        }
        // the link at 41: a tag byte, then the id
        let id = View::new(body).u64(42, LE).field("material solver link")?;
        Ok((id != 0).then_some(id))
    }
}

impl BuildValue {
    /// The reference this value holds (kind `0x1C`): `None` for the null form.
    pub fn reference(&self) -> Result<Option<Reference>, Error> {
        if self.kind & 0x3F != 0x1C {
            return Err(Error::NotThat {
                kind: self.kind,
                want: "reference",
            });
        }
        let bytes = View::new(&self.value)
            .array::<{ reference::LENGTH }>(0)
            .field("reference")?;
        Ok(reference::parse(bytes)?)
    }

    /// The link this value holds (kind `0x12`).
    pub fn link(&self) -> Result<BuildLink, Error> {
        if self.kind & 0x3F != 0x12 {
            return Err(Error::NotThat {
                kind: self.kind,
                want: "link",
            });
        }
        let v = View::new(&self.value);
        Ok(BuildLink {
            tag: v.u8(0).field("link")?,
            id: v.u64(1, LE).field("link")?,
        })
    }
}

impl StoredBuildTable {
    /// The body parsed, consumed to its last byte.
    pub fn parse(body: &[u8]) -> Result<StoredBuildTable, Error> {
        let mut c = Cursor::new(body);
        let at = c.position();
        let first = c.u8().field("first byte")?;
        if first != 1 {
            return Err(BodyError::Value {
                what: "first byte",
                at,
                found: u64::from(first),
                read: "1",
            }
            .into());
        }
        let k = count(&mut c, "column count", 12)?;
        let mut columns = Vec::with_capacity(k);
        for _ in 0..k {
            columns.push(column(&mut c)?);
        }
        let k = count(&mut c, "row count", 12)?;
        let mut rows = Vec::with_capacity(k);
        for _ in 0..k {
            rows.push(row(&mut c)?);
        }
        let flags = c.array::<4>().field("flags")?;
        let k = count(&mut c, "resources count", 12)?;
        let mut resources = Vec::with_capacity(k);
        for _ in 0..k {
            let id = c.u64(LE).field("resources id")?;
            expect(&mut c, "resources class", u64::from(DYNAMIC_RESOURCES), 4)?;
            let n = count(&mut c, "link count", 9)?;
            let links = (0..n)
                .map(|_| c.link("resource link"))
                .collect::<Result<Vec<_>, _>>()?;
            resources.push(BuildResources { id, links });
        }
        let left = body.len().saturating_sub(c.position());
        if left != 0 {
            return Err(Error::Trailing(left));
        }
        Ok(StoredBuildTable {
            columns,
            rows,
            flags,
            resources,
        })
    }
}

fn column(c: &mut Cursor<'_>) -> Result<BuildColumn, Error> {
    let id = c.u64(LE).field("column id")?;
    expect(c, "column class", u64::from(BUILD_COLUMN), 4)?;
    let apply_id = c.u64(LE).field("apply id")?;
    expect(c, "apply class", u64::from(APPLY_PASS_BIT_FIELD), 4)?;
    let apply_bits = c.u8().field("apply bits")?;
    // an embedded object of id 0
    expect(c, "path", 0, 8)?;
    expect(c, "path", u64::from(PROPERTY_PATH), 4)?;
    let nodes = count(c, "node count", 12)?;
    let mut path = Vec::with_capacity(nodes);
    for _ in 0..nodes {
        // an embedded object of id 0
        expect(c, "node", 0, 8)?;
        expect(c, "node", u64::from(PROPERTY_PATH_NODE), 4)?;
        let fields = c.take(NODE_FIELDS).field("node fields")?.to_vec();
        let solver = match c.u8().field("solver tag")? {
            3 => None,
            0 => {
                c.u64(LE).field("solver id")?;
                let class = c.u32(LE).field("solver class")?;
                let at = c.position();
                let (_, n) = SOLVERS
                    .iter()
                    .find(|(k, _)| *k == class)
                    .ok_or(Error::Class { class, at })?;
                Some((class, c.take(*n).field("solver body")?.to_vec()))
            }
            other => {
                return Err(BodyError::Value {
                    what: "solver tag",
                    at: c.position().saturating_sub(1),
                    found: u64::from(other),
                    read: "3 or 0",
                }
                .into());
            }
        };
        path.push(PathNode { fields, solver });
    }
    let path_bytes = c.array::<2>().field("path bytes")?;
    let fields = [
        c.u32(LE).field("column field")?,
        c.u32(LE).field("column field")?,
        c.u32(LE).field("column field")?,
    ];
    let table = c.link("column table")?;
    let header = [
        c.u32(LE).field("column header")?,
        c.u32(LE).field("column header")?,
    ];
    let declared = c.typed("declared value")?;
    Ok(BuildColumn {
        id,
        apply_id,
        apply_bits,
        path,
        path_bytes,
        fields,
        table,
        header,
        declared,
    })
}

fn row(c: &mut Cursor<'_>) -> Result<BuildRow, Error> {
    let id = c.u64(LE).field("row id")?;
    expect(c, "row class", u64::from(BUILD_ROW), 4)?;
    let table = c.link("row table")?;
    let weight = c.f32(LE).field("weight")?;
    // an embedded object of id 0
    expect(c, "tag", 0, 8)?;
    expect(c, "tag", u64::from(BUILD_TAG), 4)?;
    let condition = c.u32(LE).field("condition")?;
    let tags_id = c.u64(LE).field("tags id")?;
    expect(c, "tags class", u64::from(BUILD_TAGS), 4)?;
    let at = c.position();
    let tags = c.u32(LE).field("tag count")?;
    if tags != 0 {
        return Err(BodyError::Value {
            what: "tag count",
            at,
            found: u64::from(tags),
            read: "0",
        }
        .into());
    }
    let n = count(c, "property count", 16)?;
    let mut properties = Vec::with_capacity(n);
    for _ in 0..n {
        let column = c.u32(LE).field("property column")?;
        let value = c.typed("property value")?;
        properties.push(BuildProperty { column, value });
    }
    Ok(BuildRow {
        id,
        table,
        weight,
        condition,
        tags_id,
        properties,
    })
}
