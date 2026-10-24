//! Event scripts over the installed game. Census: every script the archive holds (the common scripts and every map
//! block m10..m99 / 00..09, probed by path) reads to its end. Oracle: an independent reader's decode
//! (`emevd.jsonl` in `oracles.scripts.sekiro/state`: per script the decompressed file's sha256 and every event's id,
//! rest behaviour, instructions as bank, id, argument bytes and layer, parameter substitutions, linked files and
//! strings) must equal ours exactly, and it must list the same scripts as the census found.
use std::collections::BTreeMap;

use primase_core::gate::{GateReport, Tolerance};
use primase_fromsoft::{Archive, dcx::Decoder, emevd::Script};
use primase_testdata::{Game, OracleSet, Tool, install, oodle, oracle};
use sha2::{Digest, Sha256};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[allow(unsafe_code)]
mod native {
    pub fn load(
        path: primase_testdata::OodleLibrary,
    ) -> primase_compress::Result<primase_compress::oodle::Oodle> {
        // SAFETY: caller explicitly supplies a trusted version-6 native library.
        unsafe { primase_compress::oodle::Oodle::open(path.path(), primase_compress::oodle::Version::V6) }
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Every event-script path the archive holds.
fn scripts(archive: &Archive) -> Vec<String> {
    let mut names = vec!["common".to_owned(), "common_func".to_owned()];
    for a in 10..100 {
        for b in 0..10 {
            names.push(format!("m{a:02}_{b:02}_00_00"));
        }
    }
    names
        .into_iter()
        .map(|n| format!("/event/{n}.emevd.dcx"))
        .filter(|p| archive.contains(p))
        .collect()
}

#[test]
fn every_script_reads_and_matches_an_independent_reader() -> Result<()> {
    let (Some(root), Some(dll)) = (install(Game::Sekiro), oodle(Tool::Oodle6)) else {
        return Ok(());
    };
    let dll = native::load(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 512 * 1024 * 1024,
    };
    let archive = Archive::open(root)?;
    let paths = scripts(&archive);
    let mut read = BTreeMap::new();
    let (mut events, mut instructions, mut parameters) = (0, 0, 0);
    let mut calls: BTreeMap<i32, usize> = BTreeMap::new();
    for p in &paths {
        let data = archive.read(p, &decoder)?;
        let s = Script::read(&data).map_err(|e| format!("{p}: {e}"))?;
        for e in &s.events {
            events += 1;
            parameters += e.parameters.len();
            for i in &e.instructions {
                instructions += 1;
                if (i.bank, i.id) == (2000, 6)
                    && let Some(a) = i.args_i32()
                    && !a.is_empty()
                {
                    // InitializeCommonEvent: the function's event ID first, then its arguments
                    *calls.entry(a[0]).or_default() += 1;
                }
            }
        }
        read.insert(p.clone(), (data, s));
    }
    println!(
        "census: {} scripts, {events} events, {instructions} instructions, {parameters} parameters",
        paths.len()
    );
    let state_calls: Vec<_> = [20004110, 20004112, 20004114]
        .iter()
        .map(|f| (*f, calls.get(f).copied().unwrap_or(0)))
        .collect();
    println!("part-state common functions called: {state_calls:?}");
    assert!(!paths.is_empty(), "no event script in the archive");

    let Some(oracle) = oracle(OracleSet::Scripts, Game::Sekiro, "state") else {
        return Ok(());
    };
    let mut gate = GateReport::new("EMEVD decode", Tolerance::Exact);
    let mut listed = Vec::new();
    for line in std::fs::read_to_string(oracle.join("emevd.jsonl"))?.lines() {
        let w: serde_json::Value = serde_json::from_str(line)?;
        let path = w["path"].as_str().ok_or("path")?.to_owned();
        let Some((data, s)) = read.get(&path) else {
            gate.fail(&path, "listed by the oracle, not found by the census");
            continue;
        };
        listed.push(path.clone());
        gate.compare_bytes(
            format!("{path} sha256"),
            hex(&Sha256::digest(data)).as_bytes(),
            w["sha256"].as_str().ok_or("sha256")?.as_bytes(),
        );
        let we = w["events"].as_array().ok_or("events")?;
        if we.len() != s.events.len() {
            gate.fail(&path, format!("{} events, oracle {}", s.events.len(), we.len()));
            continue;
        }
        for (e, we) in s.events.iter().zip(we) {
            let item = format!("{path}/{}", e.id);
            let mut ours = format!("{} {} |", e.id, e.rest);
            for i in &e.instructions {
                ours += &format!(" {}:{} {} {:?}", i.bank, i.id, hex(&i.args), i.layer);
            }
            ours += " |";
            for p in &e.parameters {
                ours += &format!(
                    " {} {} {} {} {}",
                    p.instruction, p.target, p.source, p.bytes, p.unk
                );
            }
            let mut theirs = format!("{} {} |", we["id"], we["rest"]);
            for i in we["instructions"].as_array().ok_or("instructions")? {
                let layer = i["layer"].as_u64().map(|l| l as u32);
                theirs += &format!(
                    " {}:{} {} {:?}",
                    i["bank"],
                    i["id"],
                    i["args"].as_str().ok_or("args")?,
                    layer
                );
            }
            theirs += " |";
            for p in we["parameters"].as_array().ok_or("parameters")? {
                theirs += &format!(
                    " {} {} {} {} {}",
                    p["instruction"], p["target"], p["source"], p["bytes"], p["unk"]
                );
            }
            gate.compare_bytes(item, ours.as_bytes(), theirs.as_bytes());
        }
        let linked: Vec<i64> = w["linked_files"]
            .as_array()
            .ok_or("linked")?
            .iter()
            .filter_map(|x| x.as_i64())
            .collect();
        gate.compare_bytes(
            format!("{path} linked"),
            format!("{:?}", s.linked_files).as_bytes(),
            format!("{linked:?}").as_bytes(),
        );
        gate.compare_bytes(
            format!("{path} strings"),
            hex(&s.strings).as_bytes(),
            w["strings"].as_str().ok_or("strings")?.as_bytes(),
        );
    }
    listed.sort();
    let mut found = paths.clone();
    found.sort();
    if listed != found {
        gate.fail(
            "script set",
            format!("census found {}, oracle listed {}", found.len(), listed.len()),
        );
    }
    gate.assert_passed();
    Ok(())
}
