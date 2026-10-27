//! A supplied session graph is recorded in the world as data. Reading the rows is tested in `rme-formats`.
use primase_core::{Instance, convert::Reader, frame::SourceFrame};
use primase_shred::{
    components::{SessionGraph, Stored},
    read::convert::ShredReader,
};
use rme_formats::session;
const DB: &[u8] = include_bytes!("fixtures/sessions.db");

#[test]
fn supplied_graph_is_data_without_a_session_choice() {
    let graph = Stored(session::SessionGraph::from_main_db(DB, 7).expect("synthetic graph"));
    let mut reader = ShredReader::new(vec![], SourceFrame::WORLD).with_sessions(graph.clone());
    let (world, report) = reader.read().expect("empty geometry with graph");
    assert_eq!(world.resource::<SessionGraph>(), Some(&graph));
    assert_eq!(world.iter::<Instance>().count(), 0);
    assert!(report.iter().any(|(_, s)| s.contains("no session selected")));
    assert!(world.verify_components().is_empty());
    let (loaded, _) = reader.load_all(world).expect("graph survives load");
    assert_eq!(loaded.resource::<SessionGraph>(), Some(&graph));
}
