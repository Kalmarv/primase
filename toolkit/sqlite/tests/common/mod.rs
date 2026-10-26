//! Compares the reader's tables with an oracle dump (`oracles/dump.py`: Python's sqlite3, values as typed hex).
use primase_sqlite::{Database, Table, Value};

fn hex(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2), "odd hex {s:?}");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex digit"))
        .collect()
}

fn value(s: &str) -> Value {
    match s.split_once(':') {
        None => {
            assert_eq!(s, "n");
            Value::Null
        }
        Some(("i", d)) => Value::Int(d.parse().expect("integer")),
        Some(("r", h)) => Value::Real(f64::from_be_bytes(hex(h).try_into().expect("eight bytes"))),
        Some(("t", h)) => Value::Text(String::from_utf8(hex(h)).expect("oracle text is UTF-8")),
        Some(("b", h)) => Value::Blob(hex(h)),
        Some((k, _)) => panic!("value kind {k:?}"),
    }
}

type OracleTable = (String, Vec<String>, Vec<(i64, Vec<Value>)>);

/// The oracle's tables: `(name, columns, [(rowid, values)])`.
pub fn parse(tsv: &str) -> Vec<OracleTable> {
    let mut out: Vec<OracleTable> = Vec::new();
    for line in tsv.lines() {
        let mut f = line.split('\t');
        let head = f.next().expect("a field");
        if let Some(name) = head.strip_prefix('@') {
            out.push((name.into(), f.map(String::from).collect(), Vec::new()));
        } else {
            let t = out.last_mut().expect("a table header first");
            t.2.push((head.parse().expect("rowid"), f.map(value).collect()));
        }
    }
    out
}

/// Every table, column name, rowid and value equal (floats by bit pattern); returns (tables, rows).
pub fn compare(data: &[u8], oracle: &str) -> (usize, usize) {
    let db = Database::open(data).expect("database opens");
    let want = parse(oracle);
    let mut names = vec!["sqlite_master".to_owned()];
    names.extend(db.tables().iter().map(|t| t.name.clone()));
    assert_eq!(names, want.iter().map(|t| t.0.clone()).collect::<Vec<_>>(), "table list");
    let mut rows = 0;
    for (name, columns, rows_want) in &want {
        let got: Table = db.table(name).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(&got.columns, columns, "{name}: columns");
        assert_eq!(got.rows.len(), rows_want.len(), "{name}: row count");
        for (g, (rowid, values)) in got.rows.iter().zip(rows_want) {
            assert_eq!(g.rowid, *rowid, "{name}: rowid");
            assert_eq!(g.values.len(), values.len(), "{name} row {rowid}: width");
            for (i, (a, b)) in g.values.iter().zip(values).enumerate() {
                let same = match (a, b) {
                    (Value::Real(x), Value::Real(y)) => x.to_bits() == y.to_bits(),
                    _ => a == b,
                };
                assert!(same, "{name} row {rowid} column {}: {a:?} != {b:?}", columns[i]);
            }
        }
        rows += rows_want.len();
    }
    (want.len(), rows)
}
