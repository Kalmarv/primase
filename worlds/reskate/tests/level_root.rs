//! Studio's level root: boxed strings that point out of their document (`postcompile::levelroot`).
//! - the synthetic gates build a document in the shape of Studio's level root from the committed `rfl2.ebx` fixture of
//!   `primase-frostbite` (a boxed `CString` record `a2 05 00 00 00 00 00 00` after the fixture's own boxed value, its
//!   target past the end of the payload) and need no data: the gate sees the slot, the repair gives it a string
//!   in the string table, the writer lays it out, every other byte of the boxed section stays;
//! - `the_games_level_root_is_the_donor` (`installs.skate/Data`, `tools.oodle9`): the donor is read from the game's
//!   level root, a missing bundle or document is refused;
//! - `studio_level_root_is_repaired` (`oracles.studio.gta/legion/patch`, a Patch folder of ReSkate Studio 20261012's
//!   compile, its map `oracles.baseline.gta/legion/map`, plus the game): the gate refuses the Patch for the level
//!   root's boxed string; the copy `postcompile::finish` writes (blend stage, repair, layering gate) passes it,
//!   differs from the Patch's documents in that one only, is the same bytes when repeated, and finishing it again
//!   repairs nothing.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::Path;

use primase_compress::oodle::{Oodle, Version};
use primase_reskate::postcompile::fb::bundle;
use primase_reskate::postcompile::fb::ebx::{
    self, BoxedRef, BoxedValueRecord, Document, FieldValue, Instance, Object, TypeDescriptor, Value,
};
use primase_reskate::postcompile::fb::folder::{Folder, Source};
use primase_reskate::postcompile::finish::finish;
use primase_reskate::postcompile::levelroot::{
    self, DONOR_BUNDLE, DONOR_EBX, DONOR_TOC, Donor, DonorField, outside_pointers, point_at_strings,
    repair_document,
};
use primase_testdata::{Game, OracleSet, Tool, install_file, oracle, tool};

type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A small hashed-reflection document with one boxed value (the fixture's own).
const BASE: &[u8] = include_bytes!("../../frostbite/tests/fixtures/ebx/rfl2.ebx");

/// The `DataField` of the default description: its `Id`, and the boxed `CString`'s encoded type.
const ID: u32 = 0x7a5f_4ec0;
const CSTRING: u32 = 0x8000_00e0;

/// Studio's distance: 0x5a2, which from the record at the end of this small payload lands far past its end.
const STUDIO_DISTANCE: i32 = 0x5a2;

/// The fixture with Studio's boxed `CString` record added after its own boxed value, as bytes of the writer's layout.
fn studio_shaped() -> Document {
    let mut doc = ebx::read(BASE).unwrap();
    let slot = doc.strings_offset;
    let mut raw = vec![0u8; 8];
    raw[..4].copy_from_slice(&STUDIO_DISTANCE.to_le_bytes());
    doc.payload.extend_from_slice(&raw);
    doc.boxed_values.push(BoxedValueRecord {
        offset: slot,
        count: 1,
        hash: 0x16ac_6a78,
        encoded_type: 0x00e0,
        class_ref: 0xFFFF,
        raw,
    });
    doc.pointer_offsets.push(slot);
    doc.strings_offset += 8;
    doc.data_end += 8;
    let bytes = ebx::write(&doc).unwrap();
    ebx::read(&bytes).unwrap()
}

/// The payload offset of the added record in `doc` (the one that is a boxed `CString`).
fn cstring_slot(doc: &Document) -> u32 {
    doc.boxed_values
        .iter()
        .find(|r| r.encoded_type == 0x00e0)
        .unwrap()
        .offset
}

/// The string `doc`'s pointer slot `slot` points at.
fn string_at_slot(doc: &Document, slot: u32) -> Vec<u8> {
    let s = slot as usize;
    let disp = i32::from_le_bytes(doc.payload[s..s + 4].try_into().unwrap());
    let at = (i64::from(slot) + i64::from(disp)) as usize;
    let end = doc.payload[at..].iter().position(|&b| b == 0).unwrap();
    doc.payload[at..at + end].to_vec()
}

#[test]
fn studios_boxed_string_is_outside_its_document() {
    let doc = studio_shaped();
    let slot = cstring_slot(&doc);
    let found = outside_pointers(&doc);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].slot, slot);
    assert_eq!(found[0].target, i64::from(slot) + i64::from(STUDIO_DISTANCE));
    assert_eq!(found[0].record, Some(slot));
    assert!(found[0].target >= doc.payload.len() as i64);
    // the fixture itself has none
    assert!(outside_pointers(&ebx::read(BASE).unwrap()).is_empty());
}

