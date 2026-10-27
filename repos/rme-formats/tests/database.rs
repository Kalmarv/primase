//! A database written by Python's sqlite3 (`oracles/make_fixture.py`) and read back by Python (`oracles/dump.py`):
//! 512-byte pages, two b-tree levels, overflow chains, every integer width, reals, NULLs, a rowid alias.
#[path = "database_oracle/mod.rs"]
mod common;
use rme_formats::database::{Database, Error, Value};

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

/// The refusal's text, or a panic when the damaged file was accepted.
fn refusal(bytes: &[u8], table: &str) -> String {
    match Database::open(bytes).and_then(|d| d.table(table)) {
        Err(Error::Invalid(detail)) => detail,
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("accepted a damaged file"),
    }
}

#[test]
fn damaged_files_are_refused_naming_the_page() {
    let db = Database::open(DB).unwrap();
    let root = db.tables()[0].root_page as usize;
    let at = (root - 1) * 512;

    // a page type that is not a table page
    let mut bad = DB.to_vec();
    bad[at] = 0x0a;
    let detail = refusal(&bad, "wide");
    assert!(detail.contains(&format!("page {root}")), "{detail}");

    // the right-most child pointing back at the root: the page is reached twice
    let mut bad = DB.to_vec();
    bad[at + 8..at + 12].copy_from_slice(&(root as u32).to_be_bytes());
    let detail = refusal(&bad, "wide");
    assert!(detail.contains(&format!("page {root}")), "{detail}");

    // a child pointer past the end of the file
    let mut bad = DB.to_vec();
    bad[at + 8..at + 12].copy_from_slice(&999u32.to_be_bytes());
    let detail = refusal(&bad, "wide");
    assert!(detail.contains("999"), "{detail}");

    // a truncated file
    assert!(matches!(
        Database::open(&DB[..DB.len() - 1]),
        Err(Error::Invalid(_))
    ));

    // a write-ahead-log database: the main file alone may lack committed data
    let mut bad = DB.to_vec();
    bad[18] = 2;
    bad[19] = 2;
    let detail = refusal(&bad, "wide");
    assert!(detail.to_lowercase().contains("wal"), "{detail}");

    assert!(matches!(db.table("nope"), Err(Error::NoTable(_))));
}

/// Never a panic: bytes of the fixture changed at the header, at page starts and anywhere, the file cut short. Each
/// mutant is read whole, table by table, and ends in a table or an `Err`. (A panic fails the test.)
#[test]
fn mutated_files_never_panic() {
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    };
    let (mut refused, mut read) = (0, 0);
    for _ in 0..4000 {
        let mut bad = DB.to_vec();
        for _ in 0..=next(3) {
            let at = match next(3) {
                0 => next(100),
                1 => next(DB.len() / 512) * 512 + next(16),
                _ => next(DB.len()),
            };
            bad[at] = match next(3) {
                0 => 0x00,
                1 => 0xff,
                _ => u8::try_from(next(256)).unwrap(),
            };
        }
        if next(20) == 0 {
            bad.truncate(next(DB.len()));
        }
        let outcome = Database::open(&bad).and_then(|db| {
            let names: Vec<String> = db.tables().iter().map(|t| t.name.clone()).collect();
            names.iter().try_for_each(|n| db.table(n).map(drop))?;
            db.table("sqlite_master").map(drop)
        });
        match outcome {
            Ok(()) => read += 1,
            Err(_) => refused += 1,
        }
    }
    assert!(
        refused > 500 && read > 0,
        "refused {refused}, read {read}: the mutants miss the structure"
    );
}
