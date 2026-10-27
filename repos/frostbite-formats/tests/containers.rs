//! Gates of the Frostbite containers: containers built by our writers read back to the same values and write again
//! to the same bytes, the TOC's perfect hashes resolve every key, and counts read from a file that cannot fit are
//! refused before anything is reserved. The real-data gates (a Studio Patch round trip, an independent reader's
//! summary, the base-game census) are in `worlds/reskate/tests/postcompile_fb.rs`.

#![allow(clippy::unwrap_used)]

use frostbite_formats::bundle::{self, Asset, Kind, Manifest};
use frostbite_formats::toc::{self, Loc, TocChunk};
use frostbite_formats::{MESHSET, db, depot, lookup};

type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn loc(chunk: u32, archive: u16) -> Loc {
    Loc {
        patch: true,
        chunk,
        archive,
    }
}

#[test]
fn synthetic_toc_round_trip() -> R {
    let names = [
        "win32/levels/game/a/a",
        "win32/levels/game/a/B_sublevel",
        "win32/levels/game/a/c",
        "win32/x",
    ];
    let bundles: Vec<(String, Vec<u8>)> = names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            (
                n.to_string(),
                bundle::write_region(
                    &[
                        (loc(1, 2), 16 * i as u32, 100),
                        (loc(1, 2), 200, 7),
                        (loc(3, 1), 0, 9),
                    ],
                    &[],
                ),
            )
        })
        .collect();
    let chunks: Vec<TocChunk> = (0u8..9)
        .map(|i| TocChunk {
            guid: [i.wrapping_mul(37); 16],
            file: (i != 4).then_some((loc(1, 3), u32::from(i) * 64, 64)),
        })
        .collect();
    let bytes = toc::write(&bundles, &chunks, 3)?;
    let t = toc::read(&bytes)?;
    toc::verify(&t)?;
    let mut got: Vec<_> = t.bundles.clone();
    got.sort();
    let mut want = bundles.clone();
    want.sort();
    assert_eq!(got, want);
    assert_eq!(t.chunks.len(), chunks.len());
    for c in &chunks {
        assert!(t.chunks.contains(c));
    }
    assert_eq!(toc::write(&t.bundles, &t.chunks, t.flags)?, bytes);
    for (_, region) in &t.bundles {
        let (files, inline) = bundle::read_region(region)?;
        assert_eq!(&bundle::write_region(&files, &inline), region);
    }
    Ok(())
}

#[test]
fn synthetic_manifest_round_trip() -> R {
    let mut ebx = Asset {
        kind: Kind::Ebx,
        name: "a/b".into(),
        sha1: [1; 20],
        size: 10,
        rtype: 0,
        rmeta: [0; 16],
        rid: 0,
        guid: [0; 16],
        loff: 0,
        lsize: 0,
    };
    let mut res = ebx.clone();
    res.kind = Kind::Res;
    res.name = "a/b_mesh".into();
    res.rtype = MESHSET;
    res.rmeta = [7; 16];
    res.rid = 99;
    let mut chunk = ebx.clone();
    chunk.kind = Kind::Chunk;
    chunk.name.clear();
    chunk.guid = [5; 16];
    chunk.lsize = 1234;
    chunk.size = 1234;
    ebx.size = 11;
    let m = Manifest {
        ebx: vec![ebx],
        res: vec![res],
        chunks: vec![chunk],
        meta: vec![1, 2, 3, 0],
        big_endian: false,
    };
    let bytes = bundle::write_manifest(&m)?;
    assert!(bundle::is_manifest(&bytes));
    let back = bundle::read_manifest(&bytes)?;
    assert_eq!(back, m);
    assert_eq!(bundle::write_manifest(&back)?, bytes);
    Ok(())
}

#[test]
fn synthetic_depot_round_trip() -> R {
    let block = |n: u32| {
        let mut b = n.to_le_bytes().to_vec();
        for i in 0..n {
            b.extend((0x1000u64 * u64::from(i + 1)).to_le_bytes());
            b.extend(0x2bb6_5c2du32.to_le_bytes());
            b.extend([1, 0, 0, 0, 1, 0, 0, 0]);
            b.extend(1.5f32.to_le_bytes());
        }
        b
    };
    let dep = depot::Depot {
        head: 0x10,
        version: 0x0119_0003,
        entries: vec![(3, 0), (1, 1), (2, 0)],
        blocks: vec![
            depot::Block {
                hash: 0xAA,
                data: block(2),
            },
            depot::Block {
                hash: 0xBB,
                data: block(1),
            },
        ],
    };
    let (bytes, meta) = depot::write(&dep);
    let back = depot::read(&bytes, &meta)?;
    assert_eq!(back, dep);
    assert_eq!(depot::write(&back), (bytes, meta));
    assert_eq!(depot::params(&dep.blocks[0].data)?.len(), 2);
    Ok(())
}

