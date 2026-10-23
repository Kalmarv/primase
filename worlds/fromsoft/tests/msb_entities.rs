//! MSB part entity IDs and entity group IDs, exact as stored, against an independent reader (SoulsFormats' MSBS):
//! `entities.jsonl` in `oracles.scripts.sekiro/state` holds, per map file, every part's index, name, entity ID and
//! eight group IDs. Every map file it lists is read; a part count, name or value that differs fails the gate.
use primase_core::gate::{GateReport, Tolerance};
use primase_fromsoft::{Archive, dcx::Decoder, msb::MapLayout};
use primase_testdata::{Game, OracleSet, Tool, install, oracle, tool};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[allow(unsafe_code)]
mod native {
    pub fn load(path: std::path::PathBuf) -> primase_compress::Result<primase_compress::oodle::Oodle> {
        // SAFETY: caller explicitly supplies a trusted version-6 native library.
        unsafe { primase_compress::oodle::Oodle::load(path, primase_compress::oodle::Version::V6) }
    }
}
fn ints(v: &serde_json::Value) -> Result<Vec<i32>> {
    v.as_array()
        .ok_or("int array")?
        .iter()
        .map(|x| x.as_i64().and_then(|x| i32::try_from(x).ok()).ok_or("i32".into()))
        .collect()
}
#[test]
fn part_entities_match_an_independent_reader() -> Result<()> {
    let (Some(root), Some(oracle), Some(dll)) = (
        install(Game::Sekiro),
        oracle(OracleSet::Scripts, Game::Sekiro, "state"),
        tool(Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let dll = native::load(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 512 * 1024 * 1024,
    };
    let archive = Archive::open(root)?;
    let mut gate = GateReport::new("MSB part entity IDs and groups", Tolerance::Exact);
    let (mut maps, mut parts, mut with_entity, mut with_group) = (0, 0, 0, 0);
    for line in std::fs::read_to_string(oracle.join("entities.jsonl"))?.lines() {
        let want: serde_json::Value = serde_json::from_str(line)?;
        let path = want["path"].as_str().ok_or("path")?;
        let map = MapLayout::read(&archive.read(path, &decoder)?)?;
        let ps = want["parts"].as_array().ok_or("parts")?;
        if map.parts.len() != ps.len() {
            gate.fail(path, format!("{} parts, oracle {}", map.parts.len(), ps.len()));
            continue;
        }
        for (part, w) in map.parts.iter().zip(ps) {
            let item = format!("{path}/{}", part.index);
            if part.name != w["name"].as_str().ok_or("name")? {
                gate.fail(&item, format!("name {} vs {}", part.name, w["name"]));
                continue;
            }
            let id = w["entity_id"]
                .as_i64()
                .and_then(|x| i32::try_from(x).ok())
                .ok_or("entity_id")?;
            let mut a = vec![part.entity_id];
            a.extend(part.entity_group_ids);
            let mut b = vec![id];
            b.extend(ints(&w["entity_group_ids"])?);
            let bytes = |v: &[i32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
            gate.compare_bytes(item, &bytes(&a), &bytes(&b));
            parts += 1;
            with_entity += usize::from(part.entity_id > 0);
            with_group += usize::from(part.entity_group_ids.iter().any(|&g| g > 0));
        }
        maps += 1;
    }
    println!("{maps} maps, {parts} parts: {with_entity} with an entity ID, {with_group} in an entity group");
    assert!(maps > 0, "the oracle lists no map");
    gate.assert_passed();
    Ok(())
}
