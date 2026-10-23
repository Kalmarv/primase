//! The runtime data (`RDAT`) part of a library container: its subobjects (state object and pipeline configurations,
//! root signatures and their associations, hit groups), which a ray tracing pipeline is built from.
//!
//! Layout (little-endian): a version (0x10) and a part count, then the offset of each part; a part is its type and byte
//! size, then its bytes. The parts read here are the string buffer (1; strings null-terminated, referenced by offset),
//! the index arrays (2; `u32` words, an array being its count then its items, referenced by word index), the raw bytes
//! (5) and the subobject table (6: a record count, a record stride, then the records). A subobject record is its kind
//! and name (a string reference) then, by kind, its data:
//!
//! | kind | data |
//! |------|------|
//! | 0 state object config | flags |
//! | 1, 2 global and local root signature | offset and size in the raw bytes |
//! | 8 association | subobject name (string), exports (index array of strings) |
//! | 9 shader config | max payload bytes, max attribute bytes |
//! | 10 pipeline config | max recursion depth |
//! | 11 hit group | type (0 triangles, 1 procedural), any-hit, closest-hit, intersection (strings) |
//! | 12 pipeline config 1 | max recursion depth, flags |
//!
//! These are DXC's definitions (`DxilRuntimeReflection.h`, `RDAT_SubobjectTypes.inl`, `DXIL::SubobjectKind` in
//! `DxilConstants.h`, DirectXShaderCompiler main as of 2026-10): kinds 3 to 7 are reserved there and refused here. A
//! record is the kind, the name and the data (a union: its fields start at the third word); the table's stride is the
//! largest record's.
//!
//! A string reference of all ones, or to an empty string, is none. A library without a subobject table has no
//! subobjects. Anything outside this (another version, a kind not listed, a reference outside its part) is refused.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use primase_shader::{HitGroup, HitGroupKind, Subobject};

use crate::{Result, error};

const VERSION: u32 = 0x10;
const STRINGS: u32 = 1;
const INDEX_ARRAYS: u32 = 2;
const RAW_BYTES: u32 = 5;
const SUBOBJECTS: u32 = 6;
const NULL: u32 = u32::MAX;

fn word(data: &[u8], at: usize) -> Result<u32> {
    at.checked_add(4)
        .and_then(|end| data.get(at..end))
        .and_then(|b| b.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| error(at.saturating_mul(8), "RDAT: a word past the end of its part"))
}

/// The part of type `wanted`, if the data has one.
fn part(rdat: &[u8], wanted: u32) -> Result<Option<&[u8]>> {
    let version = word(rdat, 0)?;
    if version != VERSION {
        return Err(error(
            0,
            format!("RDAT version {version:#x}: only {VERSION:#x} is read"),
        ));
    }
    let count = word(rdat, 4)?;
    for k in 0..count {
        let at = usize::try_from(k)
            .ok()
            .and_then(|k| k.checked_mul(4))
            .and_then(|k| k.checked_add(8))
            .ok_or_else(|| error(32, "RDAT: part count"))?;
        let offset = usize::try_from(word(rdat, at)?).map_err(|_| error(0, "RDAT: part offset"))?;
        if word(rdat, offset)? != wanted {
            continue;
        }
        let size = usize::try_from(word(rdat, offset.saturating_add(4))?)
            .map_err(|_| error(0, "RDAT: part size"))?;
        let start = offset.saturating_add(8);
        return rdat
            .get(start..start.saturating_add(size))
            .map(Some)
            .ok_or_else(|| error(offset.saturating_mul(8), "RDAT: a part past the end of the data"));
    }
    Ok(None)
}

struct Strings<'a>(&'a [u8]);
impl Strings<'_> {
    /// The string at `offset`; none for a null reference or an empty string.
    fn get(&self, offset: u32) -> Result<Option<String>> {
        if offset == NULL {
            return Ok(None);
        }
        let start = usize::try_from(offset).map_err(|_| error(0, "RDAT: string offset"))?;
        let rest = self
            .0
            .get(start..)
            .ok_or_else(|| error(0, format!("RDAT: string {offset} past the string buffer")))?;
        let end = rest
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| error(0, "RDAT: a string without its end"))?;
        let text = std::str::from_utf8(rest.get(..end).unwrap_or(&[]))
            .map_err(|_| error(0, "RDAT: a string that is not UTF-8"))?;
        Ok((!text.is_empty()).then(|| text.to_owned()))
    }

    fn required(&self, offset: u32, what: &str) -> Result<String> {
        self.get(offset)?
            .ok_or_else(|| error(0, format!("RDAT: {what} has no name")))
    }
}

