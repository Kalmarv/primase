//! Sekiro's LZB16 streams (Oodle decoder type 2) against the output the game's Oodle library gave for them
//! (`corpus.sekiro/lznib`): oozextract, the decoder where there is no game library, must give the same bytes. The oracle
//! holds what the library produced, so this runs without it.

#![allow(clippy::unwrap_used)]
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "test I/O: real-data gates"
)]

use primase_core::gate::{GateReport, Tolerance};
use primase_foundations::compress::reference;
use primase_testdata::{Game, OracleSet, Query, expect_found, oracle};

#[test]
fn oozextract_decodes_the_lznib_streams_as_the_game_library_does()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let Some(root) = oracle(OracleSet::Corpus, Game::Sekiro, "lznib") else {
        return Ok(());
    };
    let mut payloads: Vec<_> = std::fs::read_dir(&root)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::result::Result<_, _>>()?;
    payloads.retain(|path| path.extension().is_some_and(|e| e == "payload"));
    payloads.sort();
    expect_found(
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "lznib"),
        "LZB16 streams",
        payloads.len(),
    );
    let mut report = GateReport::new("LZB16 oozextract vs the game library", Tolerance::Exact);
    for payload in payloads {
        let name = payload.file_name().unwrap().to_string_lossy().into_owned();
        let stream = std::fs::read(&payload)?;
        let expected = std::fs::read(payload.with_extension("decoded"))?;
        match reference::decode(&stream, expected.len()) {
            Ok(decoded) => report.compare_bytes(name, &decoded, &expected),
            Err(e) => report.fail(name, e.to_string().lines().next().unwrap_or_default()),
        }
    }
    println!("{report}");
    report.assert_passed();
    Ok(())
}
