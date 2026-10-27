#![allow(clippy::unwrap_used)]
//! The container reader reads to the exact end and refuses the layouts the corpora never have. Containers are built by hand
//! (the format's own rules) and the committed fixtures of the two lifters are read as containers of real compilers.
use std::path::Path;

use dxcontainer::{checksum, chunks};

/// A container of these parts, directory first and the parts one after another.
fn container(parts: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let directory = 32 + 4 * parts.len();
    let mut offsets = Vec::new();
    let mut body = Vec::new();
    for (kind, data) in parts {
        offsets.push((directory + body.len()) as u32);
        body.extend_from_slice(*kind);
        body.extend_from_slice(&(data.len() as u32).to_le_bytes());
        body.extend_from_slice(data);
    }
    let mut out = b"DXBC".to_vec();
    out.extend_from_slice(&[0; 16]);
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&((directory + body.len()) as u32).to_le_bytes());
    out.extend_from_slice(&(parts.len() as u32).to_le_bytes());
    for offset in offsets {
        out.extend_from_slice(&offset.to_le_bytes());
    }
    out.extend_from_slice(&body);
    out
}

fn set(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn refusal(bytes: &[u8]) -> String {
    chunks(bytes).unwrap_err().to_string()
}

#[test]
fn parts_are_read_in_the_directorys_order_with_their_bytes() {
    // lengths that are not multiples of 4: Shader Model 6 parts are not padded
    let bytes = container(&[(b"ISG1", &[1, 2, 3]), (b"DXIL", &[4, 5, 6, 7, 8]), (b"EMPT", &[])]);
    let parts = chunks(&bytes).unwrap();
    let read: Vec<_> = parts.iter().map(|p| (p.kind, p.data.to_vec())).collect();
    assert_eq!(
        read,
        [
            (*b"ISG1", vec![1, 2, 3]),
            (*b"DXIL", vec![4, 5, 6, 7, 8]),
            (*b"EMPT", vec![])
        ]
    );
    assert!(chunks(&container(&[])).unwrap().is_empty());
}

#[test]
fn what_the_corpora_never_have_is_refused_by_name() {
    let good = container(&[(b"AAAA", &[1, 1, 1, 1]), (b"BBBB", &[2, 2])]);
    chunks(&good).unwrap();

    let mut bad = good.clone();
    bad[0] = b'X';
    assert!(refusal(&bad).contains("not DXBC"));

    let mut bad = good.clone();
    set(&mut bad, 20, 2);
    assert!(refusal(&bad).contains("version is not 1"));

    let mut bad = good.clone();
    set(&mut bad, 24, good.len() as u32 + 1);
    assert!(refusal(&bad).contains("states a size of"));

    let mut bad = good.clone();
    set(&mut bad, 28, u32::MAX);
    assert!(refusal(&bad).contains("does not fit the container"));

    // the second part starting inside the first, and after a gap
    let mut bad = good.clone();
    let first = u32::from_le_bytes(good[32..36].try_into().unwrap());
    set(&mut bad, 36, first + 4);
    assert!(refusal(&bad).contains("overlapping parts: part 1"));
    set(&mut bad, 36, first + 100);
    assert!(refusal(&bad).contains("part 1") && !refusal(&bad).contains("overlapping"));

    // a byte after the last part
    let mut bad = good.clone();
    bad.push(0);
    let size = bad.len() as u32;
    set(&mut bad, 24, size);
    assert!(refusal(&bad).contains("1 bytes after the last part"));

    // a part longer than the container
    let mut bad = good.clone();
    set(&mut bad, first as usize + 4, 1000);
    assert!(refusal(&bad).contains("part 0 (AAAA)"));

    // a gap between the directory and the first part
    let mut gap = container(&[(b"AAAA", &[1])]);
    gap.splice(36..36, [0u8; 4]);
    let size = gap.len() as u32;
    set(&mut gap, 24, size);
    set(&mut gap, 32, 40);
    assert!(refusal(&gap).contains("bytes between parts: part 0"));

    // a kind twice
    let twice = container(&[(b"AAAA", &[1]), (b"AAAA", &[2])]);
    assert!(refusal(&twice).contains("a part kind twice: AAAA"));
}

#[test]
fn every_truncation_is_refused() {
    let bytes = container(&[(b"AAAA", &[1, 1, 1, 1]), (b"BBBB", &[2, 2])]);
    for n in 0..bytes.len() {
        assert!(chunks(&bytes[..n]).is_err(), "prefix {n}");
    }
}

#[test]
fn the_committed_fixtures_of_both_lifters_are_read_to_their_end() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut read = 0;
    for lifter in ["dxbc", "dxil"] {
        for entry in std::fs::read_dir(root.join(lifter).join("tests/fixtures")).unwrap() {
            let path = entry.unwrap().path();
            if !path.extension().is_some_and(|e| e == "dxbc" || e == "dxil") {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let parts = chunks(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let total: usize = 32 + 4 * parts.len() + parts.iter().map(|p| 8 + p.data.len()).sum::<usize>();
            assert_eq!(total, bytes.len(), "{}", path.display());
            read += 1;
        }
    }
    assert!(read > 100, "{read} fixtures");
}

#[test]
fn the_digest_is_md5_of_the_bytes_from_20_as_the_compiler_pads_it() {
    // an empty container's digest covers its 12 bytes after the digest field (version 1, size 32, no parts)
    let empty = container(&[]);
    let digest = checksum(&empty).unwrap();
    assert_eq!(checksum(&empty[..19]), None);
    assert_ne!(digest, [0; 16]);
    let mut other = empty.clone();
    other[28] = 1;
    assert_ne!(checksum(&other), Some(digest));
}
