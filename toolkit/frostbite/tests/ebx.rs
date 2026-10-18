//! EBX gates: committed synthetic documents read to their expected values and written back byte for byte, values
//! carried between documents, and a census over real documents named by `PRIMASE_FROSTBITE_EBX`.
//!
//! BASELINE: the synthetic fixtures were produced from the writer's own layout rules (a standalone layout script) and
//! checked with a throwaway trace reader, so their byte-identical read->write assertions detect regressions; they are
//! not independent correctness evidence. The values each fixture is asserted to decode to are hand-worked from its
//! bytes.
//!
//! Our EBX code is a port of ReSkate's, so comparing our bytes with the reference writer's bytes (`reskate_cli
//! ebx-rewrite-diff`) measures agreement with the port's source, not independent correctness. The independent check is
//! the `PRIMASE_FROSTBITE_EBX` corpus of stock documents: every field's value must survive a rewrite, and one stock
//! document's fields are spot-checked against ReSkate's C++ reader output.
//!
//! `refl.ebx` carries a REFL reflection table with an exported root, an internal child, a pointer, a CString and two
//! arrays (scalars and structs); `rfl2.ebx` carries an RFL2 hashed numbered table with a boxed value, an external
//! import and an array.

#![allow(clippy::unwrap_used)]

use primase_frostbite::ebx::{self, Document, Pointer, Value, carry};

const REFL: &[u8] = include_bytes!("fixtures/ebx/refl.ebx");
const RFL2: &[u8] = include_bytes!("fixtures/ebx/rfl2.ebx");

type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn object_field<'a>(doc: &'a Document, instance: usize, name: &str) -> &'a Value {
    doc.instances[instance]
        .object
        .as_ref()
        .unwrap()
        .field(name)
        .unwrap()
}

#[test]
fn refl_document_reads_and_writes_identically() -> R {
    let doc = ebx::read(REFL)?;
    assert_eq!(doc.file_guid, [0xA0; 16]);
    assert_eq!(doc.root_type, "Root");
    assert!(!doc.hashed_reflection_names);
    assert_eq!(doc.instances.len(), 2);
    assert!(doc.imports.is_empty());
    assert_eq!(doc.arrays.len(), 2);
    assert!(doc.boxed_values.is_empty());

    assert!(doc.instances[0].exported);
    assert_eq!(doc.instances[0].guid, [0xAB; 16]);
    assert_eq!(doc.type_name(0), Some("Root"));
    assert!(!doc.instances[1].exported);
    assert_eq!(doc.type_name(1), Some("Child"));

    assert_eq!(object_field(&doc, 0, "flags"), &Value::Bool(true));
    assert_eq!(object_field(&doc, 0, "id"), &Value::Int(-7));
    assert_eq!(object_field(&doc, 0, "scale"), &Value::Float(1.5));
    assert_eq!(object_field(&doc, 0, "label"), &Value::Str(b"hello".to_vec()));
    assert_eq!(object_field(&doc, 0, "name"), &Value::Str(b"root".to_vec()));
    assert_eq!(
        object_field(&doc, 0, "child"),
        &Value::Pointer(Pointer::Internal(1))
    );
    assert_eq!(
        object_field(&doc, 0, "ids"),
        &Value::Array(vec![Value::Int(1), Value::Int(2), Value::Int(3)], Some(0xA1))
    );
    assert_eq!(object_field(&doc, 1, "count"), &Value::UInt(42));
    let items = match object_field(&doc, 1, "items") {
        Value::Array(items, Some(0xA2)) => items,
        other => panic!("Child.items is not the struct array the fixture holds: {other:?}"),
    };
    assert_eq!(items.len(), 2);
    for (item, want) in items.iter().zip([100, 200]) {
        let Value::Object(o) = item else {
            panic!("Child.items element is not a structure");
        };
        assert_eq!(o.field("value"), Some(&Value::Int(want)));
    }

    assert_eq!(ebx::write(&doc)?, REFL);
    Ok(())
}

