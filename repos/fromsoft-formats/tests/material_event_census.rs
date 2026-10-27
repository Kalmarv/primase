//! Every discovered Elden Ring character TAE's authored event 198, without interpreting its unknown ramp fields.
//! Main character names are exhaustively probed (c0000..c9999); companion binders come from the dictionary and
//! authored marker members. No HKX, material texture or shader is decoded. One binder is held at a time.
mod inputs;

use fromsoft_formats::{Binder, Game, Install, Keys, NameDictionary, dcx::Decoder, tae::Tae};
use inputs::{Game as Data, OracleSet, Tool, install, keys, oodle, oracle};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
mod common;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn word(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or("event word past payload")?
            .try_into()?,
    ))
}
fn units(value: u32) -> Value {
    let bytes = value.to_le_bytes();
    let float = f32::from_bits(value);
    json!({"u32":value,"hex":format!("{value:08x}"),"s32":value as i32,
        "f32":float.is_finite().then_some(float),"f32_finite":float.is_finite(),
        "i16":[i16::from_le_bytes([bytes[0],bytes[1]]),i16::from_le_bytes([bytes[2],bytes[3]])]})
}

#[test]
fn event_word_views_preserve_integer_float_and_halfword_identity() -> Result<()> {
    let bits = 0.5f32.to_bits();
    assert_eq!(word(&bits.to_le_bytes(), 0)?, bits);
    let decoded = units(bits);
    assert_eq!(decoded["f32"], json!(0.5));
    assert_eq!(decoded["i16"], json!([0, 16128]));
    assert_eq!(units(u32::MAX)["i16"], json!([-1, -1]));
    assert_eq!(units(u32::MAX)["f32_finite"], false);
    assert!(word(&[0; 3], 0).is_err());
    Ok(())
}