#[test]
fn the_writer_lays_out_a_repaired_boxed_string() -> R {
    let mut doc = studio_shaped();
    let slot = cstring_slot(&doc);
    let others: Vec<_> = doc
        .boxed_values
        .iter()
        .filter(|r| r.offset != slot)
        .cloned()
        .collect();
    let before = ebx::write(&doc)?;
    point_at_strings(&mut doc, &[(slot, b"a default description".to_vec())])?;
    assert!(outside_pointers(&doc).is_empty());
    let bytes = ebx::write(&doc)?;
    let again = ebx::read(&bytes)?;
    assert!(
        outside_pointers(&again).is_empty(),
        "{:?}",
        outside_pointers(&again)
    );
    let slot = cstring_slot(&again);
    assert_eq!(string_at_slot(&again, slot), b"a default description");
    // every other boxed record is as it was
    let kept: Vec<_> = again
        .boxed_values
        .iter()
        .filter(|r| r.offset != slot)
        .map(|r| (r.count, r.hash, r.encoded_type, r.raw.clone()))
        .collect();
    let was: Vec<_> = others
        .iter()
        .map(|r| (r.count, r.hash, r.encoded_type, r.raw.clone()))
        .collect();
    assert_eq!(kept, was);
    // the instances and their values are unchanged
    let old = ebx::read(&before)?;
    assert_eq!(
        again.instances.iter().map(|i| &i.object).collect::<Vec<_>>(),
        old.instances.iter().map(|i| &i.object).collect::<Vec<_>>()
    );
    // the layout is a fixed point
    assert_eq!(ebx::write(&again)?, bytes);
    Ok(())
}

#[test]
fn only_pointer_slots_of_boxed_values_take_a_string() {
    let mut doc = studio_shaped();
    let slot = cstring_slot(&doc);
    let before = doc.clone();
    for bad in [slot + 1, 0, 9999] {
        assert!(
            point_at_strings(&mut doc, &[(slot, b"ok".to_vec()), (bad, b"no".to_vec())]).is_err(),
            "slot {bad}"
        );
        assert_eq!(doc, before, "a refused repair left the document changed");
    }
}

/// `doc` with an `InterfaceDescriptorData` instance (in the model only) whose `Fields` hold one `DataField` of `id`
/// with the boxed value at `record`.
fn with_data_field(mut doc: Document, id: i64, encoded_type: u32, record: u32) -> Document {
    let ty = doc.types.len();
    doc.types.push(TypeDescriptor {
        name: "InterfaceDescriptorData".into(),
        name_hash: 0,
        field_index: 0,
        field_count: 0,
        encoded_type: 0,
        size: 0,
        alignment: 1,
        guid: [0; 16],
        signature: 0,
    });
    let field = Object {
        descriptor: 0,
        fields: vec![
            FieldValue {
                descriptor: 0,
                name: "BoxedValue".into(),
                value: Value::Boxed(BoxedRef {
                    encoded_type,
                    data_offset: i64::from(record),
                    relative_offset: 0,
                }),
            },
            FieldValue {
                descriptor: 0,
                name: "Id".into(),
                value: Value::Int(id),
            },
        ],
    };
    doc.instances.push(Instance {
        fixup_type: 0,
        descriptor: Some(ty),
        data_offset: 0,
        exported: false,
        guid: [0; 16],
        object: Some(Object {
            descriptor: ty,
            fields: vec![FieldValue {
                descriptor: 0,
                name: "Fields".into(),
                value: Value::Array(vec![Value::Object(Box::new(field))], None),
            }],
        }),
        raw_image: Vec::new(),
    });
    doc
}

fn donor(fields: Vec<DonorField>) -> Donor {
    Donor {
        name: "donor".into(),
        fields,
    }
}

fn description(text: Option<&[u8]>) -> DonorField {
    DonorField {
        id: ID,
        encoded_type: CSTRING,
        raw: vec![0; 8],
        text: text.map(<[u8]>::to_vec),
    }
}

