//! A shipped game database (Tony Hawk: SHRED's `MAIN.DB`, 142 tables) against Python's sqlite3 reading of it.
#[path = "database_oracle/mod.rs"]
mod common;
use primase_testdata::{Game, OracleSet, install_file, oracle};

#[test]
fn main_db_equals_pythons_dump() {
    let (Some(db), Some(tsv)) = (
        install_file(Game::Shred, "PS3_GAME/USRDIR/database/main.db"),
        oracle(OracleSet::Corpus, Game::Shred, "main-db.tsv"),
    ) else {
        return;
    };
    let data = std::fs::read(&db).unwrap();
    let text = std::fs::read_to_string(&tsv).unwrap();
    let (tables, rows) = common::compare(&data, &text);
    eprintln!("SHRED MAIN.DB: {tables} tables (with sqlite_master), {rows} rows equal Python's sqlite3");
    assert_eq!((tables, rows), (143, 9_129));
}
