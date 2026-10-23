//! RPF7 reading over archives written by this test (no cipher, so no key material is needed): the directory tree,
//! stored and DEFLATE binaries, a resource laid out by its page flags, a nested archive, and the strict checks.
//! Oracle: the layout written here, and the inputs it was written from.
use std::io::Write;

use primase_compress::{Compressor, Deflate};
use primase_gta::rage::resource::PageFlags;
use primase_gta::read::archive::keys::Keys;
use primase_gta::read::archive::rpf::{Archive, EntryKind};

enum Item {
    Dir(&'static str, Vec<Item>),
    Stored(&'static str, Vec<u8>),
    Deflated(&'static str, Vec<u8>),
    Resource(&'static str, PageFlags, Vec<u8>),
    Archive(&'static str, Vec<u8>),
}

/// Writes an unencrypted RPF7 holding `root`'s items (breadth first, as the game's archives are laid out).
fn write_rpf(root: Vec<Item>) -> Vec<u8> {
    struct Raw {
        name: String,
        kind: u8,
        children: Vec<usize>,
        data: Vec<u8>,
        size: u32,
        flags: (u32, u32),
    }
    let mut raws = vec![Raw {
        name: String::new(),
        kind: 0,
        children: vec![],
        data: vec![],
        size: 0,
        flags: (0, 0),
    }];
    let mut queue = vec![(0usize, root)];
    while let Some((dir, items)) = queue.pop() {
        for it in items {
            let i = raws.len();
            raws[dir].children.push(i);
            match it {
                Item::Dir(n, sub) => {
                    raws.push(Raw {
                        name: n.into(),
                        kind: 0,
                        children: vec![],
                        data: vec![],
                        size: 0,
                        flags: (0, 0),
                    });
                    queue.push((i, sub));
                }
                Item::Stored(n, d) => {
                    let size = d.len() as u32;
                    raws.push(Raw {
                        name: n.into(),
                        kind: 1,
                        children: vec![],
                        data: d,
                        size,
                        flags: (0, 0),
                    });
                }
                Item::Deflated(n, d) => {
                    let size = d.len() as u32;
                    let c = Deflate.compress(&d).expect("deflate");
                    raws.push(Raw {
                        name: n.into(),
                        kind: 2,
                        children: vec![],
                        data: c,
                        size,
                        flags: (0, 0),
                    });
                }
                Item::Resource(n, sys, d) => {
                    let mut stored = vec![0u8; 16];
                    stored.extend(Deflate.compress(&d).expect("deflate"));
                    raws.push(Raw {
                        name: n.into(),
                        kind: 3,
                        children: vec![],
                        data: stored,
                        size: 0,
                        flags: (sys.0, 0),
                    });
                }
                Item::Archive(n, d) => {
                    let size = d.len() as u32;
                    raws.push(Raw {
                        name: n.into(),
                        kind: 1,
                        children: vec![],
                        data: d,
                        size,
                        flags: (0, 0),
                    });
                }
            }
        }
    }
    // children must be consecutive: renumber breadth first
    let mut order = vec![0usize];
    let mut k = 0;
    while k < order.len() {
        let c = raws[order[k]].children.clone();
        order.extend(c);
        k += 1;
    }
    let mut index = vec![0usize; raws.len()];
    for (new, &old) in order.iter().enumerate() {
        index[old] = new;
    }
    let mut names = vec![0u8]; // the root's empty name
    let mut name_off = vec![0u32; raws.len()];
    for &old in &order[1..] {
        name_off[old] = names.len() as u32;
        names.extend(raws[old].name.as_bytes());
        names.push(0);
    }
    while names.len() % 16 != 0 {
        names.push(0);
    }
    let toc = 16 + 16 * raws.len() + names.len();
    let mut block = toc.div_ceil(512) as u32;
    let mut entries = Vec::new();
    let mut data = Vec::new();
    for &old in &order {
        let r = &raws[old];
        let mut e = [0u8; 16];
        if r.kind == 0 {
            let first = r.children.first().map_or(0, |&c| index[c]) as u32;
            e[0..4].copy_from_slice(&name_off[old].to_le_bytes());
            e[4..8].copy_from_slice(&0x7FFF_FF00u32.to_le_bytes());
            e[8..12].copy_from_slice(&first.to_le_bytes());
            e[12..16].copy_from_slice(&(r.children.len() as u32).to_le_bytes());
        } else {
            let stored = if r.kind == 1 { 0 } else { r.data.len() as u32 };
            e[0..2].copy_from_slice(&(name_off[old] as u16).to_le_bytes());
            e[2..5].copy_from_slice(&stored.to_le_bytes()[..3]);
            let off = if r.kind == 3 { block | 0x80_0000 } else { block };
            e[5..8].copy_from_slice(&off.to_le_bytes()[..3]);
            if r.kind == 3 {
                e[8..12].copy_from_slice(&r.flags.0.to_le_bytes());
                e[12..16].copy_from_slice(&r.flags.1.to_le_bytes());
            } else {
                e[8..12].copy_from_slice(&r.size.to_le_bytes());
            }
            let at = block as usize * 512 - toc.div_ceil(512) * 512;
            if data.len() < at {
                data.resize(at, 0);
            }
            data.extend(&r.data);
            data.resize(data.len().div_ceil(512) * 512, 0);
            block = (toc.div_ceil(512) * 512 + data.len()) as u32 / 512;
        }
        entries.extend(e);
    }
    let mut out = Vec::new();
    out.extend(0x5250_4637u32.to_le_bytes());
    out.extend((raws.len() as u32).to_le_bytes());
    out.extend((names.len() as u32).to_le_bytes());
    out.extend(0x4E45_504Fu32.to_le_bytes());
    out.extend(entries);
    out.extend(names);
    out.resize(toc.div_ceil(512) * 512, 0);
    out.extend(data);
    out
}

fn temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("primase-gta-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temporary folder");
    let p = dir.join(name);
    std::fs::File::create(&p)
        .and_then(|mut f| f.write_all(bytes))
        .expect("temporary archive");
    p
}

#[test]
fn reads_tree_binaries_resources_and_nested_archives() {
    // a resource of one 0x2000-byte system page (base shift 4, one page of the smallest size)
    let sys = PageFlags(4 | 1 << 27);
    let page: Vec<u8> = (0..0x2000u32).map(|i| (i * 7) as u8).collect();
    let inner = write_rpf(vec![Item::Stored("Inner.TXT", b"inside".to_vec())]);
    let text: Vec<u8> = b"the quick brown fox ".repeat(100);
    let bytes = write_rpf(vec![
        Item::Dir(
            "Data",
            vec![
                Item::Deflated("long.xml", text.clone()),
                Item::Resource("a.ydr", sys, page.clone()),
            ],
        ),
        Item::Stored("readme.txt", b"hello".to_vec()),
        Item::Archive("nested.rpf", inner),
    ]);
    let path = temp_file("test.rpf", &bytes);
    // no cipher: no key material is needed
    let keys = Keys::none();
    let a = Archive::open(&path, "Test.rpf", &keys).unwrap();
    let paths: Vec<_> = a.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "test.rpf",
            "test.rpf\\data",
            "test.rpf\\readme.txt",
            "test.rpf\\nested.rpf",
            "test.rpf\\data\\long.xml",
            "test.rpf\\data\\a.ydr"
        ]
    );
    let mut f = std::fs::File::open(&path).unwrap();
    assert_eq!(a.binary(&mut f, 2, &keys).unwrap(), b"hello");
    assert_eq!(a.binary(&mut f, 4, &keys).unwrap(), text);
    let r = a.resource(&mut f, 5, &keys).unwrap();
    assert!(matches!(a.entries[5].kind, EntryKind::Resource { .. }));
    assert_eq!(r.system_data(), &page[..]);
    assert!(r.graphics_data().is_empty());
    let n = a.nested(&mut f, 3, &keys).unwrap();
    assert_eq!(n.entries[1].path, "test.rpf\\nested.rpf\\inner.txt");
    assert_eq!(n.binary(&mut f, 1, &keys).unwrap(), b"inside");
}

#[test]
fn a_body_may_end_inside_its_last_page_only() {
    // system: one 0x2000 page; graphics: two 0x2000 pages (base shift 4, pages of the smallest size twice is not
    // expressible, so: one page of base << 1 and one of base << 0)
    let sys = PageFlags(4 | 1 << 27);
    let gfx = PageFlags(4 | 1 << 26 | 1 << 27);
    let full = (sys.size() + gfx.size()) as usize;
    let last = gfx.last_page_size() as usize;
    assert_eq!(last, 0x2000);
    let body = |n: usize| Deflate.compress(&vec![7u8; n]).expect("deflate");
    // ends 100 bytes into the last page: accepted, the rest zero-filled
    let r =
        primase_gta::rage::resource::inflate(&body(full - last + 100), sys, gfx).expect("short last page");
    assert_eq!(r.data.len(), full);
    assert_eq!(r.zero_filled as usize, last - 100);
    assert!(r.data[full - last + 100..].iter().all(|&b| b == 0));
    // ends before the last page: refused
    assert!(primase_gta::rage::resource::inflate(&body(full - last - 1), sys, gfx).is_err());
    // longer than the pages: refused
    assert!(primase_gta::rage::resource::inflate(&body(full + 1), sys, gfx).is_err());
}

/// The sizes in a file are its words: a body is not made to allocate what it cannot inflate to.
#[test]
fn a_size_the_stored_bytes_cannot_inflate_to_is_refused_before_it_is_allocated() {
    // a resource: pages of 512 MiB from a body of a few bytes
    let body = Deflate.compress(&[7u8; 100]).expect("deflate");
    let e = primase_gta::rage::resource::inflate(&body, PageFlags(12 | 1 << 4), PageFlags(0))
        .unwrap_err()
        .to_string();
    assert!(e.contains("inflates to at most"), "{e}");
    // a binary: an entry stating 4 GiB - 1 bytes over 100 stored bytes
    let keys = Keys::none();
    let mut bytes = write_rpf(vec![Item::Deflated("a.bin", vec![7; 5000])]);
    let at = 16 + 16 + 8;
    bytes[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    let path = temp_file("claim.rpf", &bytes);
    let a = Archive::open(&path, "claim.rpf", &keys).unwrap();
    let mut f = std::fs::File::open(&path).unwrap();
    let e = a.binary(&mut f, 1, &keys).unwrap_err().to_string();
    assert!(e.contains("which inflates to at most"), "{e}");
}

#[test]
fn rejects_broken_tables() {
    let keys = Keys::none();
    let mut bytes = write_rpf(vec![Item::Stored("a.txt", b"x".to_vec())]);
    // a child range past the end of the table
    bytes[16 + 12] = 9;
    let path = temp_file("broken.rpf", &bytes);
    assert!(Archive::open(&path, "broken.rpf", &keys).is_err());
    let mut bytes = write_rpf(vec![Item::Stored("a.txt", b"x".to_vec())]);
    bytes[0] = 0;
    let path = temp_file("magic.rpf", &bytes);
    assert!(Archive::open(&path, "magic.rpf", &keys).is_err());
}

#[test]
fn key_material_is_checked() {
    // wrong sizes and wrong contents are refused with a message, never used
    let e = Keys::from_parts(&[0; 10], &[], &[], None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("NG keys"), "{e}");
    let e = Keys::from_parts(&[0; 101 * 272], &[0; 17 * 16 * 1024], &[0; 256], None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("NG key 0 does not match"), "{e}");
}

/// The install's listing over archives written here: a nested archive is an archive of its own and a file of its
/// parent, directories are not files, and every file carries the sizes and extension its table of contents gives.
/// Oracle: the layout written here, and the inputs it was written from.
#[test]
fn the_listing_names_every_archive_and_file_with_its_storage() {
    use primase_gta::read::archive::archive_set::Install;
    use primase_gta::read::archive::listing::Storage;
    let sys = PageFlags(4 | 1 << 27);
    let inner = write_rpf(vec![Item::Stored("Inner.TXT", b"inside".to_vec())]);
    let text: Vec<u8> = b"the quick brown fox ".repeat(100);
    let bytes = write_rpf(vec![
        Item::Dir(
            "Data",
            vec![
                Item::Deflated("long.xml", text.clone()),
                Item::Resource("a.YDR", sys, vec![7; 0x2000]),
            ],
        ),
        Item::Stored("readme", b"hello".to_vec()),
        Item::Archive("nested.rpf", inner),
    ]);
    let dir = std::env::temp_dir().join(format!("primase-gta-listing-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temporary folder");
    std::fs::File::create(dir.join("Test.rpf"))
        .and_then(|mut f| f.write_all(&bytes))
        .expect("temporary archive");
    let install = Install::open(&dir, &Keys::none()).unwrap();

    let archives: Vec<_> = install
        .archive_listing()
        .map(|a| (a.path, a.files, a.archives))
        .collect();
    assert_eq!(archives, [("test.rpf", 4, 1), (r"test.rpf\nested.rpf", 1, 0)]);
    let files: Vec<_> = install
        .listing()
        .map(|f| (f.archive, f.path, f.name, f.extension, f.size()))
        .collect();
    assert_eq!(
        files,
        [
            ("test.rpf", r"test.rpf\readme", "readme", "", 5),
            (
                "test.rpf",
                r"test.rpf\nested.rpf",
                "nested.rpf",
                "rpf",
                files[1].4
            ),
            (
                "test.rpf",
                r"test.rpf\data\long.xml",
                "long.xml",
                "xml",
                text.len() as u64
            ),
            ("test.rpf", r"test.rpf\data\a.ydr", "a.YDR", "ydr", 0x2000),
            (
                r"test.rpf\nested.rpf",
                r"test.rpf\nested.rpf\inner.txt",
                "Inner.TXT",
                "txt",
                6
            ),
        ]
    );
    let listed: Vec<_> = install.listing().collect();
    assert!(listed[1].is_archive() && !listed[0].is_archive());
    assert_eq!(
        listed[3].storage,
        Storage::Resource {
            system: 0x2000,
            graphics: 0
        }
    );
    assert!(matches!(listed[2].storage, Storage::Binary { stored, size, .. } if stored > 0 && stored < size));
    // every listed file is the install's file at its path
    for f in &listed {
        assert_eq!(install.file(f.path), Some(f.file));
    }
}
