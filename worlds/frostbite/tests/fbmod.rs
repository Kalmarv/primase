//! `.fbmod` gates: hand-built containers of every entry kind read back to the same values and write again byte for
//! byte, the bytes of a small one are checked against the layout worked by hand, and what the reader refuses is
//! refused. The real-data gate reads every file of `PRIMASE_RESKATE_FBMOD`.
//!
//! BASELINE: the real files are ReSkate Studio's own output (`cosmetic-mesh-import`, 2026-10-12), a round-trip baseline
//! of our producer's files, not an oracle. Each file's entry listing (kind, name, decoded size, stored size, one line
//! per entry, printed with `--nocapture`) was compared by hand with the Python reference reader's; every payload must
//! decode to the size its entry states.

#![allow(clippy::unwrap_used)]

use primase_frostbite::fbmod::{self, Body, ChunkInfo, Entry, EntryKind, Fbmod, ResInfo, Stored};

type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn stored(index: usize, size: u64) -> Option<Stored> {
    Some(Stored {
        index,
        size,
        flags: 8,
        handler: 0,
        user: "t".into(),
    })
}

/// One of each kind: an embedded file without a payload, an EBX and a resource sharing a payload, a chunk with its
/// own; the second payload is over 64 KiB, so it takes two CAS blocks.
fn sample() -> Fbmod {
    let mut m = Fbmod {
        version: 6,
        profile: "Skate".into(),
        game_version: 625200,
        title: "t".into(),
        author: "a".into(),
        category: String::new(),
        mod_version: "1.0".into(),
        description: String::new(),
        link: String::new(),
        entries: Vec::new(),
        payloads: Vec::new(),
    };
    let small = m.add_payload(b"hello").unwrap();
    let big: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
    let large = m.add_payload(&big).unwrap();
    m.entries = vec![
        Entry {
            name: "Icon".into(),
            stored: None,
            bundles: vec![],
            body: Body::Embedded,
        },
        Entry {
            name: "a/b".into(),
            stored: stored(small, 5),
            bundles: vec![7],
            body: Body::Ebx,
        },
        Entry {
            name: "a/b".into(),
            stored: stored(small, 5),
            bundles: vec![7, 9],
            body: Body::Res(ResInfo {
                res_type: 0x49B1_56D4,
                rid: 1 << 60,
                meta: vec![1, 2, 3],
            }),
        },
        Entry {
            name: "766b543f-8028-4fea-92e9-0d9a5c256d3e".into(),
            stored: stored(large, 70_000),
            bundles: vec![],
            body: Body::Chunk(ChunkInfo {
                range_start: 1,
                range_end: 2,
                logical_offset: 3,
                logical_size: 70_000,
                h32: 5,
                first_mip: -1,
                added_bundles: vec![11, 12],
            }),
        },
    ];
    m
}

