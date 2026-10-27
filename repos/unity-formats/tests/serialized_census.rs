//! Exact object-table comparison against an independent UnityPy census.

#![allow(clippy::unwrap_used)]

mod inputs;

use inputs::{Game, OracleSet, install_file, oracle};
use std::collections::BTreeMap;

use serde::Deserialize;
use unity_formats::SerializedFile;

#[derive(Deserialize)]
struct Census {
    files: BTreeMap<String, CensusFile>,
    totals: Totals,
}

#[derive(Deserialize)]
struct CensusFile {
    size: u64,
    version: u32,
    unity_version: String,
    object_count: usize,
    /// `[path_id, class_id, byte_start, byte_size]`, sorted by path id.
    objects: Vec<[i64; 4]>,
}

#[derive(Deserialize)]
struct Totals {
    files: usize,
    objects: usize,
    file_bytes: u64,
    object_bytes: u64,
}

/// An object's identity and bytes as UnityPy reports them.
type Entry = [i64; 3]; // class id, offset, size

fn entry(objects: [i64; 4]) -> (i64, Entry) {
    let [path_id, class_id, offset, size] = objects;
    (path_id, [class_id, offset, size])
}

#[test]
fn matches_unitypy_census() {
    let (Some(data), Some(census)) = (
        install_file(Game::Skatebird, "SkateBIRD_Data"),
        oracle(OracleSet::UnityPy, Game::Skatebird, "census.json"),
    ) else {
        return;
    };
    let census: Census = serde_json::from_slice(&std::fs::read(&census).unwrap()).unwrap();
    assert!(!census.files.is_empty(), "census lists no files");

    let (mut objects, mut file_bytes, mut object_bytes) = (0usize, 0u64, 0u64);
    for (name, expected) in &census.files {
        let bytes = std::fs::read(data.join(name)).unwrap();
        assert_eq!(
            bytes.len() as u64,
            expected.size,
            "{name}: file size (UnityPy census)"
        );
        let file = SerializedFile::parse(bytes).unwrap();

        assert_eq!(file.version, expected.version, "{name}: container version");
        assert_eq!(
            file.unity_version, expected.unity_version,
            "{name}: Unity version string"
        );
        assert_eq!(
            file.objects.len(),
            expected.object_count,
            "{name}: object count (census lists {})",
            expected.object_count
        );

        let theirs: BTreeMap<i64, Entry> = expected.objects.iter().copied().map(entry).collect();
        assert_eq!(
            theirs.len(),
            expected.object_count,
            "{name}: census object table has duplicate path ids"
        );
        let ours: BTreeMap<i64, Entry> = file
            .objects
            .iter()
            .map(|o| {
                (
                    o.path_id,
                    [i64::from(o.class_id), o.offset as i64, i64::from(o.size)],
                )
            })
            .collect();
        assert_eq!(
            ours.len(),
            file.objects.len(),
            "{name}: primase object table has duplicate path ids"
        );
        assert_eq!(ours.len(), theirs.len(), "{name}: object table length");
        for (path_id, want) in &theirs {
            let [class_id, offset, size] = *want;
            match ours.get(path_id) {
                None => panic!(
                    "{name}: path id {path_id} missing from primase (UnityPy: class {class_id}, \
                     offset {offset}, size {size})"
                ),
                Some(got) if *got != *want => {
                    let [got_class, got_offset, got_size] = *got;
                    panic!(
                        "{name}: path id {path_id}: primase class {got_class}, offset {got_offset}, \
                         size {got_size}; UnityPy class {class_id}, offset {offset}, size {size}"
                    );
                }
                Some(_) => {}
            }
        }

        objects += file.objects.len();
        file_bytes += expected.size;
        object_bytes += file.objects.iter().map(|o| u64::from(o.size)).sum::<u64>();
    }

    assert_eq!(census.files.len(), census.totals.files, "totals: files");
    assert_eq!(objects, census.totals.objects, "totals: objects");
    assert_eq!(file_bytes, census.totals.file_bytes, "totals: file bytes");
    assert_eq!(object_bytes, census.totals.object_bytes, "totals: object bytes");
    eprintln!(
        "matched {} files, {objects} objects, {file_bytes} file bytes, {object_bytes} object bytes",
        census.files.len()
    );
}
