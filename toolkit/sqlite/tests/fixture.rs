//! A database written by Python's sqlite3 (`oracles/make_fixture.py`) and read back by Python (`oracles/dump.py`):
//! 512-byte pages, two b-tree levels, overflow chains, every integer width, reals, NULLs, a rowid alias.
mod common;
use primase_sqlite::{Database, Error, Value};

const DB: &[u8] = include_bytes!("fixtures/multi.db");
const TSV: &str = include_str!("fixtures/multi.tsv");

#[test]
fn every_table_and_value_equals_pythons() {
    let (tables, rows) = common::compare(DB, TSV);
    assert_eq!((tables, rows), (4, 307));
}

#[test]
fn shape_of_the_fixture_is_what_the_test_claims() {
    // 66 pages of 512 bytes; the long text and blob must have gone to overflow pages, and 300 rows to an interior page
    assert_eq!(DB.len(), 66 * 512);
    let db = Database::open(DB).unwrap();
    let wide = db.table("WIDE").unwrap();
    assert_eq!(wide.column("data"), Some(4));
    assert!(
        wide.rows
            .iter()
            .any(|r| matches!(&r.values[1], Value::Text(t) if t.len() == 5000))
    );
    assert!(
        wide.rows
            .iter()
            .any(|r| matches!(&r.values[4], Value::Blob(b) if b.len() == 3000))
    );
    // the first table's root page is an interior page
    let root = db.tables()[0].root_page as usize;
    assert_eq!(DB[(root - 1) * 512], 0x05);
}

#[test]
fn damaged_files_are_refused_with_an_offset() {
    let db = Database::open(DB).unwrap();
    let root = db.tables()[0].root_page as usize;
    let at = (root - 1) * 512;
    let table = |bytes: &[u8]| Database::open(bytes).and_then(|d| d.table("wide"));
    let invalid = |r: Result<(), Error>| match r {
        Err(Error::Invalid { offset, detail }) => (offset, detail),
        Err(e) => panic!("{e}"),
        Ok(()) => panic!("accepted a damaged file"),
    };

    // a page type that is not a table page
    let mut bad = DB.to_vec();
    bad[at] = 0x0a;
    let (offset, _) = invalid(table(&bad).map(|_| ()));
    assert_eq!(offset, at);

    // the right-most child pointing back at the root: the page is reached twice
    let mut bad = DB.to_vec();
    bad[at + 8..at + 12].copy_from_slice(&(root as u32).to_be_bytes());
    let (_, detail) = invalid(table(&bad).map(|_| ()));
    assert!(detail.contains("twice"), "{detail}");

    // a child pointer past the end of the file
    let mut bad = DB.to_vec();
    bad[at + 8..at + 12].copy_from_slice(&999u32.to_be_bytes());
    let (_, detail) = invalid(table(&bad).map(|_| ()));
    assert!(detail.contains("outside the file"), "{detail}");

    // a truncated file
    let (offset, _) = invalid(Database::open(&DB[..DB.len() - 1]).map(|_| ()));
    assert_eq!(offset, DB.len() - 1);

    // text encoding UTF-16
    let mut bad = DB.to_vec();
    bad[56..60].copy_from_slice(&2u32.to_be_bytes());
    let (offset, _) = invalid(Database::open(&bad).map(|_| ()));
    assert_eq!(offset, 56);

    assert!(matches!(db.table("nope"), Err(Error::NoTable(_))));
}