#[test]
fn every_elden_character_material_event198_payload() -> Result<()> {
    let (Some(root), Some(key_dir), Some(dll)) =
        (install(Data::Elden), keys(Data::Elden), oodle(Tool::Oodle6))
    else {
        println!("TAE198_SKIP=PRIMASE_DATA Elden install/keys/oodle6 not configured");
        return Ok(());
    };
    let codec = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    let install = Install::new(
        root,
        Game::EldenRing,
        Keys::from_folder(key_dir, Game::EldenRing, None)?,
    );
    let archive = install.open()?;
    let table =
        fromsoft_formats::game_params::read_param(&install, &archive, &decoder, "MaterialExParam.param")?;
    if table.param_type != "MATERIAL_EX_PARAM_ST" || table.data_version != 4 || table.row_size != 96 {
        return Err("MaterialEx layout differs from the verified ER v4 / 96-byte layout".into());
    }
    let mut material_rows = BTreeMap::new();
    for row in &table.rows {
        let name: Vec<_> = row.data[..64]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .take_while(|unit| *unit != 0)
            .collect();
        let values = (0..5)
            .map(|i| word(&row.data, 68 + i * 4).map(units))
            .collect::<Result<Vec<_>>>()?;
        material_rows.insert(
            row.id,
            json!({"id":row.id,"parameter":String::from_utf16(&name)?,
            "material_id":word(&row.data,64)? as i32,"rgba_i":values}),
        );
    }
    let mut todo = BTreeSet::new();
    // The archive hash table answers existence without opening a binder. This covers every main character,
    // including ones the UXM dictionary or character-model list does not name.
    for number in 0..10000 {
        let path = format!("/chr/c{number:04}.anibnd.dcx");
        if archive.contains(&path) {
            todo.insert(path);
        }
    }
    let main_binders = todo.len();
    let mut dictionary_binders = 0usize;
    if let Some(path) = oracle(OracleSet::Uxm, Data::Elden, "dictionary.txt") {
        let dictionary = NameDictionary::read_for(path, Game::EldenRing)?;
        for name in dictionary
            .names
            .values()
            .flatten()
            .filter(|name| name.starts_with("/chr/") && name.ends_with(".anibnd.dcx"))
        {
            if archive.contains(name) && todo.insert(name.clone()) {
                dictionary_binders += 1;
            }
        }
    } else {
        println!(
            "TAE198_COVERAGE_NOTE=no UXM dictionary: exhaustive main names and their authored companions only"
        );
    }
    let mut visited = BTreeSet::new();
    let mut histogram: Vec<BTreeMap<u32, usize>> = vec![BTreeMap::new(); 16];
    let mut by_row: BTreeMap<i32, Vec<BTreeMap<u32, usize>>> = BTreeMap::new();
    let mut lengths = BTreeMap::new();
    let mut characters: BTreeMap<String, Value> = BTreeMap::new();
    let mut tae_files = 0usize;
    let mut events = 0usize;
    let mut imports_with_events = 0usize;
    let mut malformed = 0usize;
    let mut marker_binders = 0usize;
    let mut import_refusals = 0usize;
    while let Some(path) = todo.pop_first() {
        if !visited.insert(path.clone()) {
            continue;
        }
        let stem = path
            .strip_prefix("/chr/")
            .and_then(|p| p.strip_suffix(".anibnd.dcx"))
            .ok_or("unexpected character binder path")?;
        let character = stem.get(..5).ok_or("character binder name shorter than cNNNN")?;
        let binder = Binder::read(&archive.read(&path, &decoder)?, &decoder)?;
        let mut local_files = 0usize;
        let mut local_events = 0usize;
        for file in &binder.files {
            let member = file.name.as_deref().unwrap_or("");
            let leaf = member
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(member)
                .to_ascii_lowercase();
            if let Some(set) = leaf.strip_suffix(".txt").or_else(|| leaf.strip_suffix(".blf"))
                && set.starts_with(&format!("{character}_"))
            {
                let companion = format!("/chr/{set}.anibnd.dcx");
                if archive.contains(&companion) && !visited.contains(&companion) && todo.insert(companion) {
                    marker_binders += 1;
                }
            }
            if !leaf.ends_with(".tae") {
                continue;
            }
            let tae = Tae::read(&decoder.decode_if_dcx(&file.bytes)?)?;
            tae_files += 1;
            local_files += 1;
            for animation in &tae.animations {
                match tae.resolved_animation(animation.id) {
                    Ok(Some(resolved))
                        if resolved.id != animation.id
                            && resolved.events.iter().any(|event| event.kind == 198) =>
                    {
                        imports_with_events += 1;
                        println!(
                            "TAE198_IMPORT={}",
                            json!({"binder":path,"file":member,"animation":animation.id,
                            "resolved_animation":resolved.id,"scope":"alias only; excluded from authored-instance histograms"})
                        );
                    }
                    Err(error) => {
                        import_refusals += 1;
                        println!(
                            "TAE198_IMPORT_REFUSAL={}",
                            json!({"binder":path,"file":member,"animation":animation.id,
                            "error":error.to_string(),"scope":"raw authored events below remain counted; unresolved aliases are not inferred"})
                        );
                    }
                    _ => {}
                }
                for (index, event) in animation
                    .events
                    .iter()
                    .enumerate()
                    .filter(|(_, event)| event.kind == 198)
                {
                    events += 1;
                    local_events += 1;
                    *lengths.entry(event.parameters.len()).or_insert(0usize) += 1;
                    let words = event
                        .parameters
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|bytes| u32::from_le_bytes(*bytes))
                        .collect::<Vec<_>>();
                    let row_id = words.get(1).copied().map(|word| word as i32);
                    let mut candidates = Vec::new();
                    for (word, value) in words.iter().enumerate() {
                        if word < histogram.len() {
                            *histogram[word].entry(*value).or_default() += 1;
                        }
                        if let Some(row) = material_rows.get(&(*value as i32)) {
                            candidates.push(json!({"byte":word*4,"row":row}));
                        }
                        if let Some(row) = row_id {
                            let counts = by_row.entry(row).or_insert_with(|| vec![BTreeMap::new(); 16]);
                            if word < counts.len() {
                                *counts[word].entry(*value).or_default() += 1;
                            }
                        }
                    }
                    if event.parameters.len() != 64 {
                        malformed += 1;
                    }
                    println!(
                        "TAE198_INSTANCE={}",
                        json!({"binder":path,"file":member,"animation":animation.id,"event":index,
                        "start":event.start,"end":event.end,"byte_length":event.parameters.len(),"raw":event.parameters,
                        "header_at_0":words.first().copied().map(units),
                        "payload_15_at_4":words.iter().skip(1).copied().map(units).collect::<Vec<_>>(),
                        "row_at_4":row_id.and_then(|id|material_rows.get(&id)),"row_candidates":candidates,
                        "scope":"authored event, no ramp/target semantics inferred from field position"})
                    );
                }
            }
        }
        let tally = characters
            .entry(character.to_owned())
            .or_insert_with(|| json!({"binders":0,"tae_files":0,"events198":0}));
        for (key, increment) in [
            ("binders", 1),
            ("tae_files", local_files),
            ("events198", local_events),
        ] {
            let total = tally[key].as_u64().ok_or("character tally is not an integer")? + increment as u64;
            tally[key] = json!(total);
        }
    }
    for (word, values) in histogram.iter().enumerate() {
        println!(
            "TAE198_WORD={}",
            json!({"byte":word*4,"payload_word":word.checked_sub(1),
            "values":values.iter().map(|(bits,count)|json!({"value":units(*bits),"count":count})).collect::<Vec<_>>()})
        );
    }
    for (row, words) in by_row {
        println!(
            "TAE198_ROW_WORDS={}",
            json!({"candidate_row_at_4":row,"row":material_rows.get(&row),
            "words":words.iter().enumerate().map(|(word,values)|json!({"byte":word*4,
                "values":values.iter().map(|(bits,count)|json!({"value":units(*bits),"count":count})).collect::<Vec<_>>()})).collect::<Vec<_>>()})
        );
    }
    println!(
        "TAE198_TOTAL={}",
        json!({"main_binders":main_binders,"dictionary_additional_binders":dictionary_binders,
        "marker_additional_binders":marker_binders,"visited_binders":visited.len(),"tae_files":tae_files,
        "authored_events198":events,"import_aliases_with198":imports_with_events,"import_refusals":import_refusals,"lengths":lengths,
        "unexpected_lengths":malformed,"characters":characters,
        "scope":"all c0000..c9999 main animation binders present in archive, dictionary animation binders and recursively named companion binders; authored records counted once per stored occurrence"})
    );
    assert!(
        main_binders > 0 && tae_files > 0,
        "no character TAE: census did not observe its denominator"
    );
    assert!(
        characters
            .get("c2120")
            .and_then(|row| row["events198"].as_u64())
            .is_some_and(|count| count > 0),
        "known c2120 events missing: census cannot observe the intended control"
    );
    Ok(())
}
