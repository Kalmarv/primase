//! Hand-written rows persisted by Python SQLite: source fields, exact hashes, no selected mode.
use rme_formats::session::{SessionGraph, SessionRow};
const DB: &[u8] = include_bytes!("fixtures/sessions.db");
fn row(rows: &[SessionRow], id: u32) -> &SessionRow {
    rows.iter().find(|r| r.id == id).expect("fixture row")
}

#[test]
fn rows_and_relationship_expressions_stay_authored() {
    let graph = SessionGraph::from_main_db(DB, 7).expect("synthetic MAIN.DB");
    assert_eq!(
        (graph.sessions.len(), graph.goals.len(), graph.tasks.len()),
        (3, 1, 7)
    );
    assert_eq!(graph.level.fields["SESSIONS"].as_deref(), Some("2&3"));
    assert_eq!(
        row(&graph.sessions, 2).fields["unknown"].as_deref(),
        Some("unknown value")
    );
    assert_eq!(row(&graph.sessions, 0).fields["unknown"], None);
    assert_eq!(row(&graph.sessions, 3).fields["unknown"].as_deref(), Some(""));
    assert_eq!(
        row(&graph.goals, 5).fields["TASKS_OPT"].as_deref(),
        Some("[10-12]")
    );
    assert_eq!(row(&graph.goals, 5).fields["SEQUENTIAL"].as_deref(), Some("1"));
    assert!(row(&graph.sessions, 0).identifiers.is_empty());
    assert!(row(&graph.sessions, 3).identifiers.is_empty());
    assert!(row(&graph.tasks, 14).identifiers.is_empty());
    let names = &row(&graph.sessions, 2).identifiers;
    assert_eq!(
        names.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
        ["Boundary", "Marker"]
    );
    // Case-sensitive sdbm, independently hand-evaluated for the short token 'AB'.
    assert_eq!(primase_foundations::hash::sdbm(b"AB"), 0x00411041);
    assert_eq!(
        row(&graph.tasks, 8).identifiers[0].hash,
        primase_foundations::hash::sdbm(b"Active_Icon")
    );
    assert_ne!(
        row(&graph.tasks, 8).identifiers[0].hash,
        primase_foundations::hash::sdbm(b"active_icon")
    );
    assert!(SessionGraph::from_main_db(DB, 8).is_err());
    assert!(SessionGraph::from_main_db(&DB[..30], 7).is_err());
    let duplicates: Vec<_> = graph.tasks.iter().filter(|r| r.id == 8).collect();
    assert_eq!(duplicates.len(), 2);
    assert_ne!(duplicates[0].rowid, duplicates[1].rowid);
    assert_eq!(
        duplicates[1].fields["unknown"].as_deref(),
        Some("preserve duplicate")
    );
}