#[test]
fn every_kind_round_trips() -> R {
    let m = sample();
    let bytes = fbmod::write(&m)?;
    let back = fbmod::read(&bytes)?;
    assert_eq!(back, m);
    assert_eq!(fbmod::write(&back)?, bytes);
    let kinds: Vec<_> = back.entries.iter().map(Entry::kind).collect();
    assert_eq!(
        kinds,
        [
            EntryKind::Embedded,
            EntryKind::Ebx,
            EntryKind::Res,
            EntryKind::Chunk
        ]
    );
    assert_eq!(
        back.entries.iter().map(|e| e.kind().id()).collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
    Ok(())
}

/// A mod with one payload-less embedded entry, bytes worked by hand from the layout in the module's doc comment.
#[test]
fn layout_by_hand() -> R {
    let m = Fbmod {
        version: 6,
        profile: "S".into(),
        game_version: 1,
        title: "t".into(),
        author: String::new(),
        category: String::new(),
        mod_version: String::new(),
        description: String::new(),
        link: String::new(),
        entries: vec![Entry {
            name: "I".into(),
            stored: None,
            bundles: vec![],
            body: Body::Embedded,
        }],
        payloads: vec![vec![0xAB, 0xCD]],
    };
    let mut want = Vec::new();
    want.extend(b"FROSTY\0\x01");
    want.extend(6u32.to_le_bytes());
    let table_at = 8 + 4 + 8 + 4 + 1 + 1 + 4 + (2 + 5) + 4 + (1 + 4 + 2 + 4);
    want.extend((table_at as u64).to_le_bytes());
    want.extend(1u32.to_le_bytes()); // payload count
    want.extend([1, b'S']);
    want.extend(1u32.to_le_bytes());
    want.extend(b"t\0\0\0\0\0\0"); // title, then five empty strings
    want.extend(1u32.to_le_bytes()); // entries
    want.extend([0]);
    want.extend((-1i32).to_le_bytes());
    want.extend(b"I\0");
    want.extend(0u32.to_le_bytes()); // bundles
    assert_eq!(want.len(), table_at);
    want.extend(0u64.to_le_bytes());
    want.extend(2u64.to_le_bytes());
    want.extend([0xAB, 0xCD]);
    assert_eq!(fbmod::write(&m)?, want);
    assert_eq!(fbmod::read(&want)?, m);
    Ok(())
}

#[test]
fn payloads_decode_and_are_shared() -> R {
    let mut m = sample();
    assert_eq!(m.payload(0, None)?, b"hello");
    let big = m.payload(1, None)?;
    assert_eq!(big.len(), 70_000);
    assert_eq!(big[250], 250);
    assert_eq!(big[251], 0);
    // an identical payload is reused; an empty one is one empty block
    assert_eq!(m.add_payload(b"hello")?, 0);
    let empty = m.add_payload(b"")?;
    assert_eq!(empty, 2);
    assert!(m.payload(empty, None)?.is_empty());
    assert_eq!(m.add_payload(b"")?, empty);
    assert!(m.payload(3, None).is_err());
    Ok(())
}

#[test]
fn unused_payloads_are_dropped_and_indices_follow() -> R {
    let mut m = sample();
    // the EBX entry is given a new payload; the resource still shares the first
    let fresh = m.add_payload(b"fresh")?;
    m.entries[1].stored.as_mut().unwrap().index = fresh;
    assert_eq!(m.drop_unused_payloads(), 0);
    // now nothing points at "hello": it goes, the others move down by one
    m.entries[2].stored.as_mut().unwrap().index = fresh;
    assert_eq!(m.drop_unused_payloads(), 1);
    assert_eq!(m.payloads.len(), 2);
    assert_eq!(
        m.entries.iter().map(Entry::index).collect::<Vec<_>>(),
        [None, Some(1), Some(1), Some(0)]
    );
    assert_eq!(m.payload(1, None)?, b"fresh");
    assert_eq!(m.payload(0, None)?.len(), 70_000);
    assert_eq!(fbmod::read(&fbmod::write(&m)?)?, m);
    Ok(())
}

fn refused(bytes: &[u8], what: &str) {
    let e = fbmod::read(bytes).unwrap_err().to_string();
    assert!(e.contains(what), "{e:?} does not say {what:?}");
}

#[test]
fn refusals() -> R {
    let bytes = fbmod::write(&sample())?;

    let mut b = bytes.clone();
    b[0] = b'X';
    refused(&b, "not a FROSTY mod");

    let mut b = bytes.clone();
    b[8] = 5;
    refused(&b, "version 5");

    // a read past the end: every proper prefix is an error, none a panic
    for n in 0..bytes.len() {
        assert!(fbmod::read(&bytes[..n]).is_err(), "prefix of {n} bytes read");
    }

    refused(&[&bytes[..], &[0]].concat(), "1 bytes after the last payload");

    // the first entry's kind byte follows the six detail strings
    let kind_at = 8 + 4 + 8 + 4 + 1 + 5 + 4 + (2 + 2 + 1 + 4 + 1 + 1) + 4;
    assert_eq!(bytes[kind_at + 5..kind_at + 9], *b"Icon");
    let mut b = bytes.clone();
    b[kind_at] = 4;
    refused(&b, "unknown entry kind 4: Icon");

    // a payload that is not what its SHA-1 says
    let mut b = bytes.clone();
    *b.last_mut().unwrap() ^= 1;
    refused(&b, "SHA-1 of payload 1 differs");

    // a payload index past the table
    let mut b = bytes.clone();
    b[kind_at + 1..kind_at + 5].copy_from_slice(&9i32.to_le_bytes());
    refused(&b, "payload 9 of 2");

    // payloads that are not one after the other
    let table_at = u64::from_le_bytes(bytes[12..20].try_into()?) as usize;
    let mut b = bytes.clone();
    b[table_at + 16..table_at + 24].copy_from_slice(&1u64.to_le_bytes());
    refused(&b, "payload 1 at 0x1, expected");

    // a payload table that does not start where the entries end
    let mut b = bytes.clone();
    b[12..20].copy_from_slice(&(table_at as u64 + 1).to_le_bytes());
    refused(&b, "entries end here");

    // the writer refuses what the reader could not read back
    let mut m = sample();
    m.entries[1].stored = stored(9, 5);
    assert!(fbmod::write(&m).is_err());
    let mut m = sample();
    m.version = 7;
    assert!(fbmod::write(&m).is_err());
    let mut m = sample();
    m.title = "a\0b".into();
    assert!(fbmod::write(&m).is_err());
    let mut m = sample();
    m.profile = "p".repeat(256);
    assert!(fbmod::write(&m).is_err());
    Ok(())
}

/// A count that cannot fit the file is refused before anything is reserved.
#[test]
fn corrupt_counts_are_refused() -> R {
    let bytes = fbmod::write(&sample())?;
    let entries_at = 8 + 4 + 8 + 4 + 1 + 5 + 4 + (2 + 2 + 1 + 4 + 1 + 1);
    let mut b = bytes.clone();
    b[entries_at..entries_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    refused(&b, "entries of at least");
    let mut b = bytes;
    b[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(fbmod::read(&b).is_err());
    Ok(())
}

fn fbmod_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            fbmod_files(&p, out)?;
        } else if p.extension().is_some_and(|x| x == "fbmod") {
            out.push(p);
        }
    }
    Ok(())
}

