//! Studio's level root: boxed strings that point out of their document, repaired from the game's own level root.
//!
//! Studio builds every level root (`DetachedSubWorldData`) from one template, and the template's
//! `InterfaceDescriptorData` carries ten `DataField`s whose default values are boxed values: floats, flags, integers, a
//! colour and a `CString`. A boxed `CString` is a pointer slot holding the distance from the slot to the string; Studio
//! copies the template's bytes unchanged (`a2 05 00 00 00 00 00 00`: 0x5a2) while its own string table is much shorter,
//! so the pointer lands 1074 to 1184 bytes past the end of the document (measured on 7 Studio level roots: Shaun White
//! level 4 and 1, and five community maps). The game loads the document's data into one block and relocates the slot;
//! the object it builds holds a `char*` that points past its block, and reading it is a copy of the field's value
//! (`strlen` of that pointer), which faults when the page after the block is not mapped (Shaun White level 4, two
//! launches of three, `Skate+0x3daa8a0`). The template is the game's own level root, where the same slot points at the
//! string `Gameflow test running` inside the document. Every other default value in Studio's copy equals the donor's.
//!
//! The repair gives each such slot a real string: the donor's value of the same `DataField` (matched by its `Id`),
//! appended to the document's string table, and the document is laid out again by the EBX writer, which carries the
//! other boxed values and every other string as they are (a Studio level root read and written again is byte for byte
//! the same: the writer's layout is Studio's). The donor is read from the installed game at run time ([`Donor::find`]);
//! no game data is part of this crate.
//!
//! [`check`] is the build gate: an EBX document of a Patch with a pointer slot (string, array, boxed value) outside its
//! payload is an error, and so is a document that cannot be read.

use std::path::Path;

use primase_foundations::compress::oodle::Oodle;

use crate::postcompile::fb::bundle;
use crate::postcompile::fb::ebx::{self, BoxedValueRecord, Document, FieldType, Value};
use crate::postcompile::fb::folder::{Folder, Source};
use crate::postcompile::vblend::{Archives, copy_dir, encode, patch_bundles, sha1, write_back};
use crate::postcompile::{Error, Result, bad};

const W: &str = "level root";

/// The game's level root, where Studio's template comes from: the EBX `levels/game/bam_levelroot/bam_levelroot` in
/// the bundle of that name of the TOC `Win32/levels/game/bam_levelroot/bam_levelroot.toc` (relative to the data
/// folder). It is the only document of the game's level TOCs that holds the template's string.
pub const DONOR_TOC: &str = "Win32/levels/game/bam_levelroot/bam_levelroot.toc";
pub const DONOR_BUNDLE: &str = "win32/levels/game/bam_levelroot/bam_levelroot";
pub const DONOR_EBX: &str = "levels/game/bam_levelroot/bam_levelroot";

/// The type whose `Fields` hold the boxed default values.
const INTERFACE: &str = "InterfaceDescriptorData";

/// The type of a boxed value or record from its encoded type (the low 16 bits, as stored, shifted back).
fn boxed_type(encoded: u32) -> FieldType {
    FieldType::from_bits((((encoded & 0xFFFF) >> 1) >> 4) as u16 & 0x1F)
}

/// One `DataField` of an `InterfaceDescriptorData`: its `Id`, its boxed value's encoded type, and the offset of the
/// boxed-value record that holds the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataField {
    pub id: u32,
    pub encoded_type: u32,
    pub record: u32,
}

impl DataField {
    pub fn ty(&self) -> FieldType {
        boxed_type(self.encoded_type)
    }
}

/// Every `DataField` of every `InterfaceDescriptorData` instance of `doc` that has a boxed value, in order.
pub fn data_fields(doc: &Document) -> Vec<DataField> {
    let mut out = Vec::new();
    for (i, inst) in doc.instances.iter().enumerate() {
        if doc.type_name(i) != Some(INTERFACE) {
            continue;
        }
        let Some(Value::Array(fields, _)) = inst.object.as_ref().and_then(|o| o.field("Fields")) else {
            continue;
        };
        for f in fields {
            let Value::Object(f) = f else { continue };
            let id = match f.field("Id") {
                Some(Value::Int(v)) => *v as u32,
                Some(Value::UInt(v)) => *v as u32,
                _ => continue,
            };
            let Some(Value::Boxed(b)) = f.field("BoxedValue") else {
                continue;
            };
            let Ok(record) = u32::try_from(b.data_offset) else {
                continue;
            };
            out.push(DataField {
                id,
                encoded_type: b.encoded_type,
                record,
            });
        }
    }
    out
}