#[test]
fn a_data_field_takes_the_donors_value() {
    let base = studio_shaped();
    let slot = cstring_slot(&base);
    let mut doc = with_data_field(base, i64::from(ID), CSTRING, slot);
    let repairs = repair_document(&mut doc, &donor(vec![description(Some(b"from the donor"))])).unwrap();
    assert_eq!(repairs.len(), 1);
    assert_eq!(repairs[0].id, ID);
    assert_eq!(repairs[0].value, b"from the donor");
    assert_eq!(repairs[0].was, i64::from(slot) + i64::from(STUDIO_DISTANCE));
    assert!(outside_pointers(&doc).is_empty());
    assert_eq!(string_at_slot(&doc, slot), b"from the donor");
    // a document with nothing outside is left alone
    let after = doc.clone();
    assert!(repair_document(&mut doc, &donor(vec![])).unwrap().is_empty());
    assert_eq!(doc, after);
}

#[test]
fn a_repair_without_a_donor_value_is_refused_and_changes_nothing() {
    let base = studio_shaped();
    let slot = cstring_slot(&base);
    let doc = with_data_field(base.clone(), i64::from(ID), CSTRING, slot);
    let refuse = |doc: &Document, d: Donor, what: &str| {
        let mut work = doc.clone();
        let e = repair_document(&mut work, &d).expect_err(what).to_string();
        assert_eq!(&work, doc, "{what}: the document changed");
        e
    };
    // the donor has no such field
    let other = DonorField {
        id: ID ^ 1,
        ..description(Some(b"x"))
    };
    let e = refuse(&doc, donor(vec![other]), "no field");
    assert!(
        e.contains("no DataField 0x7a5f4ec1") || e.contains("0x7a5f4ec0"),
        "{e}"
    );
    // the donor's field is another type
    let float = DonorField {
        encoded_type: 0x8000_0260,
        ..description(Some(b"x"))
    };
    let e = refuse(&doc, donor(vec![float]), "another type");
    assert!(e.contains("0x80000260"), "{e}");
    // the donor's field holds no string
    let e = refuse(&doc, donor(vec![description(None)]), "no string");
    assert!(e.contains("holds no string"), "{e}");
    // the document's slot belongs to no DataField
    let plain = with_data_field(base.clone(), i64::from(ID), CSTRING, slot + 1);
    let e = refuse(&plain, donor(vec![description(Some(b"x"))]), "not a field");
    assert!(e.contains("belongs to no"), "{e}");
    // the document's field is not a CString
    let float_field = with_data_field(base.clone(), i64::from(ID), 0x8000_0260, slot);
    let e = refuse(
        &float_field,
        donor(vec![description(Some(b"x"))]),
        "not a CString",
    );
    assert!(e.contains("not a CString"), "{e}");
    // a pointer outside the document that is no boxed value at all
    let mut loose = with_data_field(base, i64::from(ID), CSTRING, slot);
    loose.pointer_offsets.push(0);
    loose.payload[..4].copy_from_slice(&(-9i32).to_le_bytes());
    let e = refuse(&loose, donor(vec![description(Some(b"x"))]), "not a boxed value");
    assert!(e.contains("not the pointer of a boxed value"), "{e}");
}

/// The game's data folder and Oodle, when configured.
fn game() -> R<Option<(Folder, Oodle)>> {
    let (Some(data), Some(dll)) = (install_file(Game::Skate, "Data"), tool(Tool::Oodle9)) else {
        return Ok(None);
    };
    // SAFETY: the configuration names the game's own Oodle DLL as tools.oodle9.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V9) }?;
    Ok(Some((Folder::open(data)?, oodle)))
}

#[test]
fn the_games_level_root_is_the_donor() -> R {
    let Some((folder, oodle)) = game()? else {
        return Ok(());
    };
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&oodle),
    };
    let d = Donor::find(&src, DONOR_TOC, DONOR_BUNDLE, DONOR_EBX)?;
    assert_eq!(d.name, DONOR_EBX);
    let field = d.field(ID).expect("the template's description field");
    assert_eq!(field.encoded_type, CSTRING);
    let text = field.text.as_ref().expect("a CString holds its string");
    assert!(!text.is_empty() && text.is_ascii());
    // the other defaults are values, not pointers
    assert!(d.fields.iter().filter(|f| f.id != ID).all(|f| f.text.is_none()));
    // a missing bundle or document is refused
    assert!(Donor::find(&src, DONOR_TOC, "no/such/bundle", DONOR_EBX).is_err());
    assert!(Donor::find(&src, DONOR_TOC, DONOR_BUNDLE, "levels/game/no_such_level").is_err());
    Ok(())
}

