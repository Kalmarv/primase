//! The objects of version-26 entries: a directory and objects laid out here by hand, their refusals, and every entry
//! of every forge of `installs.shaunwhite` read to its objects (the census prints the classes and trailers it meets).
use std::collections::BTreeMap;
use std::fs::File;

use anvil_formats::data::read_entry;
use anvil_formats::forge::Forge;
use anvil_formats::objects::{Error, read_objects};
use primase_testdata::{Game, install};

/// One object: class, name, body, trailer.
fn object(class: u32, name: &str, body: &[u8], trailer: &[u8]) -> Vec<u8> {
    let mut o = class.to_le_bytes().to_vec();
    o.extend_from_slice(&(body.len() as u32).to_le_bytes());
    o.extend_from_slice(&(name.len() as u32).to_le_bytes());
    o.extend_from_slice(name.as_bytes());
    o.push(0);
    o.extend_from_slice(body);
    o.extend_from_slice(trailer);
    o
}

fn directory(rows: &[(u32, usize)]) -> Vec<u8> {
    let mut d = (rows.len() as u16).to_le_bytes().to_vec();
    for (id, size) in rows {
        d.extend_from_slice(&id.to_le_bytes());
        d.extend_from_slice(&(*size as u32).to_le_bytes());
    }
    d
}

#[test]
fn objects_read_as_laid_and_refuse_what_does_not_fit() {
    let a = object(0x415D_9568, "Mesh_A", b"body", b"");
    let b = object(0xAC2B_BF68, "Cell", b"xy", b"\x01\x02");
    let res = [a.clone(), b.clone()].concat();
    let dir = directory(&[(7, a.len()), (9, b.len())]);
    let objs = read_objects(&dir, &res).unwrap();
    assert_eq!(objs.len(), 2);
    assert_eq!(
        (objs[0].id, objs[0].class, objs[0].name.as_str()),
        (7, 0x415D_9568, "Mesh_A")
    );
    assert_eq!(&res[objs[0].body.clone()], b"body");
    assert_eq!(
        (&res[objs[1].body.clone()], &res[objs[1].trailer.clone()]),
        (&b"xy"[..], &b"\x01\x02"[..])
    );
    // a directory longer than its rows, objects short of the block, a size past it, a body past its object
    assert!(matches!(
        read_objects(&[dir.clone(), vec![0]].concat(), &res),
        Err(Error::Directory { .. })
    ));
    assert!(matches!(
        read_objects(&dir, &[res.clone(), vec![0]].concat()),
        Err(Error::Trailing { .. })
    ));
    assert!(matches!(
        read_objects(&directory(&[(7, res.len() + 1)]), &res),
        Err(Error::Object { .. })
    ));
    assert!(matches!(
        read_objects(&directory(&[(7, a.len() - 1)]), &a[..a.len() - 1]),
        Err(Error::Object { .. })
    ));
}

/// Every entry of every forge (but each forge's GlobalMetaFile, which holds no blocks) reads to its objects.
#[test]
fn every_real_entry_reads_to_its_objects() {
    let Some(folder) = install(Game::ShaunWhite) else {
        return;
    };
    let mut forges: Vec<_> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "forge"))
        .collect();
    forges.sort();
    assert_eq!(forges.len(), 33);
    let (mut entries, mut objects) = (0usize, 0usize);
    let mut classes: BTreeMap<u32, usize> = BTreeMap::new();
    let mut trailers: BTreeMap<usize, usize> = BTreeMap::new();
    let mut leading: BTreeMap<String, usize> = BTreeMap::new();
    for path in &forges {
        let f = Forge::open(path).unwrap();
        let mut r = File::open(path).unwrap();
        for (k, n) in f.names.iter().enumerate() {
            if n.name == "GlobalMetaFile" {
                continue;
            }
            let e = read_entry(&f.payload(&mut r, k).unwrap(), 1 << 30).unwrap();
            let objs = read_objects(&e.metadata.data, &e.resource.data)
                .unwrap_or_else(|err| panic!("{}: {}: {err}", path.display(), n.name));
            entries += 1;
            for o in &objs {
                objects += 1;
                *classes.entry(o.class).or_default() += 1;
                *trailers.entry(o.trailer.len()).or_default() += 1;
                let body = &e.resource.data[o.body.clone()];
                let lead = if body.get(1..5) == Some(&o.id.to_le_bytes()[..]) {
                    format!("byte, id ({:#x})", body[0])
                } else if body.get(..4) == Some(&o.id.to_le_bytes()[..]) {
                    "id".to_string()
                } else {
                    "other".to_string()
                };
                *leading.entry(lead).or_default() += 1;
            }
        }
    }
    println!("{entries} entries, {objects} objects, {} classes", classes.len());
    println!("trailer sizes: {trailers:?}");
    println!("bodies start with: {leading:?}");
    let mut top: Vec<_> = classes.into_iter().collect();
    top.sort_by_key(|(c, n)| (std::cmp::Reverse(*n), *c));
    println!("classes: {:x?}", &top[..top.len().min(40)]);
    assert_eq!(entries, 3894 - 33);
}