/// A pointer slot whose target is not inside the document's payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outside {
    /// The slot's payload offset.
    pub slot: u32,
    /// Where it points, as a payload offset.
    pub target: i64,
    /// The boxed-value record the slot lies in (its offset), when it lies in one.
    pub record: Option<u32>,
}

/// Every pointer slot of `doc` (its EFIX pointer fixups) whose target lies outside the payload: before its start or at
/// or after its end.
pub fn outside_pointers(doc: &Document) -> Vec<Outside> {
    let mut out = Vec::new();
    for &slot in &doc.pointer_offsets {
        let s = slot as usize;
        let Some(disp) = doc
            .payload
            .get(s..s + 4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        else {
            out.push(Outside {
                slot,
                target: -1,
                record: None,
            });
            continue;
        };
        let target = i64::from(slot) + i64::from(disp);
        if (0..doc.payload.len() as i64).contains(&target) {
            continue;
        }
        let record = doc
            .boxed_values
            .iter()
            .find(|r| r.offset <= slot && (slot - r.offset) < r.raw.len() as u32)
            .map(|r| r.offset);
        out.push(Outside { slot, target, record });
    }
    out
}

/// The NUL-terminated string at payload offset `at`, when it lies in the document's string table.
fn string_in_table(doc: &Document, at: i64) -> Option<Vec<u8>> {
    let start = usize::try_from(at).ok()?;
    if start < doc.strings_offset as usize || start >= doc.payload.len() {
        return None;
    }
    let tail = &doc.payload[start..];
    let end = tail.iter().position(|&b| b == 0)?;
    Some(tail[..end].to_vec())
}

/// One `DataField` of the donor with its value as the document holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DonorField {
    pub id: u32,
    pub encoded_type: u32,
    /// The boxed-value record's bytes.
    pub raw: Vec<u8>,
    /// The string a `CString` value points at (its bytes, without the terminator).
    pub text: Option<Vec<u8>>,
}

/// The game's level root: the `DataField` values Studio's template copies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Donor {
    /// The EBX document's name.
    pub name: String,
    pub fields: Vec<DonorField>,
}

impl Donor {
    /// Reads the donor from EBX `name` of the bundle `bundle_name` of the TOC `toc` (all relative to the data folder
    /// of `game`; the constants [`DONOR_TOC`], [`DONOR_BUNDLE`] and [`DONOR_EBX`] name the game's level root). An
    /// error when the TOC, the bundle or the document is not there, or the document has no `DataField`, or one of
    /// its `CString` values does not point into its string table.
    pub fn find(game: &Source, toc: &str, bundle_name: &str, name: &str) -> Result<Donor> {
        let t = game.folder.toc(toc)?;
        let (_, region) = t
            .bundles
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(bundle_name))
            .ok_or_else(|| Error::Missing(format!("{toc} has no bundle {bundle_name}")))?;
        let (files, inline) = bundle::read_region(region)?;
        let (m, first, _) = game
            .manifest(&files, &inline)?
            .ok_or_else(|| Error::Missing(format!("{bundle_name} has no manifest")))?;
        let (i, e) = m
            .ebx
            .iter()
            .enumerate()
            .find(|(_, e)| e.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| Error::Missing(format!("{bundle_name} holds no EBX {name}")))?;
        let doc = ebx::read(&game.payload(files[first + i], e.size as usize)?)?;
        Donor::from_document(name, &doc)
    }

    /// The donor from an EBX document read.
    pub fn from_document(name: &str, doc: &Document) -> Result<Donor> {
        let fields = data_fields(doc);
        if fields.is_empty() {
            return Err(Error::Missing(format!("{name} has no {INTERFACE} DataField")));
        }
        let mut out = Vec::with_capacity(fields.len());
        for f in fields {
            let rec = record_at(doc, f.record).ok_or_else(|| {
                bad(
                    W,
                    f.record as usize,
                    format!("{name}: field {:#010x} names no boxed-value record", f.id),
                )
            })?;
            let text = if f.ty() == FieldType::CString {
                let t = doc
                    .pointer_offsets
                    .contains(&rec.offset)
                    .then(|| pointer_target(doc, rec.offset))
                    .and_then(|t| string_in_table(doc, t));
                Some(t.ok_or_else(|| {
                    bad(
                        W,
                        rec.offset as usize,
                        format!(
                            "{name}: CString field {:#010x} does not point into the document's string table",
                            f.id
                        ),
                    )
                })?)
            } else {
                None
            };
            out.push(DonorField {
                id: f.id,
                encoded_type: f.encoded_type,
                raw: rec.raw.clone(),
                text,
            });
        }
        Ok(Donor {
            name: name.to_owned(),
            fields: out,
        })
    }

    pub fn field(&self, id: u32) -> Option<&DonorField> {
        self.fields.iter().find(|f| f.id == id)
    }
}

