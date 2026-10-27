//! Every Kraken DCX envelope in a game's archives (`installs.sekiro`; `installs.elden` with `keys.elden`): the game's Oodle 6
//! library and oozextract, the decoder where there is no game library, decode each to the same bytes. A stream oozextract
//! refuses is a failure named by archive entry. The game's library is needed, so this runs where it exists (Windows).
//!
//! `[run] gate = 'smoke'` checks a seeded sample (one entry in sixteen), `'full'` every entry. The debug key `oodle.dump`
//! names a folder under the scratch root where the payload and the library's output of each failing entry are kept.

#![allow(clippy::unwrap_used)]
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "test I/O: real-data gates"
)]

mod inputs;

use inputs::{Depth, Game as Data, OracleSet, Tool, debug, depth, oracle, scratch, seed, tool};
use primase_foundations::compress::Decompressor;
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_foundations::compress::reference;
use primase_fromsoft::Game;
use primase_fromsoft::dcx::{Compression, Envelope};
use primase_testdata::gate::{GateReport, Tolerance};
mod common;

type BoxError = Box<dyn std::error::Error>;

/// The sample's stride in smoke mode.
const STRIDE: u64 = 16;

fn gate(game: Game, data: Data) -> std::result::Result<(), BoxError> {
    let Some(archive) = common::archive(game)? else {
        return Ok(());
    };
    let Some(path) = tool(Tool::Oodle6) else {
        return Ok(());
    };
    // SAFETY: explicitly configured trusted version-6 game library.
    #[allow(unsafe_code)]
    let dll = unsafe { Oodle::load(path, Version::V6)? };
    let names: std::collections::BTreeMap<u64, String> = oracle(OracleSet::Uxm, data, "dictionary.txt")
        .map(std::fs::read_to_string)
        .transpose()?
        .map(|text| {
            text.lines()
                .map(|l| (archive.hash(l.trim()), l.trim().to_owned()))
                .collect()
        })
        .unwrap_or_default();
    let dump = debug("oodle.dump").map(|name| scratch().join(name));
    let seed = seed();
    let smoke = depth() == Depth::Smoke;
    println!("depth {} seed {seed}", if smoke { "smoke" } else { "full" });
    let mut report = GateReport::new(
        format!("{game:?} KRAK DCX: DLL 6 vs oozextract"),
        Tolerance::Exact,
    );
    let mut streams = 0usize;
    for (&hash, entry) in &archive.entries {
        if smoke && hash.wrapping_add(seed).wrapping_mul(0x9E37_79B9_7F4A_7C15) % STRIDE != 0 {
            continue;
        }
        let stored = archive.read_stored(hash)?;
        let Ok(envelope) = Envelope::parse(&stored, usize::MAX) else {
            continue;
        };
        if envelope.compression != Compression::Kraken {
            continue;
        }
        streams += 1;
        let item = format!(
            "shard {} {hash:016x} {}",
            entry.shard,
            names.get(&hash).map_or("?", String::as_str)
        );
        let expected = dll.decompress(envelope.payload, envelope.decoded_size)?;
        match reference::decode(envelope.payload, envelope.decoded_size) {
            Ok(decoded) if decoded == expected => report.record(item, 0.),
            outcome => {
                if let Some(dir) = &dump {
                    std::fs::create_dir_all(dir)?;
                    std::fs::write(dir.join(format!("{hash:016x}.payload")), envelope.payload)?;
                    std::fs::write(dir.join(format!("{hash:016x}.decoded")), &expected)?;
                }
                match outcome {
                    Ok(_) => report.fail(item, "oozextract decodes other bytes"),
                    Err(e) => report.fail(item, e.to_string().lines().next().unwrap_or_default()),
                }
            }
        }
    }
    println!("{streams} Kraken streams checked\n{report}");
    assert!(streams > 0, "no Kraken stream was checked");
    report.assert_passed();
    Ok(())
}

#[test]
fn sekiro_archives() -> std::result::Result<(), BoxError> {
    gate(Game::Sekiro, Data::Sekiro)
}

#[test]
fn elden_ring_archives() -> std::result::Result<(), BoxError> {
    gate(Game::EldenRing, Data::Elden)
}