#[test]
fn rfl2_document_reads_and_writes_identically() -> R {
    let doc = ebx::read(RFL2)?;
    assert_eq!(doc.file_guid, [0xB0; 16]);
    assert_eq!(doc.root_type, "Root2");
    assert!(doc.hashed_reflection_names);
    assert_eq!(doc.instances.len(), 1);
    assert_eq!(doc.imports.len(), 1);
    assert_eq!(doc.imports[0].file_guid, [0xB1; 16]);
    assert_eq!(doc.imports[0].class_guid, [0xB2; 16]);
    assert_eq!(doc.type_name(0), Some("Root2"));

    assert_eq!(
        object_field(&doc, 0, "boxed"),
        &Value::Boxed(ebx::BoxedRef {
            encoded_type: 0x0003_0005,
            data_offset: 112,
            relative_offset: 80,
        })
    );
    assert_eq!(
        object_field(&doc, 0, "other"),
        &Value::Pointer(Pointer::External(0))
    );
    assert_eq!(
        object_field(&doc, 0, "values"),
        &Value::Array(vec![Value::Int(7)], Some(0xA3))
    );

    assert_eq!(doc.boxed_values.len(), 1);
    assert_eq!(doc.boxed_values[0].offset, 112);
    assert_eq!(doc.boxed_values[0].raw, (0x30..0x40).collect::<Vec<u8>>());
    assert_eq!(doc.boxed_reference_offsets, [24]);

    assert_eq!(ebx::write(&doc)?, RFL2);
    Ok(())
}

#[test]
fn carrying_appends_a_reachable_instance() -> R {
    let from = ebx::read(REFL)?;
    let mut to = ebx::read(REFL)?;
    {
        let mut c = carry::Carrier::new(&from, &mut to);
        let carried = c.value(&Value::Pointer(Pointer::Internal(1)))?;
        assert_eq!(carried, Value::Pointer(Pointer::Internal(2)));
        assert_eq!(c.instances, 1);
        assert_eq!(c.renumbered, 0);
    }
    assert_eq!(to.instances.len(), 3);
    assert_eq!(to.type_name(2), Some("Child"));
    assert_eq!(object_field(&to, 2, "count"), &Value::UInt(42));

    // Point the root at the carried copy so it is reachable, and check it survives a write.
    let root = to.instances[0].object.as_mut().unwrap();
    *root.field_mut("child").unwrap() = Value::Pointer(Pointer::Internal(2));
    assert!(!carry::order_exported_first(&mut to));

    let back = ebx::read(&ebx::write(&to)?)?;
    assert_eq!(back.instances.len(), 3);
    assert_eq!(
        object_field(&back, 0, "child"),
        &Value::Pointer(Pointer::Internal(2))
    );
    assert_eq!(object_field(&back, 2, "count"), &Value::UInt(42));
    let items = match object_field(&back, 2, "items") {
        Value::Array(items, Some(0xA2)) => items,
        other => panic!("the carried child lost its struct array: {other:?}"),
    };
    assert_eq!(items.len(), 2);
    Ok(())
}

#[test]
fn compact_drops_what_the_root_cannot_reach() -> R {
    let from = ebx::read(REFL)?;
    let mut to = ebx::read(REFL)?;
    {
        let mut c = carry::Carrier::new(&from, &mut to);
        c.value(&Value::Pointer(Pointer::Internal(1)))?;
    }
    assert_eq!(to.instances.len(), 3);
    assert_eq!(carry::compact(&mut to), (2, 0));
    assert_eq!(to.instances.len(), 2);

    let back = ebx::read(&ebx::write(&to)?)?;
    assert_eq!(back.instances.len(), 2);
    assert_eq!(
        object_field(&back, 0, "child"),
        &Value::Pointer(Pointer::Internal(1))
    );
    Ok(())
}

#[test]
fn exported_instances_are_ordered_first() -> R {
    let mut doc = ebx::read(REFL)?;
    doc.instances.swap(0, 1);
    // The root is now instance 1 and its child instance 0.
    let root = doc.instances[1].object.as_mut().unwrap();
    *root.field_mut("child").unwrap() = Value::Pointer(Pointer::Internal(0));

    assert!(carry::order_exported_first(&mut doc));
    assert!(doc.instances[0].exported);
    assert!(!doc.instances[1].exported);

    let back = ebx::read(&ebx::write(&doc)?)?;
    assert_eq!(back.type_name(0), Some("Root"));
    assert_eq!(back.type_name(1), Some("Child"));
    assert_eq!(
        object_field(&back, 0, "child"),
        &Value::Pointer(Pointer::Internal(1))
    );
    Ok(())
}