/// A document's place (TOC, bundle, name) and what its manifest says of it (size, sha1).
type Documents = BTreeMap<(String, String, String), (u32, [u8; 20])>;

/// Every placed EBX document of a folder.
fn documents(src: &Source) -> R<Documents> {
    let mut out = BTreeMap::new();
    for rel in &src.folder.tocs {
        let t = src.folder.toc(rel)?;
        for (name, region) in &t.bundles {
            let (files, inline) = bundle::read_region(region)?;
            let Some((m, first, _)) = src.manifest(&files, &inline)? else {
                continue;
            };
            for (i, e) in m.ebx.iter().enumerate() {
                if files[first + i].0.patch {
                    out.insert((rel.clone(), name.clone(), e.name.clone()), (e.size, e.sha1));
                }
            }
        }
    }
    Ok(out)
}

/// Both folders' files, relative name -> bytes.
fn files(dir: &Path) -> R<BTreeMap<String, Vec<u8>>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) -> R {
        for e in std::fs::read_dir(dir)? {
            let p = e?.path();
            if p.is_dir() {
                walk(root, &p, out)?;
            } else {
                out.insert(
                    p.strip_prefix(root)?.to_string_lossy().replace('\\', "/"),
                    std::fs::read(&p)?,
                );
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out)?;
    Ok(out)
}

#[test]
fn studio_level_root_is_repaired() -> R {
    let (Some(map), Some(patch), Some((base, oodle))) = (
        oracle(OracleSet::Baseline, Game::Gta, "legion/map"),
        oracle(OracleSet::Studio, Game::Gta, "legion/patch"),
        game()?,
    ) else {
        return Ok(());
    };
    let folder = Folder::open(&patch)?;
    let src = Source {
        folder: &folder,
        base: Some(&base),
        oodle: Some(&oodle),
    };
    let donor_src = Source {
        folder: &base,
        base: Some(&base),
        oodle: Some(&oodle),
    };
    let donor = Donor::find(&donor_src, DONOR_TOC, DONOR_BUNDLE, DONOR_EBX)?;
    // ReSkate Studio 20261012 writes the template's distance into the level root: the Patch is refused for it
    let e = levelroot::check(&src).expect_err("Studio's level root points out of its document");
    assert!(e.to_string().contains("(a boxed value)"), "{e}");

    let scratch = primase_testdata::scratch().join("reskate-levelroot");
    #[allow(clippy::disallowed_methods, reason = "test scratch folder")]
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch)?;
    }
    let (a, b) = (scratch.join("a"), scratch.join("b"));
    // through the one public route (the blend stage first, which rewrites no EBX document; the layering gate after)
    let report = finish(&map, &patch, &a, &base, &oodle)?.level_root;
    assert_eq!(report.repaired.len(), 1, "{:?}", report.repaired);
    let r = &report.repaired[0];
    assert_eq!(r.repairs.len(), 1);
    assert_eq!(r.repairs[0].id, ID);
    assert_eq!(Some(&r.repairs[0].value), donor.field(ID).unwrap().text.as_ref());

    // the repaired copy passes the gate, reads the same documents, and differs in the one document
    let out = Folder::open(&a)?;
    let out_src = Source {
        folder: &out,
        base: Some(&base),
        oodle: Some(&oodle),
    };
    assert_eq!(levelroot::check(&out_src)?, report.documents);
    let (before, after) = (documents(&src)?, documents(&out_src)?);
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>()
    );
    let changed: Vec<_> = before
        .iter()
        .filter(|(k, v)| after[*k] != **v)
        .map(|(k, _)| k)
        .collect();
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(changed[0].2, r.document);

    // a repeat gives the same files, and finishing the finished copy (as a map with no blend record) changes nothing
    finish(&map, &patch, &b, &base, &oodle)?;
    assert_eq!(files(&a)?, files(&b)?);
    let (plain, c) = (scratch.join("no-blend-map"), scratch.join("c"));
    #[allow(clippy::disallowed_methods, reason = "test scratch folder")]
    std::fs::create_dir_all(&plain)?;
    let again = finish(&plain, &a, &c, &base, &oodle)?.level_root;
    assert!(again.repaired.is_empty());
    assert_eq!(files(&a)?, files(&c)?);
    Ok(())
}
