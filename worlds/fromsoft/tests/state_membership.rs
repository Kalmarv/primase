//! Event-flag membership of a whole block against an independent implementation: `PRIMASE_FROMSOFT_STATE_SCENE` is a
//! scene of one whole block exported with its world states (`extensions.sekiro.world_state`: the rules, and every
//! state-switched placement and world-collision part with its per-flag membership). Every part it lists must get the
//! same membership from our rules, and every part either side decides among those it places must be decided by both.
use std::collections::BTreeMap;

use primase_core::gate::{GateReport, Tolerance, real_data};
use primase_fromsoft::{
    Archive, dcx::Decoder, emevd::Script, msb::MapLayout, msb::PartKind, sekiro::states::States,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[allow(unsafe_code)]
mod native {
    pub fn load(path: std::path::PathBuf) -> primase_compress::Result<primase_compress::oodle::Oodle> {
        // SAFETY: caller explicitly supplies a trusted version-6 native library.
        unsafe { primase_compress::oodle::Oodle::load(path, primase_compress::oodle::Version::V6) }
    }
}

/// Per flag a part's rules name: "off" (present only while off), "on", or "never".
fn membership(rules: &[&primase_fromsoft::sekiro::states::Rule]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let flags: std::collections::BTreeSet<i32> = rules
        .iter()
        .flat_map(|r| r.disabled_when.iter().map(|&(f, _)| f))
        .collect();
    for f in flags {
        let hidden_on = rules.iter().any(|r| r.disabled_when.contains(&(f, true)));
        let hidden_off = rules.iter().any(|r| r.disabled_when.contains(&(f, false)));
        let m = match (hidden_on, hidden_off) {
            (true, true) => "never",
            (true, false) => "off",
            _ => "on",
        };
        out.insert(f.to_string(), m.to_owned());
    }
    out
}

#[test]
fn block_membership_matches_an_independent_implementation() -> Result<()> {
    let (Some(root), Some(scene), Some(dll)) = (
        real_data("PRIMASE_SEKIRO_DIR"),
        real_data("PRIMASE_FROMSOFT_STATE_SCENE"),
        real_data("PRIMASE_OODLE6"),
    ) else {
        return Ok(());
    };
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(scene.join("scene.json"))?)?;
    let ws = &manifest["extensions"]["sekiro"]["world_state"];
    let listed = ws["parts"].as_array().ok_or("world_state.parts")?;
    let placed: std::collections::BTreeSet<&str> = manifest["placement_sources"]
        .as_array()
        .ok_or("placement_sources")?
        .iter()
        .filter_map(|s| s.as_str())
        .collect();
    let first = listed.first().ok_or("no state parts listed")?;
    let block = first["source"]
        .as_str()
        .ok_or("source")?
        .split(':')
        .next()
        .ok_or("block")?
        .to_owned();

    let dll = native::load(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 512 * 1024 * 1024,
    };
    let archive = Archive::open(root)?;
    let layout = MapLayout::read(&archive.read(&format!("/map/mapstudio/{block}.msb.dcx"), &decoder)?)?;
    let states = States::read(&Script::read(
        &archive.read(&format!("/event/{block}.emevd.dcx"), &decoder)?,
    )?);

    let mut gate = GateReport::new("world-state membership", Tolerance::Exact);
    let theirs: BTreeMap<&str, &serde_json::Value> = listed
        .iter()
        .filter_map(|p| Some((p["source"].as_str()?, p)))
        .collect();
    // identity-level accounting: which reference parts were compared, which left open
    let (mut compared, mut left_open) = (
        std::collections::BTreeSet::new(),
        std::collections::BTreeSet::new(),
    );
    let mut both_plain = 0;
    for part in &layout.parts {
        let source = format!("{block}:{}", part.name);
        let collision_world = part.kind == PartKind::Collision && matches!(part.hit_filter, Some(8 | 27));
        let in_scope = matches!(part.kind, PartKind::MapPiece | PartKind::Object)
            && placed.contains(source.as_str())
            || theirs.contains_key(source.as_str())
            || collision_world;
        if !in_scope {
            continue;
        }
        let ours = membership(&states.rules_for(part));
        // our policy: a part something uninterpreted also names keeps its default presence (counted, not compared)
        if !ours.is_empty() && states.left_open(part) {
            // our rules name it, so the reference must list it too
            if !theirs.contains_key(source.as_str()) {
                gate.fail(
                    &source,
                    format!("rules {ours:?} here, not listed by the reference"),
                );
            }
            left_open.insert(source);
            continue;
        }
        match theirs.get(source.as_str()) {
            Some(t) => {
                let want: BTreeMap<String, String> = serde_json::from_value(t["states"].clone())?;
                gate.compare_bytes(
                    &source,
                    format!("{ours:?}").as_bytes(),
                    format!("{want:?}").as_bytes(),
                );
                compared.insert(source);
            }
            None if ours.is_empty() => both_plain += 1,
            None => gate.fail(&source, format!("decided here {ours:?}, not by the reference")),
        }
    }
    println!(
        "{block}: {} parts listed by the reference, {} compared, {} left at their default here because something uninterpreted also names them {left_open:?}, {both_plain} placed parts decided by neither",
        listed.len(),
        compared.len(),
        left_open.len()
    );
    assert!(compared.is_disjoint(&left_open));
    let accounted: std::collections::BTreeSet<&str> =
        compared.iter().chain(&left_open).map(String::as_str).collect();
    let listed_keys: std::collections::BTreeSet<&str> = theirs.keys().copied().collect();
    assert_eq!(
        accounted, listed_keys,
        "every listed part, by identity, was compared or left open"
    );
    gate.assert_passed();
    Ok(())
}