/// The subobjects of the library `rdat` (an `RDAT` part's bytes).
pub fn subobjects(rdat: &[u8]) -> Result<Vec<Subobject>> {
    let Some(table) = part(rdat, SUBOBJECTS)? else {
        return Ok(Vec::new());
    };
    let strings = Strings(part(rdat, STRINGS)?.unwrap_or(&[]));
    let arrays = part(rdat, INDEX_ARRAYS)?.unwrap_or(&[]);
    let raw = part(rdat, RAW_BYTES)?.unwrap_or(&[]);
    let count = usize::try_from(word(table, 0)?).map_err(|_| error(0, "RDAT: subobject count"))?;
    let stride = usize::try_from(word(table, 4)?).map_err(|_| error(0, "RDAT: subobject stride"))?;
    if stride < 8 {
        return Err(error(0, format!("RDAT: subobject records of {stride} bytes")));
    }
    let mut out = Vec::new();
    for k in 0..count {
        let at = k
            .checked_mul(stride)
            .and_then(|o| o.checked_add(8))
            .ok_or_else(|| error(0, "RDAT: subobject table size"))?;
        let record = table
            .get(at..at.saturating_add(stride))
            .ok_or_else(|| error(0, "RDAT: the subobject table is shorter than its count says"))?;
        let field = |n: usize| word(record, n.saturating_mul(4));
        let kind = field(0)?;
        let name = strings.required(field(1)?, "a subobject")?;
        out.push(match kind {
            0 => Subobject::StateObjectConfig {
                name,
                flags: field(2)?,
            },
            1 | 2 => {
                let offset =
                    usize::try_from(field(2)?).map_err(|_| error(0, "RDAT: root signature offset"))?;
                let size = usize::try_from(field(3)?).map_err(|_| error(0, "RDAT: root signature size"))?;
                let signature = raw
                    .get(offset..offset.saturating_add(size))
                    .ok_or_else(|| error(0, format!("RDAT: root signature {name} past the raw bytes")))?
                    .to_vec();
                if kind == 1 {
                    Subobject::GlobalRootSignature { name, signature }
                } else {
                    Subobject::LocalRootSignature { name, signature }
                }
            }
            8 => {
                let subobject = strings.required(field(2)?, "an association's subobject")?;
                // an index array: its count, then its items (string references)
                let first = usize::try_from(field(3)?).map_err(|_| error(0, "RDAT: exports index"))?;
                let n = usize::try_from(word(arrays, first.saturating_mul(4))?)
                    .map_err(|_| error(0, "RDAT: exports count"))?;
                let mut exports = Vec::new();
                for i in 0..n {
                    let at = first.saturating_add(1).saturating_add(i).saturating_mul(4);
                    exports.push(strings.required(word(arrays, at)?, "an export")?);
                }
                Subobject::ExportsAssociation {
                    name,
                    subobject,
                    exports,
                }
            }
            9 => Subobject::ShaderConfig {
                name,
                max_payload_bytes: field(2)?,
                max_attribute_bytes: field(3)?,
            },
            10 => Subobject::PipelineConfig {
                name,
                max_recursion_depth: field(2)?,
                flags: None,
            },
            11 => Subobject::HitGroup(HitGroup {
                name,
                kind: match field(2)? {
                    0 => HitGroupKind::Triangles,
                    1 => HitGroupKind::ProceduralPrimitives,
                    other => return Err(error(0, format!("RDAT: hit group type {other}"))),
                },
                any_hit: strings.get(field(3)?)?,
                closest_hit: strings.get(field(4)?)?,
                intersection: strings.get(field(5)?)?,
            }),
            12 => Subobject::PipelineConfig {
                name,
                max_recursion_depth: field(2)?,
                flags: Some(field(3)?),
            },
            3..=7 => return Err(error(0, format!("RDAT: subobject kind {kind} is reserved"))),
            other => return Err(error(0, format!("RDAT: subobject kind {other}"))),
        });
    }
    Ok(out)
}

/// A function's export name from its name in the module: `\u{1}?closest_hit@@YAXXZ` is `closest_hit`.
pub fn export_name(function: &str) -> String {
    let undecorated = function.strip_prefix('\u{1}').unwrap_or(function);
    match undecorated.strip_prefix('?') {
        Some(rest) => rest.split("@@").next().unwrap_or(rest).to_owned(),
        None => undecorated.to_owned(),
    }
}