#[test]
fn carrying_shares_imports() -> R {
    let from = ebx::read(RFL2)?;
    let mut to = ebx::read(REFL)?;
    assert!(to.imports.is_empty());
    {
        let mut c = carry::Carrier::new(&from, &mut to);
        assert_eq!(
            c.value(&Value::Pointer(Pointer::External(0)))?,
            Value::Pointer(Pointer::External(0))
        );
        assert_eq!(c.instances, 0);
    }
    assert_eq!(to.imports.len(), 1);
    assert_eq!(to.imports[0].file_guid, [0xB1; 16]);

    // Carrying the same import again shares it instead of appending a second copy.
    {
        let mut c = carry::Carrier::new(&from, &mut to);
        assert_eq!(
            c.value(&Value::Pointer(Pointer::External(0)))?,
            Value::Pointer(Pointer::External(0))
        );
    }
    assert_eq!(to.imports.len(), 1);
    ebx::read(&ebx::write(&to)?)?;
    Ok(())
}

/// An EBX document's values without their layout: instances (type name, export, GUID, values with boxed references
/// named by rank), imports and the file GUID. The writer may lay the same values out differently.
fn meaning(d: &Document) -> String {
    fn walk(d: &Document, v: &mut Value) {
        match v {
            Value::Boxed(b) => {
                b.data_offset = d
                    .boxed_values
                    .iter()
                    .position(|r| i64::from(r.offset) == b.data_offset)
                    .map_or(-1, |r| r as i64);
                b.relative_offset = 0;
            }
            Value::Array(items, h) => {
                *h = None;
                items.iter_mut().for_each(|x| walk(d, x));
            }
            Value::Object(o) => {
                o.descriptor = 0;
                for f in &mut o.fields {
                    f.descriptor = 0;
                    walk(d, &mut f.value);
                }
            }
            Value::TypeRef(t) => t.descriptor = None,
            _ => {}
        }
    }
    let mut out = format!("file {:?} imports {:?}\n", d.file_guid, d.imports);
    for (i, inst) in d.instances.iter().enumerate() {
        let mut o = inst.object.clone();
        if let Some(o) = &mut o {
            o.descriptor = 0;
            for f in &mut o.fields {
                f.descriptor = 0;
                walk(d, &mut f.value);
            }
        }
        out.push_str(&format!(
            "{i} {} {} {:?} {o:?}\n",
            d.type_name(i).unwrap_or("?"),
            inst.exported,
            inst.guid
        ));
    }
    out
}

fn ebx_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<_>>()?;
    entries.sort();
    for p in entries {
        if p.is_dir() {
            ebx_files(&p, out)?;
        } else if p.extension().is_some_and(|e| e == "ebx") {
            out.push(p);
        }
    }
    Ok(())
}