fn record_at(doc: &Document, offset: u32) -> Option<&BoxedValueRecord> {
    doc.boxed_values.iter().find(|r| r.offset == offset)
}

/// Where the pointer slot at payload offset `slot` points (a payload offset; may lie outside the payload).
fn pointer_target(doc: &Document, slot: u32) -> i64 {
    let s = slot as usize;
    doc.payload.get(s..s + 4).map_or(-1, |b| {
        i64::from(slot) + i64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    })
}

/// A repaired slot: the `DataField` it belongs to, where it pointed and the string it points at now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repair {
    pub id: u32,
    pub was: i64,
    pub value: Vec<u8>,
}

/// Gives every boxed `CString` of `doc` whose pointer lies outside the document the donor's value of the same
/// `DataField`, appended to the string table (the writer lays it out). Every other pointer outside the document, a
/// slot that is not the `CString` value of an `InterfaceDescriptorData` field, a field the donor lacks or holds as
/// another type, is an error and `doc` is left as it was.
pub fn repair_document(doc: &mut Document, donor: &Donor) -> Result<Vec<Repair>> {
    let outside = outside_pointers(doc);
    if outside.is_empty() {
        return Ok(Vec::new());
    }
    let fields = data_fields(doc);
    let mut plan = Vec::with_capacity(outside.len());
    for o in &outside {
        let unsupported = |why: &str| {
            bad(
                W,
                o.slot as usize,
                format!(
                    "pointer slot {} points to {} of a payload of {}: {why}",
                    o.slot,
                    o.target,
                    doc.payload.len()
                ),
            )
        };
        let rec = o
            .record
            .filter(|&r| r == o.slot)
            .ok_or_else(|| unsupported("it is not the pointer of a boxed value"))?;
        let field = fields
            .iter()
            .find(|f| f.record == rec)
            .ok_or_else(|| unsupported("its boxed value belongs to no InterfaceDescriptorData DataField"))?;
        if field.ty() != FieldType::CString {
            return Err(unsupported("its DataField is not a CString"));
        }
        let want = donor
            .field(field.id)
            .ok_or_else(|| Error::Missing(format!("{}: no DataField {:#010x}", donor.name, field.id)))?;
        if want.encoded_type != field.encoded_type {
            return Err(bad(
                W,
                o.slot as usize,
                format!(
                    "DataField {:#010x} is {:#x} here and {:#x} in {}",
                    field.id, field.encoded_type, want.encoded_type, donor.name
                ),
            ));
        }
        let text = want.text.clone().ok_or_else(|| {
            Error::Missing(format!(
                "{}: DataField {:#010x} holds no string",
                donor.name, field.id
            ))
        })?;
        plan.push((field.id, *o, text));
    }
    let assign: Vec<(u32, Vec<u8>)> = plan.iter().map(|(_, o, text)| (o.slot, text.clone())).collect();
    point_at_strings(doc, &assign)?;
    Ok(plan
        .into_iter()
        .map(|(id, o, value)| Repair {
            id,
            was: o.target,
            value,
        })
        .collect())
}

/// Points each pointer slot of a boxed value (`(slot, string)`, the slot's payload offset) at a copy of its string
/// appended to the string table: the slot's distance, the record's bytes and the payload are updated, so the writer
/// ([`ebx::write`]) carries the string with the slot. An error when a slot is not a pointer slot inside a boxed-value
/// record; `doc` is then as it was.
pub fn point_at_strings(doc: &mut Document, slots: &[(u32, Vec<u8>)]) -> Result<()> {
    for (slot, _) in slots {
        let in_record = doc.pointer_offsets.contains(slot)
            && doc
                .boxed_values
                .iter()
                .any(|r| r.offset <= *slot && (*slot - r.offset) as usize + 4 <= r.raw.len());
        if !in_record {
            return Err(bad(
                W,
                *slot as usize,
                "not a pointer slot inside a boxed-value record",
            ));
        }
    }
    for (slot, text) in slots {
        let at = doc.payload.len();
        doc.payload.extend_from_slice(text);
        doc.payload.push(0);
        let bytes = ((at as i64 - i64::from(*slot)) as i32).to_le_bytes();
        let s = *slot as usize;
        doc.payload[s..s + 4].copy_from_slice(&bytes);
        if let Some(r) = doc
            .boxed_values
            .iter_mut()
            .find(|r| r.offset <= *slot && (*slot - r.offset) as usize + 4 <= r.raw.len())
        {
            let k = (*slot - r.offset) as usize;
            r.raw[k..k + 4].copy_from_slice(&bytes);
        }
    }
    Ok(())
}