/// Every file of `PRIMASE_RESKATE_FBMOD`: read, written again byte for byte, every payload decoded to the size its
/// entry states (Oodle blocks with `PRIMASE_OODLE9`). Prints the entry counts by kind and one line per entry.
#[test]
#[allow(unsafe_code)]
fn real_files_round_trip() -> R {
    let Some(dir) = primase_testdata::real_data("PRIMASE_RESKATE_FBMOD") else {
        eprintln!("SKIPPED: PRIMASE_RESKATE_FBMOD not set");
        return Ok(());
    };
    let oodle = match primase_testdata::real_data("PRIMASE_OODLE9") {
        // SAFETY: the gate configuration names the user's own trusted game DLL.
        Some(p) => {
            Some(unsafe { primase_compress::oodle::Oodle::load(p, primase_compress::oodle::Version::V9)? })
        }
        None => {
            eprintln!("PRIMASE_OODLE9 not set: files with Oodle payloads fail");
            None
        }
    };
    let mut files = Vec::new();
    fbmod_files(&dir, &mut files)?;
    files.sort();
    assert!(!files.is_empty(), "PRIMASE_RESKATE_FBMOD holds no .fbmod files");
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy();
        let bytes = std::fs::read(path)?;
        let m = fbmod::read(&bytes).map_err(|e| format!("{name}: {e}"))?;
        assert_eq!(fbmod::write(&m)?, bytes, "{name}: write differs from the file");
        let mut counts = std::collections::BTreeMap::new();
        for e in &m.entries {
            *counts.entry(e.kind().id()).or_insert(0usize) += 1;
        }
        eprintln!(
            "{name}: {} bytes, profile {:?}, game version {}, {} entries {counts:?} (kind: count), {} payloads",
            bytes.len(),
            m.profile,
            m.game_version,
            m.entries.len(),
            m.payloads.len()
        );
        for i in 0..m.payloads.len() {
            let decoded = m
                .payload(i, oodle.as_ref())
                .map_err(|e| format!("{name}: payload {i}: {e}"))?;
            for e in m.entries.iter().filter(|e| e.index() == Some(i)) {
                let size = e.stored.as_ref().map(|s| s.size);
                assert_eq!(
                    size,
                    Some(decoded.len() as u64),
                    "{name}: {} decodes to another size",
                    e.name
                );
            }
        }
        for e in &m.entries {
            let s = e.stored.as_ref();
            eprintln!(
                "LIST {name} {} {} {:?} {:?}",
                e.kind().id(),
                e.name,
                s.map(|s| s.size),
                s.map(|s| m.payloads[s.index].len())
            );
        }
    }
    Ok(())
}