/// Real-data census over documents as the game wrote them.
///
/// Every document must keep its typed values through read -> write -> read, field by field. The byte layout is
/// compared with the reference writer: `reskate_cli ebx-rewrite-diff <game root> <asset>` says whether ReSkate's C++
/// writer reproduces the stock bytes. Where it does, ours must too; where it does not, the document is counted and
/// named instead of failed. One stock document's fields are additionally spot-checked against what the same C++
/// reader prints (`ebx-values`), a third-party decode rather than ours.
#[test]
fn real_documents_census() -> R {
    let Some(dir) = primase_testdata::real_data_value("PRIMASE_FROSTBITE_EBX") else {
        eprintln!("SKIPPED: PRIMASE_FROSTBITE_EBX not set");
        return Ok(());
    };
    let mut files = Vec::new();
    ebx_files(std::path::Path::new(&dir), &mut files)?;
    files.sort();
    assert!(!files.is_empty(), "PRIMASE_FROSTBITE_EBX holds no .ebx documents");

    let reference = match (
        primase_testdata::real_data_value("PRIMASE_RESKATE_CLI"),
        primase_testdata::real_data_value("PRIMASE_SKATE_DATA"),
    ) {
        (Some(cli), Some(data)) => {
            let root = std::path::Path::new(&data)
                .parent()
                .ok_or("PRIMASE_SKATE_DATA has no parent to use as the game root")?
                .to_path_buf();
            Some((std::path::PathBuf::from(cli), root))
        }
        _ => {
            eprintln!(
                "SKIPPED: reference writer comparison (set PRIMASE_RESKATE_CLI and PRIMASE_SKATE_DATA)"
            );
            None
        }
    };

    let mut values_changed = Vec::new();
    let mut layout_mismatched = Vec::new();
    let mut reference_differs = Vec::new();
    let mut reference_identical = 0;
    for path in &files {
        let bytes = std::fs::read(path)?;
        let doc = ebx::read(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        assert_eq!(
            ebx::read_file_guid(&bytes)?,
            doc.file_guid,
            "{}: file GUID",
            path.display()
        );
        let written = ebx::write(&doc)?;
        let again = ebx::read(&written).map_err(|e| format!("{}: {e}", path.display()))?;
        if meaning(&doc) != meaning(&again) {
            values_changed.push(path.display().to_string());
        }
        let Some((cli, root)) = &reference else {
            continue;
        };
        let asset = path
            .strip_prefix(&dir)
            .map_err(|_| format!("{}: outside PRIMASE_FROSTBITE_EBX", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        let asset = asset.strip_suffix(".ebx").unwrap_or(asset.as_str()).to_string();
        let out = std::process::Command::new(cli)
            .arg("ebx-rewrite-diff")
            .arg(root)
            .arg(&asset)
            .output()
            .map_err(|e| format!("{}: cannot run {}: {e}", path.display(), cli.display()))?;
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{}: the reference reader failed: {text}{}",
            path.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        let differing = text
            .lines()
            .find_map(|l| l.split_once("differing-bytes:"))
            .and_then(|(_, n)| n.split_whitespace().next())
            .and_then(|n| n.parse::<usize>().ok())
            .ok_or_else(|| {
                format!(
                    "{}: the reference writer printed no differing-bytes line: {text}",
                    path.display()
                )
            })?;
        if differing == 0 {
            reference_identical += 1;
            if written != bytes {
                layout_mismatched.push(asset);
            }
        } else {
            reference_differs.push(format!("{asset} ({differing} bytes)"));
        }
    }
    assert!(
        values_changed.is_empty(),
        "values changed by the rewrite: {values_changed:?}"
    );
    assert!(
        layout_mismatched.is_empty(),
        "our writer differs from stock where the reference writer reproduces it: {layout_mismatched:?}"
    );
    match &reference {
        Some(_) => eprintln!(
            "EBX census: {} documents, values preserved; {reference_identical} byte-identical under both writers; \
             the reference writer itself differs for {}: {reference_differs:?}",
            files.len(),
            reference_differs.len()
        ),
        None => eprintln!(
            "EBX census: {} documents, values preserved; byte layout not compared",
            files.len()
        ),
    }

    // The stock bundle reference table, as ReSkate's reader prints it: root and instance 0's scalar/string fields.
    let stock = files.iter().find(|p| {
        p.to_string_lossy()
            .replace('\\', "/")
            .to_lowercase()
            .ends_with("characters/customization/configs/cas_main_bundlereftable.ebx")
    });
    if let Some(path) = stock {
        let doc = ebx::read(&std::fs::read(path)?)?;
        assert_eq!(doc.root_type, "DingoCharacterBundleRefTable");
        assert_eq!(
            object_field(&doc, 0, "Name"),
            &Value::Str(b"Characters/Customization/Configs/CAS_Main_BundleRefTable".to_vec())
        );
        assert_eq!(object_field(&doc, 0, "IsInLaunchChunk"), &Value::Bool(true));
        assert_eq!(
            object_field(&doc, 0, "BundleRefTableResource"),
            &Value::Resource(0x99AA_73AB_AEF7_AD57)
        );
        assert!(
            matches!(object_field(&doc, 0, "Priority"), Value::Int(0) | Value::UInt(0)),
            "Priority is not zero: {:?}",
            object_field(&doc, 0, "Priority")
        );
    }
    Ok(())
}