/// A document of a Patch repaired: where it is and what was done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentRepair {
    pub toc: String,
    pub bundle: String,
    pub document: String,
    pub repairs: Vec<Repair>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The patch-placed EBX documents read.
    pub documents: usize,
    pub repaired: Vec<DocumentRepair>,
}

/// Rewrites Patch `patch` into `out` (which must not exist) with every document repaired by [`repair_document`], their
/// payloads, manifests and TOCs rewritten the way [`crate::postcompile::vblend::apply`] does, and checks the result
/// with [`check`].
#[allow(
    clippy::disallowed_methods,
    reason = "writer I/O: refuses an existing output folder"
)]
pub(crate) fn apply(
    patch: &Path,
    out: &Path,
    base: Option<&Folder>,
    oodle: Option<&Oodle>,
    donor: &Donor,
) -> Result<Report> {
    if out.exists() {
        return Err(Error::Missing(format!(
            "{} exists; give a new folder",
            out.display()
        )));
    }
    copy_dir(patch, out)?;
    let folder = Folder::open(out)?;
    let src = Source {
        folder: &folder,
        base,
        oodle,
    };
    let mut report = Report::default();
    let mut tocs = patch_bundles(&src)?;
    let mut archives = Archives::new(&folder);
    for (rel, t, bundles) in &mut tocs {
        for (j, b) in bundles.iter_mut().enumerate() {
            let Some(b) = b.as_mut() else { continue };
            for i in 0..b.manifest.ebx.len() {
                let f = b.files[1 + i];
                if !f.0.patch {
                    continue;
                }
                let name = b.manifest.ebx[i].name.clone();
                let bytes = src.payload(f, b.manifest.ebx[i].size as usize)?;
                let mut doc = ebx::read(&bytes).map_err(|e| bad(W, 0, format!("{rel} {name}: {e}")))?;
                report.documents += 1;
                if outside_pointers(&doc).is_empty() {
                    continue;
                }
                let repairs =
                    repair_document(&mut doc, donor).map_err(|e| bad(W, 0, format!("{rel} {name}: {e}")))?;
                let d = ebx::write(&doc)?;
                let again = ebx::read(&d)?;
                if !outside_pointers(&again).is_empty() {
                    return Err(bad(
                        W,
                        0,
                        format!("{rel} {name}: a pointer is still outside after the repair"),
                    ));
                }
                let enc = encode(&d)?;
                b.manifest.ebx[i].size = d.len() as u32;
                b.manifest.ebx[i].sha1 = sha1(&enc);
                b.files[1 + i] = archives.put(f.0, &enc)?;
                b.changed = true;
                report.repaired.push(DocumentRepair {
                    toc: rel.clone(),
                    bundle: t.bundles[j].0.clone(),
                    document: name,
                    repairs,
                });
            }
        }
    }
    write_back(&folder, &mut tocs, archives)?;
    let written = Folder::open(out)?;
    let checked = check(&Source {
        folder: &written,
        base,
        oodle,
    })?;
    if checked != report.documents {
        return Err(bad(
            W,
            0,
            format!(
                "{} documents read before the repair, {checked} after",
                report.documents
            ),
        ));
    }
    Ok(report)
}

/// The build gate: reads every EBX document the Patch places and refuses one that cannot be read or holds a pointer
/// slot outside its payload; returns the number of documents read.
pub fn check(src: &Source) -> Result<usize> {
    let mut documents = 0;
    let mut found = Vec::new();
    for (rel, _, bundles) in patch_bundles(src)? {
        for b in bundles.iter().flatten() {
            for (i, a) in b.manifest.ebx.iter().enumerate() {
                let f = b.files[1 + i];
                if !f.0.patch {
                    continue;
                }
                let bytes = src.payload(f, a.size as usize)?;
                documents += 1;
                let doc = match ebx::read(&bytes) {
                    Ok(d) => d,
                    Err(e) => {
                        found.push(format!("{rel} {}: cannot be read: {e}", a.name));
                        continue;
                    }
                };
                for o in outside_pointers(&doc) {
                    found.push(format!(
                        "{rel} {}: the pointer slot at {} points to {} of a payload of {}{}",
                        a.name,
                        o.slot,
                        o.target,
                        doc.payload.len(),
                        if o.record.is_some() {
                            " (a boxed value)"
                        } else {
                            ""
                        }
                    ));
                }
            }
        }
    }
    if found.is_empty() {
        return Ok(documents);
    }
    let shown = found.len().min(8);
    Err(bad(
        W,
        0,
        format!(
            "{} of {documents} EBX documents: {}{}",
            found.len(),
            found[..shown].join("; "),
            if found.len() > shown { "; ..." } else { "" }
        ),
    ))
}