#[test]
fn synthetic_db_round_trip() -> R {
    let leaf = |name: &str, ty: u8, payload: Vec<u8>| db::Node {
        ty,
        named: true,
        name: name.into(),
        payload,
        ..db::Node::default()
    };
    let mut obj = db::Node {
        ty: 2,
        terminated: true,
        ..db::Node::default()
    };
    obj.children = vec![
        leaf("name", 7, b"win32/x\0".to_vec()),
        leaf("persistentIndex", 8, 3u32.to_le_bytes().to_vec()),
    ];
    let list = db::Node {
        ty: 1,
        named: true,
        name: "installChunks".into(),
        children: vec![obj],
        terminated: true,
        ..db::Node::default()
    };
    let root = db::Node {
        ty: 2,
        named: false,
        terminated: true,
        children: vec![list],
        ..db::Node::default()
    };
    let mut env = vec![0u8; toc::ENVELOPE];
    env[..4].copy_from_slice(&toc::MAGIC);
    let bytes = db::write_file(&env, &root, &[])?;
    let (e, back, trailer) = db::read_file(&bytes)?;
    assert_eq!((e, &back, trailer.len()), (env.clone(), &root, 0));
    assert_eq!(db::write_file(&env, &back, &[])?, bytes);
    Ok(())
}

#[test]
fn synthetic_program_lookup_round_trip() -> R {
    let mut d = vec![9u8; 24];
    d.extend(2u32.to_le_bytes());
    for (k, row) in [(1u64, vec![5u32, 6]), (7, vec![8])] {
        d.extend(k.to_le_bytes());
        d.extend((row.len() as u32).to_le_bytes());
        for p in row {
            d.extend(p.to_le_bytes());
        }
    }
    let mut meta = [0u8; 16];
    meta[0..4].copy_from_slice(&20u32.to_le_bytes());
    meta[4..8].copy_from_slice(&4u32.to_le_bytes());
    meta[8..12].copy_from_slice(&((d.len() - 24) as u32).to_le_bytes());
    let t = lookup::read_programs(&d, &meta)?;
    assert_eq!(t.rows[&1], vec![5, 6]);
    assert_eq!(lookup::write_programs(&t, &meta), (d, meta));
    Ok(())
}

/// Counts read from a file are checked against the bytes present before anything is reserved: a huge count in a
/// short buffer is an error, not an allocation.
fn refused(r: Result<(), frostbite_formats::Error>) {
    let e = r.expect_err("a corrupt count must be an error").to_string();
    assert!(
        e.contains("entries of at least"),
        "refused for another reason: {e}"
    );
}

#[test]
fn corrupt_counts_are_errors() {
    // bundle region: 0x7fffffff files declared in a 64-byte region
    let mut region = vec![0u8; 64];
    for (i, v) in [0u32, 0, 0x24, 0x7fff_ffff, 0x24, 0x24, 0x24, 0, 0x7fff_ffff]
        .iter()
        .enumerate()
    {
        region[4 * i..4 * i + 4].copy_from_slice(&v.to_be_bytes());
    }
    refused(bundle::read_region(&region).map(|_| ()));
    // manifest: total = ne = 0x7fffffff in a 40-byte manifest
    let mut m = vec![0u8; 40];
    m[0..4].copy_from_slice(&36u32.to_be_bytes());
    m[4..8].copy_from_slice(&(bundle::BUNDLE_MAGIC ^ bundle::BUNDLE_SALT).to_le_bytes());
    m[8..12].copy_from_slice(&0x7fff_ffffu32.to_le_bytes());
    m[12..16].copy_from_slice(&0x7fff_ffffu32.to_le_bytes());
    refused(bundle::read_manifest(&m).map(|_| ()));
    // depot: count 0x7fffffff in 32 bytes
    let mut meta = [0u8; 16];
    meta[0..4].copy_from_slice(&28u32.to_le_bytes());
    meta[4..8].copy_from_slice(&4u32.to_le_bytes());
    meta[12..16].copy_from_slice(&0x7fff_ffffu32.to_le_bytes());
    refused(depot::read(&[0u8; 32], &meta).map(|_| ()));
    // shader block: 0x7fffffff parameters in 8 bytes
    let mut block = 0x7fff_ffffu32.to_le_bytes().to_vec();
    block.extend([0; 4]);
    refused(depot::params(&block).map(|_| ()));
    // program lookup: 0x7fffffff rows in 8 bytes after the program records
    let d = [0xffu8, 0xff, 0xff, 0x7f, 0, 0, 0, 0];
    let mut meta = [0u8; 16];
    meta[8..12].copy_from_slice(&8u32.to_le_bytes());
    refused(lookup::read_programs(&d, &meta).map(|_| ()));
}
