//! Census over a Unity player's data folder (`PRIMASE_UNITY_DATA`) with a type tree package (`PRIMASE_UNITY_TPK`):
//! every SerializedFile under it parsed, every object decoded by its class's type tree, which must cover the object's
//! bytes exactly (a MonoBehaviour's script fields after its base fields are counted, not decoded; an object the
//! package's layout for its Unity version falls short of is listed by name), every streamed resource it names inside
//! its file. Files of container versions not read are listed. Counts by class.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use primase_core::gate::{GateReport, Tolerance, real_data};
use primase_unity::resource::{StreamingInfo, streaming_infos};
use primase_unity::{Error, SerializedFile, TypeTree, TypeTreePackage};

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            files(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// A SerializedFile header of any version (legacy fields: the version at 8, big-endian, small).
fn serialized_version(b: &[u8]) -> Option<u32> {
    if b.len() < 20 {
        return None;
    }
    let v = u32::from_be_bytes(b[8..12].try_into().unwrap());
    let meta = u32::from_be_bytes(b[0..4].try_into().unwrap());
    ((6..=40).contains(&v) && (meta == 0 || (meta as usize) < b.len())).then_some(v)
}

#[test]
fn every_object_decodes() {
    let (Some(data), Some(tpk)) = (real_data("PRIMASE_UNITY_DATA"), real_data("PRIMASE_UNITY_TPK")) else {
        return;
    };
    let package = TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap();
    let mut paths = Vec::new();
    files(&data, &mut paths);
    let mut objects = GateReport::new("objects decoded by their type trees", Tolerance::Exact);
    let mut streams = GateReport::new("streamed resources inside their files", Tolerance::Exact);
    let mut by_class: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let (mut script_objects, mut script_bytes) = (0u64, 0u64);
    let mut refused: Vec<String> = Vec::new();
    // objects the package's tree for this Unity version does not cover (its layout is short of the data)
    let mut short: Vec<String> = Vec::new();
    let mut read = 0;
    for path in &paths {
        let bytes = std::fs::read(path).unwrap();
        let rel = path.strip_prefix(&data).unwrap().display().to_string();
        let Some(version) = serialized_version(&bytes) else {
            continue;
        };
        let file = match SerializedFile::parse(bytes) {
            Ok(f) => f,
            Err(Error::Unsupported(why)) => {
                refused.push(format!("{rel}: {why}"));
                continue;
            }
            Err(e) if !SerializedFile::looks_like(&std::fs::read(path).unwrap()) => {
                // a file that only looked like one by its first bytes
                refused.push(format!("{rel}: not a SerializedFile v{version}? {e}"));
                continue;
            }
            Err(e) => panic!("{rel}: {e}"),
        };
        read += 1;
        let mut trees: HashMap<i32, Result<TypeTree, String>> = HashMap::new();
        for o in &file.objects {
            let name = package
                .class_name(o.class_id, file.unity)
                .map_or_else(|| format!("class {}", o.class_id), str::to_owned);
            let entry = by_class.entry(name).or_default();
            entry.0 += 1;
            entry.1 += u64::from(o.size);
            let item = format!("{rel}/{}", o.path_id);
            let tree = trees.entry(o.class_id).or_insert_with(|| {
                package
                    .type_tree(o.class_id, file.unity)
                    .map_err(|e| e.to_string())
            });
            let tree = match tree {
                Ok(t) => t,
                Err(e) => {
                    objects.fail(&item, e.clone());
                    continue;
                }
            };
            match file.decode_with(o, tree) {
                Ok(d) => {
                    objects.record(&item, 0.0);
                    if !d.covered(o.class_id) {
                        short.push(format!(
                            "{item} (class {}): {} bytes after the tree",
                            o.class_id, d.rest
                        ));
                    } else if d.rest > 0 {
                        script_objects += 1;
                        script_bytes += d.rest as u64;
                    }
                    for (at, v) in streaming_infos(&d.value) {
                        match StreamingInfo::of(v) {
                            Ok(None) => {}
                            Ok(Some(s)) => {
                                let f = s.file_in(path.parent().unwrap()).unwrap();
                                let len = std::fs::metadata(&f).map(|m| m.len());
                                match len {
                                    Ok(n) if s.offset + s.size <= n => {
                                        streams.record(format!("{item}{at}"), 0.0)
                                    }
                                    Ok(n) => streams.fail(
                                        format!("{item}{at}"),
                                        format!("{}+{} past {n} bytes of {}", s.offset, s.size, s.path),
                                    ),
                                    Err(e) => streams.fail(format!("{item}{at}"), format!("{}: {e}", s.path)),
                                }
                            }
                            Err(e) => streams.fail(format!("{item}{at}"), e.to_string()),
                        }
                    }
                }
                Err(e) => objects.fail(&item, e.to_string()),
            }
        }
    }
    println!("{read} SerializedFiles read under {}", data.display());
    for r in &refused {
        println!("  not read: {r}");
    }
    println!("{objects}\n{streams}");
    println!(
        "MonoBehaviour script fields not decoded (no script type trees in a stripped build): {script_objects} objects, \
         {script_bytes} bytes"
    );
    println!(
        "objects the type tree package's layout does not cover (decoded up to its end): {}",
        short.len()
    );
    for x in &short {
        println!("  {x}");
    }
    println!("{} classes (objects, bytes):", by_class.len());
    for (k, (n, b)) in &by_class {
        println!("  {k}: {n}, {b}");
    }
    objects.assert_passed();
    assert!(streams.failures.is_empty(), "{streams}");
}
